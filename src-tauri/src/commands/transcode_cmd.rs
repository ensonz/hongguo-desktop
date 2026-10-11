//! 转码能力查询与播放兼容兜底。
//!
//! 真正的转码实现在 [`hongguo_media::transcode`]，由合并功能
//! （`merge_service::compat`）与播放兼容兜底（`play_service::compat_play`）驱动。
//! 缓存清理已并入 storage_cmd（`get_cache_usage` / `clear_cache`）。

use std::path::PathBuf;

use serde::Serialize;
use tauri::Emitter;

use crate::service::download_service::events::names::COMPAT_PROGRESS;
use crate::service::play_service::compat_play::{self, CompatSource};
use hongguo_common::{AppError, AppResult};
use hongguo_media_platform::capability::probe::DecodeCapability;

/// 硬解/软解能力。
#[tauri::command]
pub fn decode_capability() -> DecodeCapability {
    hongguo_media_platform::capability::probe::detect()
}

/// 丢弃缓存重新探测一次 ffmpeg 与编码器。
///
/// 探测一次要跑 `-version` + `-encoders` + 逐个试编一帧，一到两秒。**用户装完
/// ffmpeg 之后进程的环境变量不会更新**，不重探就一直报「未检测到」——只能重启
/// 应用才生效。给出这条路后设置页就能点一下解决。
#[tauri::command]
pub fn redetect_capability() -> DecodeCapability {
    hongguo_media_platform::capability::probe::redetect()
}

/// 兜底转码的结果（前端直接消费）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatPlayResult {
    pub url: String,
    pub cached: bool,
    pub backend: String,
    pub elapsed_ms: u128,
}

/// 兼容转码进度事件的阶段。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompatPhase {
    Downloading,
    Transcoding,
    Ready,
}

/// 兼容转码进度事件载荷（`compat-play-progress`）。
///
/// 与前端 `compatProgressSchema` 两端镜像，改字段同一 commit 改另一端。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatProgressEvent {
    /// 缓存键 `{series_id}:{vid_index}`
    pub key: String,
    pub percent: f64,
    pub phase: CompatPhase,
}

/// 把一集转成 H.264 供本机解不了的机器播放。
///
/// 播放器确认「有声音没画面」后调它；转好的产物落进兼容缓存，同一集只转一次。
/// 已下载的走本地文件，没下载的现取现解。
#[tauri::command]
pub async fn transcode_for_playback(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::app_state::AppState>,
    series_id: String,
    vid_index: u32,
    vid: Option<String>,
) -> AppResult<CompatPlayResult> {
    let key = format!("{series_id}:{vid_index}");
    let emit = |percent: f64, phase: CompatPhase| {
        let _ = app.emit(
            COMPAT_PROGRESS,
            CompatProgressEvent {
                key: key.clone(),
                percent,
                phase,
            },
        );
    };

    // 优先用已下载的本地文件：不用再走网络，也不用等取流
    let local: Option<PathBuf> = state
        .queue()
        .completed_path(&series_id, vid_index)
        .map(PathBuf::from);

    let result = if let Some(p) = local {
        compat_play::ensure_compat(&series_id, vid_index, CompatSource::LocalFile(&p), &|pct| {
            emit(pct, CompatPhase::Transcoding)
        })
        .await
    } else {
        let Some(vid) = vid else {
            return Err(AppError::InvalidArgs("既没有本地文件，也没有 vid".into()));
        };
        let settings = state.settings();
        // 转码源必须保持全量候选：HEVC-only 的剧集兜底转码还要拿 HEVC 源
        let play = hongguo_api::play_url::fetch_play_url(
            &vid,
            None,
            hongguo_api::stream_pick::DecoderCapability::Full,
            &state.api_env(),
        )
        .await?;
        // 取流占前 20%，转码占后 80%：两段都动，但用户看到的是一条连续的进度
        emit(0.0, CompatPhase::Downloading);
        let plain = crate::service::play_service::online::fetch_plain(&play, &settings).await?;
        compat_play::ensure_compat(
            &series_id,
            vid_index,
            CompatSource::PlainBytes(plain),
            &|pct| emit(20.0 + pct * 0.8, CompatPhase::Transcoding),
        )
        .await
    };

    emit(100.0, CompatPhase::Ready);
    let r = result?;
    Ok(CompatPlayResult {
        url: r.url,
        cached: r.cached,
        backend: r.backend,
        elapsed_ms: r.elapsed_ms,
    })
}

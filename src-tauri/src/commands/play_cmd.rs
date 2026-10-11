//! 播放。

use tauri::State;

use hongguo_common::AppResult;
use hongguo_common::model::{PlayRequest, PlayResponse};
use hongguo_service::app_state::AppState;

#[tauri::command]
pub async fn play_series(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: PlayRequest,
) -> AppResult<PlayResponse> {
    hongguo_service::play_service::local::resolve_play(&app, &state, &request).await
}

/// 与首页起点决策同口径（home-page 的起点 effect：看过 >3 秒就续播那一集）。
/// 预取要瞄准的是切过去**真正会播**的那一集——固定第 1 集在看过几集的剧上
/// 永远命中不了，「你的内容」种子（历史/书架排头）部部如此。
const RESUME_THRESHOLD_SECS: f64 = 3.0;

/// 预取一集的在线流（沉浸流「一切就下一部」）。
///
/// 幂等且静默：本地没档案就先解析登记（通常前端已预取过分集，这里秒回），
/// 流已取/在取直接返回；取流表失败不报错——预取是优化，失败大不了现场取。
#[tauri::command]
pub async fn play_prefetch(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    series_id: String,
) -> AppResult<()> {
    let series = match state.store.series_by_id(&series_id)? {
        Some(s) => s,
        None => {
            let env = state.api_env();
            let s =
                hongguo_service::series_service::resolver::resolve_series(&series_id, &env).await?;
            hongguo_service::series_service::registry::upsert_and_persist(&state, s.clone())?;
            s
        }
    };
    // 起点对齐首页的续播决策：看过 >3 秒的剧预取上次看到的那一集；
    // 没看过的（或那一集已不在分集表里）回落第 1 集。
    let vid_index = match state.store.series_last_position(&series_id)? {
        Some((idx, p)) if p.current_time > RESUME_THRESHOLD_SECS => idx,
        _ => 1,
    };
    let ep = series
        .episodes
        .iter()
        .find(|e| e.vid_index == vid_index)
        .or_else(|| series.episodes.iter().find(|e| e.vid_index == 1));
    let Some(ep) = ep else {
        return Ok(());
    };
    if ep.vid.is_empty() {
        return Ok(());
    }
    let settings = state.settings();
    let env = state.api_env();
    hongguo_service::play_service::online::prefetch_stream(
        &app,
        &ep.vid,
        &format!("{series_id}:{}", ep.vid_index),
        &settings,
        &env,
    )
    .await
}

/// 同步 command 会占 **Tauri 主线程**——播放时每 5 秒一次的进度保存
/// 与其它 IPC 在主线程排队，是「窗口未响应」的候选元凶。这里改 async
/// 并把 DB 查询扔进阻塞线程池，主线程只做调度。
#[tauri::command]
pub async fn save_playback_position(
    state: State<'_, AppState>,
    series_id: String,
    vid_index: u32,
    current_time: f64,
    duration: f64,
) -> AppResult<()> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        hongguo_service::play_service::position::save_store(
            &store,
            &series_id,
            vid_index,
            current_time,
            duration,
        )
    })
    .await
    .map_err(|e| hongguo_common::AppError::Internal(format!("保存播放进度失败: {e}")))?
}

/// 一部剧最近看到的那一集（详情页「继续看」的数据源）。
///
/// 没看过（playback 表无记录）返回 None，前端回落云端观看历史。
/// 读是单行查询，但同样走阻塞线程池，不给主线程添堵。
#[tauri::command]
pub async fn series_progress(
    state: State<'_, AppState>,
    series_id: String,
) -> AppResult<Option<hongguo_common::model::SeriesProgress>> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.series_last_position(&series_id))
        .await
        .map_err(|e| hongguo_common::AppError::Internal(format!("读取播放进度失败: {e}")))?
        .map(|hit| {
            hit.map(|(vid_index, p)| hongguo_common::model::SeriesProgress {
                vid_index,
                current_time: p.current_time,
                duration: p.duration,
                updated_at: p.updated_at,
            })
        })
}

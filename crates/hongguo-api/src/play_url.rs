//! 取流地址。
//!
//! 平台把每集的可用流放在 `video_model`（一个 **JSON 字符串**）里，需要二次解析；
//! 挑哪一条由 [`super::stream_pick`] 决定。端点走单集
//! `video_model/v1/`（lq 域），见 [`fetch_play_url`] 注释。
//!
//! ⚠️ 签名失败时服务端返回 **HTTP 200 + 0 字节**，不是错误码。
//!    排查「接口没数据」时第一件事是看字节数，不是状态码。

use serde_json::Value;

use super::stream_pick::{Definition, list_definitions, pick_stream_at};
use hongguo_common::utils::json::check_code;
use hongguo_common::{AppError, AppResult};

/// 一集的取流结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayInfo {
    /// 视频直链
    pub url: String,
    /// 是否为 CENC 加密流
    pub encrypted: bool,
    /// 加密密钥材料（`spade_a` 原始值，派生在 worker 里做）
    pub key_material: Vec<u8>,
    /// 编码器标识
    pub codec: String,
    /// 实际选中的档位
    pub definition: u32,
    /// 本集提供的全部档位，供前端渲染切换菜单
    pub definitions: Vec<Definition>,
}

/// 取一集的播放地址。
///
/// `definition` 为 `Some` 时取该档位；平台没提供这一档时自动回退到最高档
/// （见 [`pick_stream_at`]），不会因此播放失败。
///
/// `cap` 决定选流模式：在线播放传本机探测结论（无 HEVC 扩展的机器自动
/// 翻转到 h264）；转码源/下载选流传 `Full` 保持全量候选。
///
/// 端点走**单集** [`crate::params::SINGLE_MODEL_PATH`]（lq 域）：2026-10-14
/// 抓包实锤批量形态 `multi_video_model/player/v1` 连 hgplayer 自己（全新
/// 注册设备）都收 200+0，真实播放路径是批量空 → 回落单集。我们直接单集，
/// 省一次注定为空的请求。
pub async fn fetch_play_url(
    vid: &str,
    definition: Option<u32>,
    cap: super::stream_pick::DecoderCapability,
    env: &super::client::ApiEnv,
) -> AppResult<PlayInfo> {
    let payload = serde_json::to_vec(&crate::params::single_model_payload(vid))
        .map_err(|e| AppError::Signer(e.to_string()))?;

    let bytes = crate::client::api_call_full(
        crate::client::LQ_API_ORIGIN,
        crate::params::SINGLE_MODEL_PATH,
        Some(payload),
        &[],
        env,
    )
    .await?;

    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Media(format!("解析响应失败: {e}")))?;

    // 业务错误码：签名对了但参数/内容不对会走这里
    check_code(&value)?;

    // 单集响应：data.video_model 是 JSON 字符串（与批量形态的
    // data[vid].video_model 同构，选流逻辑共用）
    let model = value
        .pointer("/data/video_model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Media("缺少 video_model".into()))?;

    let parsed: Value = serde_json::from_str(model)
        .map_err(|e| AppError::Media(format!("video_model 解析失败: {e}")))?;

    let stream = pick_stream_at(&parsed, definition, cap)
        .ok_or_else(|| AppError::Media("没有可用的视频流".into()))?;

    Ok(PlayInfo {
        url: stream.url,
        encrypted: !stream.spade.is_empty(),
        key_material: stream.spade.into_bytes(),
        codec: stream.codec.unwrap_or_default(),
        definition: stream.definition,
        definitions: list_definitions(&parsed, cap),
    })
}

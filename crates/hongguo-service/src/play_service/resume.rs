//! 起点决策：这一剧从第几集、什么位置开始播。
//!
//! 「看过 > [`RESUME_THRESHOLD_SECS`] 续播那一集」的单一实现：本地断点
//! （playback 表，播放期 5 秒一写）优先，云端观看历史（约 1 分钟一报）
//! 兜底，都没有回落第 1 集。此前沉浸流起播与下一剧预取各写一份、
//! 前端详情页/首页又各一份——阈值漂移和口径分叉就是这么来的。

use hongguo_api::history::WatchHistoryItem;
use hongguo_common::model::playback::RESUME_THRESHOLD_SECS;
use hongguo_common::AppResult;
use hongguo_store::Store;

/// 起点决策。返回 `(集号, 起播秒)`；无断点时 `(1, 0.0)`。
///
/// 云端行的 vid_index 0/1 基混杂（第三方上报 0 基），按既有口径对 0 钳 1。
pub fn resolve_entry(
    store: &Store,
    series_id: &str,
    history: &[WatchHistoryItem],
) -> AppResult<(u32, f64)> {
    if let Some((idx, p)) = store
        .series_last_position(series_id)?
        .filter(|(_, p)| p.is_resumable())
    {
        return Ok((idx, p.current_time));
    }
    let threshold_ms = (RESUME_THRESHOLD_SECS * 1000.0) as i64;
    if let Some(h) = history
        .iter()
        .find(|h| h.series_id == series_id && h.position_ms > threshold_ms)
    {
        return Ok((h.vid_index.max(1) as u32, h.position_ms as f64 / 1000.0));
    }
    Ok((1, 0.0))
}

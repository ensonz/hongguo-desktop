//! 在线播放进度设施：事件出口类型、节流上报器与强类型事件载荷。

use std::sync::Arc;

/// 进度事件出口：service 只负责产出事件，送达方式由壳决定
/// （Tauri 壳 emit 给前端；GPUI 壳现阶段没有订阅方，传 [`PlayRuntime::progress`] = None）。
pub type ProgressSink = Arc<dyn Fn(&OnlineProgressEvent) + Send + Sync>;

/// 在线播放进度上报器。
///
/// 复用下载服务那套节流（0.5% 或 500ms 才发一次）：取流是每秒好几 MB 的量，
/// 每次都发会把 UI 线程打满。事件经壳给的 [`ProgressSink`] 送达——service
/// 不关心前端在哪；没有 sink（GPUI 壳现阶段）时是纯空操作。
///
/// 进度不是可选装饰：整集取回 + 解密期间界面上只有一个转圈，用户既看不出在动
/// 还是卡住，也看不到还要多久。
pub struct ProgressReporter {
    sink: Option<ProgressSink>,
    /// 前端按 `{seriesId}:{vidIndex}` 过滤事件（episodeKey，与
    /// compat-play-progress 同一格式）——曾经发裸 vid 导致「正在缓存 X%」
    /// 永远匹配不上，别改回 vid。
    key: String,
    throttle: crate::download_service::events::ProgressThrottle,
}

impl ProgressReporter {
    pub fn new(sink: Option<ProgressSink>, key: String) -> Self {
        Self {
            sink,
            key,
            throttle: Default::default(),
        }
    }

    /// 报一次进度。`total` 为 0（CDN 没给 Content-Length）时不发百分比。
    pub fn report(&self, received: u64, total: u64, phase: &str) {
        let Some(sink) = &self.sink else {
            return;
        };
        let percent = if total > 0 {
            received as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        if !self.throttle.should_send(&self.key, percent) {
            return;
        }
        sink(&OnlineProgressEvent {
            key: self.key.clone(),
            received,
            total,
            percent: percent.min(100.0),
            phase: phase.to_string(),
        });
    }

    /// 100%：解密完成、缓冲就绪。
    pub fn done(&self) {
        self.throttle.forget(&self.key);
        let Some(sink) = &self.sink else {
            return;
        };
        sink(&OnlineProgressEvent {
            key: self.key.clone(),
            received: 0,
            total: 0,
            percent: 100.0,
            phase: "ready".to_string(),
        });
    }
}

/// 在线播放进度事件（`online-play-progress`，红线 10：强类型载荷）。
///
/// `key` 是 `{seriesId}:{vidIndex}`（episodeKey），前端按它过滤事件——
/// 曾经发裸 vid 导致「正在缓存 X%」永远匹配不上，别改回 vid。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OnlineProgressEvent {
    pub key: String,
    pub received: u64,
    pub total: u64,
    pub percent: f64,
    pub phase: String,
}

//! 事件契约：[`PlayerEvent`] 引擎通报与 [`OpenOptions`] 打开参数。

use tokio::sync::mpsc;

/// 引擎事件（经调用方给的通道送达；通道关闭 = 宿主已弃，引擎自停）。
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// 首帧已进上屏槽（宿主据此触发下一部预取）。
    FirstFrame { width: usize, height: usize },
    /// 新一帧可上屏，**帧随事件一起送达**。
    ///
    /// 不带帧、让宿主回头去 [`PlayerHandle::latest_frame`](super::PlayerHandle::latest_frame) 取，会有致命竞态：
    /// 宿主在「引擎就位」时清一次帧槽，首帧事件若先于就位到达，清空就把
    /// 「已出画」的事实抹掉了，封面永远收不掉。帧跟着事件走，宿主只认事件
    /// 里的帧，顺序天然正确。
    Frame(hongguo_media_platform::platform::video::VideoFrame),
    /// 缓冲断流：设备已暂停、时钟冻结（水位见 [`BUFFER_LOW_SECS`](super::feeder::BUFFER_LOW_SECS)）。
    Buffering,
    /// 断流恢复：设备重新起跑。
    Ready,
    /// 自然播完。
    Ended,
    /// 致命失败（引擎已停）。
    Failed(String),
}

/// 打开参数。
pub struct OpenOptions {
    /// 流的 vid（调用方已 `prepare`，缓存里有这一档）。
    pub vid: String,
    pub definition: u32,
    /// 起播媒体时间（秒）：续播位置。0 = 从头。
    pub start_secs: f64,
    /// 事件通道。
    pub events: mpsc::Sender<PlayerEvent>,
}

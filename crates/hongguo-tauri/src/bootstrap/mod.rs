//! 启动装配：顺序即依赖顺序。
//!
//! 关键路径（决定窗口多快能亮出）：加载数据 → 事件订阅 → 把待跑任务推入
//! 调度。不拦启动的重活一律后台化——设备健康复检（async）、磁盘扫描补
//! 登记、转码能力探测（spawn_blocking）：它们的结果晚几秒到，用户感知
//! 只是「列表/能力标记稍后出现」，不该让启动白等。

pub mod device;
pub mod downloader;
pub mod events;
pub mod rescan;
pub mod store;
pub mod transcoder;

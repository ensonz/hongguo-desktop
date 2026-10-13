//! 服务事件泵：订阅 `hongguo_service::events`，转发为 GPUI 侧可 await 的通道。
//!
//! service 的业务事件（登录 MFA 状态、下载进度/状态等）经 tokio broadcast
//! 发布；本泵把它们转发进 `tokio::sync::mpsc` 无界通道——接收端是同步
//! 原语，不依赖 tokio 上下文，可在 GPUI 执行器上直接 await（与
//! `bridge.rs` 同款约定）。消费方（HomeView）在构造时起一条 `cx.spawn`
//! 循环，把事件落到视图状态——与 Tauri 壳消费同一批事件（一份业务实现，
//! 两种 UI 适配）。

use hongguo_service::app_state::AppState;
use hongguo_service::events::ServiceEvent;
use tokio::sync::mpsc;

/// 订阅服务事件，返回 GPUI 侧可 await 的接收端。
pub(crate) fn subscribe(state: &AppState) -> mpsc::UnboundedReceiver<ServiceEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut events = state.events().subscribe();
    hongguo_service::runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ev) => {
                    if tx.send(ev).is_err() {
                        break; // 消费方已退出
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("[Events] GPUI 事件泵滞后，丢弃 {n} 条旧事件");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    rx
}

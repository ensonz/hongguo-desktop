//! tokio 桥：把 service 的 async 调用投到常驻 tokio 运行时，经 oneshot
//! 回传给 GPUI 执行器。`Receiver` 不依赖 tokio 上下文，可在 GPUI 任务里
//! 直接 await（`main.rs` 有桥的背景说明）。

use std::future::Future;

use tokio::sync::oneshot;

/// 把一个 future 丢进常驻 tokio 运行时执行，返回可跨执行器 await 的接收端。
pub(crate) fn spawn<T, F>(future: F) -> oneshot::Receiver<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    crate::tokio().spawn(async move {
        let _ = tx.send(future.await);
    });
    rx
}

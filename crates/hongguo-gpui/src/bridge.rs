//! tokio 桥：把 service 的 async 调用投到常驻 tokio 运行时，经 oneshot
//! 回传给 GPUI 执行器。`Receiver` 不依赖 tokio 上下文，可在 GPUI 任务里
//! 直接 await（`main.rs` 有桥的背景说明）。
//!
//! **不变量集中说明**：壳里大量 `rx.await.expect("tokio 桥不会丢弃发送端")`
//! 依赖本文件保证——`spawn` 总是把发送端移进运行时任务，任务要么送回
//! `Ok` 要么送回 `Err`（`let _ = tx.send(...)` 不吞结果），接收端因此
//! 只在运行时自身死亡时才会收到 RecvError；而运行时由 `main.rs` 常驻
//! 到进程结束。所以这些 expect 是不变量断言，不是业务错误的吞点——
//! 新增调用请沿用同一措辞，别在此处加兜底。

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

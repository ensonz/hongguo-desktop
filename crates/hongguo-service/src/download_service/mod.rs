//! 下载服务：任务生命周期编排。
//!
//! 职责分五块，各占一个文件：
//! - [`queue`]：任务集合与状态流转
//! - [`scheduler`]：后台调度，按并发上限派发任务
//! - [`worker`]：单集执行（取流 → 下载 → 解密 → 落盘）
//! - [`events`]：进度节流与事件发射
//! - [`rescan`]：从磁盘文件补回丢失的任务记录
//!
//! 本文件只做组装：把调度器挂到全局状态上，供 command 层触发。

pub mod events;
pub mod queue;
pub mod rescan;
pub mod scheduler;
pub mod worker;

use crate::app_state::AppState;
use hongguo_common::AppResult;
use hongguo_store::Store;

/// 把下载队列快照写回数据库。
///
/// 队列是内存态，落盘失败不必让用户的操作失败，但没有日志时「重启后任务
/// 不见了」根本无从查起，所以这里一律记下来。`tag` 是调用方的模块名，
/// 用来区分是哪条路径触发的落盘。
///
/// 归属：本函数原先挂在 `store` 模块上，但它的两个入参都由 [`AppState`]
/// 持有——留在持久化层会让 store 反向依赖 UI 壳的状态。队列落盘是下载域的
/// 编排，故落在下载服务里，且签名不含 `AppState`（便于服务层后续脱离壳）。
pub fn persist_tasks(store: &Store, queue: &queue::DownloadQueue, tag: &str) -> AppResult<()> {
    let tasks = queue.all();
    store
        .replace_tasks(&tasks)
        .inspect_err(|e| log::error!("[{tag}] 落盘失败: {e}"))
}

/// 触发一轮调度。
///
/// 在「提交任务」「一键启动」「启动恢复」之后调用；每个任务完成时会自动再触发。
/// 后台任务走共用任务运行时（任意线程可调，见 [`crate::runtime`]）。
pub fn kick(state: AppState) {
    let sched = state.scheduler();
    crate::runtime::spawn(async move {
        sched.run_once(state).await;
    });
}

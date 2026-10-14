//! 下载队列 command。
//!
//! 按职责拆分：
//! - 本文件：查询（任务列表、队列状态）
//! - [`actions`]：变更（提交、暂停、重试、删除、补登记）——编排在
//!   service 的 `download_service::actions`

use tauri::State;

use hongguo_common::model::{DownloadTask, QueueStatus};
use hongguo_service::app_state::AppState;

mod actions;
mod cleanup;

pub use actions::*;

pub use cleanup::*;

/// 全部下载任务。
#[tauri::command]
pub fn get_download_tasks(state: State<'_, AppState>) -> Vec<DownloadTask> {
    state.queue().all()
}

/// 队列状态概览。
#[tauri::command]
pub fn get_queue_status(state: State<'_, AppState>) -> QueueStatus {
    state.queue().status()
}

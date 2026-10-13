//! 下载能力（GPUI 壳）：入队 / headless 调度 / 完成集查询。
//!
//! 复用 `download_service` 的队列与 worker（queue 状态机、进度节流、
//! 临时文件防损、persist_tasks 落盘全在服务层），这里只补一块：
//! Tauri 版调度器依赖 `tauri::AppHandle`（emit 事件 + async_runtime），
//! GPUI 壳没有这两样——调度循环在常驻 tokio 运行时里自转，进度写入
//! 队列状态，面板轮询读取（壳无事件总线，轮询是唯一消费方式）。
//!
//! 队列恢复在壳启动时完成（`restore` 把 running 视为中断改回 pending）。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use hongguo_common::AppError;
use hongguo_common::model::{DownloadTask, TaskStatus};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::download_service::events::ProgressThrottle;
use hongguo_service::download_service::worker::ProgressSink;
use hongguo_service::download_service::{persist_tasks, worker};

/// 入队一批集（已在队列里的自动跳过），返回新入队数。
pub fn enqueue_batch(state: &Arc<AppStateInner>, series_id: &str, vids: &[u32]) -> usize {
    let Ok(series) = state.store.series_by_id(series_id) else {
        return 0;
    };
    let Some(series) = series else {
        return 0;
    };
    let mut added = 0usize;
    for vid_index in vids {
        let Some(episode) = series.episodes.iter().find(|e| e.vid_index == *vid_index) else {
            continue;
        };
        let task = DownloadTask::new(
            &series.series_id,
            &series.title,
            episode.vid_index,
            &episode.vid,
            &episode.title,
        );
        if state.queue().enqueue(task.clone()).id != task.id {
            continue; // 已在队列里
        }
        added += 1;
    }
    if added > 0 {
        let _ = persist_tasks(&state.store, &state.queue(), "Download");
        kick(Arc::clone(state));
    }
    added
}

/// 触发一轮调度（headless）：无 runnable 时自然退出，入队方再 kick。
pub fn kick(state: Arc<AppStateInner>) {
    crate::tokio().spawn(async move {
        schedule_loop(state).await;
    });
}

/// headless 调度循环：按并发上限把 runnable 任务全部派出去，等全部
/// 结束后落盘。Tauri 版调度器的事件 emit 在壳里没有订阅方，这里省略；
/// 面板直接轮询队列状态。
async fn schedule_loop(state: Arc<AppStateInner>) {
    let limit = state.queue().limit();
    let mut joins = Vec::new();
    for _ in 0..limit {
        let Some(task) = state.queue().next_runnable() else {
            break;
        };
        if !state.queue().mark_running(&task.id) {
            continue; // 状态已被别的流程改掉，换下一个
        }
        let st = Arc::clone(&state);
        joins.push(crate::tokio().spawn(async move { run_one(st, task).await }));
    }
    if joins.is_empty() {
        return;
    }
    for j in joins {
        let _ = j.await;
    }
    let _ = persist_tasks(&state.store, &state.queue(), "Download");
}

/// 一集的完整流程：取流地址 → 下载 → 解密（scheduler::run_one 的
/// headless 复刻，emit 换成无操作）。
async fn run_one(state: Arc<AppStateInner>, task: DownloadTask) {
    let id = task.id.clone();
    let settings = state.settings();

    // 1) 取流地址（下载固定取最高档，与 Tauri 调度器同口径）
    let play = match hongguo_api::play_url::fetch_play_url(
        &task.vid,
        None,
        hongguo_api::stream_pick::DecoderCapability::Full,
        &state.api_env(),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            let msg = e.to_string();
            state.queue().mark_failed(&id, &msg);
            log::warn!("[Download] {} 拉流失败: {msg}", task.vid_index);
            return;
        }
    };

    // 2) 输出路径
    let series_title = state
        .store
        .series_by_id(&task.series_id)
        .ok()
        .flatten()
        .map(|s| s.title)
        .unwrap_or_else(|| task.series_title.clone());
    let file_name = settings.render_file_name(&task.series_title, task.vid_index, &task.ep_title);
    let output = settings
        .series_dir(&series_title)
        .join(format!("{file_name}.mp4"));

    // 3) 下载 + 解密（取消标志从取消槽取：stop_task 置位后 worker 在下个
    // 分片检查点退出，见 cancel_flags/stop_task）
    let cancelled = cancel_flag(&id);
    let episode = hongguo_service::download_service::worker::EpisodeDownload {
        id: id.clone(),
        video_url: play.url,
        output_path: output.clone(),
        encrypted: play.encrypted,
        key_material: play.key_material,
        settings: settings.clone(),
        cancelled: &cancelled,
    };
    let queue = state.queue();
    let progress_id = id.clone();
    let sink = ProgressSink {
        throttle: Arc::new(ProgressThrottle::default()),
        on_progress: Arc::new(move |downloaded, total| {
            queue.update_progress(&progress_id, downloaded, total);
        }),
    };

    match worker::download_episode(&episode, &sink).await {
        Ok(size) => {
            let path = output.to_string_lossy().to_string();
            state.queue().mark_completed(&id, &path, size);
            log::info!("[Download] 第 {} 集完成（{size}B）", task.vid_index);
        }
        // 用户停止：状态保持 stopped（stop_task 已标），不能落成 failed
        Err(AppError::Cancelled) => {
            log::info!(
                "[Download] 第 {} 集已停止（保留临时文件供重试）",
                task.vid_index
            );
        }
        Err(e) => {
            let msg = e.to_string();
            state.queue().mark_failed(&id, &msg);
            log::warn!("[Download] 第 {} 集失败: {msg}", task.vid_index);
        }
    }
    cancel_flags().lock().unwrap().remove(&id);
    let _ = persist_tasks(&state.store, &state.queue(), "Download");
}

/// 单任务取消标志槽（task_id → flag）：stop_task 置位，run_one 借给
/// worker 的分片检查点。std Mutex（壳没有 parking_lot 依赖，与
/// danmaku_emoji 同口径）。
fn cancel_flags() -> &'static std::sync::Mutex<HashMap<String, Arc<AtomicBool>>> {
    static CANCELS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> =
        std::sync::OnceLock::new();
    CANCELS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn cancel_flag(id: &str) -> Arc<AtomicBool> {
    let mut flags = cancel_flags().lock().unwrap();
    flags
        .entry(id.to_string())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// 停止单个任务：置位取消标志（worker 在下个分片检查点退出并清临时
/// 文件）+ 队列状态机标 stopped + 落盘。返回任务是否存在。
pub fn stop_task(state: &Arc<AppStateInner>, id: &str) -> bool {
    if state.queue().get(id).is_none() {
        return false;
    }
    cancel_flag(id).store(true, std::sync::atomic::Ordering::SeqCst);
    state.queue().mark_stopped(id);
    let _ = persist_tasks(&state.store, &state.queue(), "Download");
    true
}

/// 重试单个任务（stopped/failed → pending 重新排队）并触发调度。
pub fn retry_task(state: &Arc<AppStateInner>, id: &str) -> bool {
    if state.queue().retry(id).is_none() {
        return false;
    }
    let _ = persist_tasks(&state.store, &state.queue(), "Download");
    kick(Arc::clone(state));
    true
}

/// 某剧已下载完成的集号（选集浮层完成角标数据源）。
pub fn completed_vids(state: &Arc<AppStateInner>, series_id: &str) -> Vec<u32> {
    state
        .queue()
        .of_series(series_id)
        .into_iter()
        .filter(|t| t.status == TaskStatus::Completed && !t.file_path.is_empty())
        .map(|t| t.vid_index)
        .collect()
}

/// 某剧的全部任务快照（下载面板数据源）。
pub fn tasks_of(state: &Arc<AppStateInner>, series_id: &str) -> Vec<DownloadTask> {
    state.queue().of_series(series_id)
}

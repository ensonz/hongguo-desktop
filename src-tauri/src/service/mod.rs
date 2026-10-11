//! 应用服务层。
//!
//! 与 [`crate::commands`] 同名同构：看目录就能定位 command → service 的对应关系。

pub mod capability_service;
pub mod download_service;
pub mod login_service;
pub mod merge_service;
pub mod play_service;
pub mod series_service;
pub mod settings_service;
pub mod storage_service;
pub mod transcode_service;

#[cfg(test)]
mod tests {
    /// 与命令层同一纪律：service 层同样可能在无 Tokio 上下文的线程（协议
    /// 回调、主线程）上被调用，裸 `tokio::spawn` / `Handle::current` /
    /// `block_on` 会 panic→abort。后台任务一律 `tauri::async_runtime::spawn`
    /// （全局运行时，任意线程可调）——典型反例见 `play_service::online` 的
    /// `resume_fill`（同步入口，可能被协议线程调用）。async fn 内
    /// `tokio::time::sleep` / `tokio::sync::*` 这类运行时内调用不在禁止之列；
    /// 测试代码允许自建运行时，故先切掉每个文件 `#[cfg(test)]` 之后的内容再扫。
    #[test]
    fn services_do_not_touch_tokio_context_directly() {
        const SOURCES: &[&str] = &[
            include_str!("capability_service/mod.rs"),
            include_str!("download_service/events.rs"),
            include_str!("download_service/mod.rs"),
            include_str!("download_service/queue.rs"),
            include_str!("download_service/queue_query.rs"),
            include_str!("download_service/rescan.rs"),
            include_str!("download_service/scheduler.rs"),
            include_str!("download_service/worker.rs"),
            include_str!("login_service/account.rs"),
            include_str!("login_service/csrf.rs"),
            include_str!("login_service/events.rs"),
            include_str!("login_service/mod.rs"),
            include_str!("login_service/polling.rs"),
            include_str!("merge_service/compat.rs"),
            include_str!("merge_service/guard.rs"),
            include_str!("merge_service/mod.rs"),
            include_str!("merge_service/prepare.rs"),
            include_str!("merge_service/progress.rs"),
            include_str!("merge_service/quick.rs"),
            include_str!("play_service/compat_play.rs"),
            include_str!("play_service/compat_prefetch.rs"),
            include_str!("play_service/local.rs"),
            include_str!("play_service/mod.rs"),
            include_str!("play_service/online.rs"),
            include_str!("play_service/position.rs"),
            include_str!("series_service/mod.rs"),
            include_str!("series_service/registry.rs"),
            include_str!("series_service/resolver.rs"),
            include_str!("settings_service/mod.rs"),
            include_str!("settings_service/proxy.rs"),
            include_str!("storage_service/cleanup.rs"),
            include_str!("storage_service/mod.rs"),
            include_str!("storage_service/usage.rs"),
            include_str!("transcode_service/cache.rs"),
            include_str!("transcode_service/mod.rs"),
            include_str!("transcode_service/pipeline.rs"),
        ];
        for src in SOURCES {
            let runtime_src = src.split("#[cfg(test)").next().unwrap_or(src);
            for banned in ["tokio::spawn(", "Handle::current(", "block_on("] {
                assert!(
                    !runtime_src.contains(banned),
                    "service 层不允许裸用 {banned}：无 Tokio 上下文的调用点会 panic→abort。\n\
                     起后台任务请用 tauri::async_runtime::spawn。出处片段：\n{runtime_src}"
                );
            }
        }
    }
}

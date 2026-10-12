//! 应用服务层（hongguo-service crate）。
//!
//! 业务编排：download / play / merge / login / series / settings / storage /
//! transcode / capability 九个域，外加全局业务状态 [`app_state`]（经壳的
//! Tauri `manage` 注入；GPUI 壳则直接构造）。壳的差异被挡在外：在线取流
//! 编排（`play_service::online`）已经壳无关（运行时与进度事件经
//! [`play_service::online::PlayRuntime`] 由壳注入），其余域仍以
//! `tauri::async_runtime` 起后台任务、`AppHandle` 形参发事件——本 crate
//! 不含任何窗口/IPC 逻辑。
//!
//! 与壳的 `commands` 同名同构：看目录就能定位 command → service 的对应关系。

pub mod app_state;
pub mod capability_service;
pub mod device;
pub mod download_service;
pub mod login_service;
pub mod merge_service;
pub mod play_service;
pub mod series_service;
pub mod settings_service;
pub mod storage_service;
pub mod transcode_service;

pub use app_state::{AppState, AppStateInner};

#[cfg(test)]
mod tests {
    /// 与命令层同一纪律：service 层同样可能在无 Tokio 上下文的线程（协议
    /// 回调、主线程）上被调用，裸 `tokio::spawn` / `Handle::current` /
    /// `block_on` 会 panic→abort。后台任务要么 `tauri::async_runtime::spawn`
    /// （全局运行时，任意线程可调），要么走壳显式注入的运行时 handle
    /// （`play_service::online::PlayRuntime`，语义等价且壳无关）——典型反例
    /// 见 `play_service::online` 的 `resume_fill`（同步入口，可能被协议线程
    /// 调用）。async fn 内 `tokio::time::sleep` / `tokio::sync::*` 这类
    /// 运行时内调用不在禁止之列；测试代码允许自建运行时，故先切掉每个文件
    /// `#[cfg(test)]` 之后的内容再扫。
    ///
    /// 源码清单由目录自动发现（`collect_sources`，随 crate 迁移内联），不再
    /// 手工登记——新增 service 文件漏登记曾让本守护静默失效。
    #[test]
    fn services_do_not_touch_tokio_context_directly() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = collect_sources(&src);
        assert!(
            !sources.is_empty(),
            "没扫到 service 层源码：{}",
            src.display()
        );
        for (path, text) in &sources {
            let runtime_src = text.split("#[cfg(test)").next().unwrap_or(text);
            for banned in ["tokio::spawn(", "Handle::current(", "block_on("] {
                assert!(
                    !runtime_src.contains(banned),
                    "service 层不允许裸用 {banned}：无 Tokio 上下文的调用点会 panic→abort。\n\
                     起后台任务请用 tauri::async_runtime::spawn。出处：{}",
                    path.display()
                );
            }
        }
    }

    /// 递归收集 `dir` 下所有生产态 `.rs`（跳过 `*_tests.rs` 兄弟测试文件——
    /// 它们可能合法使用测试专用 runtime）。与壳的
    /// `testing::source_guard` 同一份逻辑；跨 crate 后各自内联，改口径要同步。
    fn collect_sources(dir: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                    continue;
                }
                let is_rs = path.extension().and_then(|ext| ext.to_str()) == Some("rs");
                let is_test_file = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("_tests.rs"));
                if is_rs
                    && !is_test_file
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    out.push((path, text));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, &mut out);
        out
    }
}

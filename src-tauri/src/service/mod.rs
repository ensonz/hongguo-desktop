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
    ///
    /// 源码清单由目录自动发现（[`crate::testing::source_guard`]），不再手工登记——
    /// 新增 service 文件漏登记曾让本守护静默失效。
    #[test]
    fn services_do_not_touch_tokio_context_directly() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service");
        let sources = crate::testing::source_guard::collect_sources(&dir);
        assert!(!sources.is_empty(), "没扫到 service 层源码：{}", dir.display());
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
}

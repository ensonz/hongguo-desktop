//! 本 crate 的测试基建（不进生产树）。
//!
//! 放在 src-tauri 而不是 hongguo-common：这些工具服务于**本 crate 源码**的
//! 元测试（commands/service 层禁裸 tokio 等）。而依赖 crate 对下游是以非
//! test 方式编译的——放 common 里的 `#[cfg(test)]` 模块，下游元测试根本
//! 看不见（`#[cfg(test)]` 只对编译该 crate 自身的 test 生效）。
//!
//! [`media_pipeline_e2e`] 是跨层端到端验证（媒体核心 + 平台/ffmpeg + 服务层），
//! 三边都够不着单一 crate，只能留在壳里。

#[cfg(test)]
pub mod media_pipeline_e2e;
#[cfg(test)]
pub mod source_guard;

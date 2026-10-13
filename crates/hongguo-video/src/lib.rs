//! MP4 解析与 CENC 解密（hongguo-video crate）。
//!
//! - [`r#box`]：box 树遍历
//! - [`sample_table`]：样本表与加密辅助信息
//! - [`chunk_map`]：chunk → 样本 偏移换算
//! - [`cenc_info`]：`saiz` / `senc` 加密辅助信息
//! - [`deprotect`]：摘掉加密标记，还原成普通可播放的 MP4
//! - [`decrypt_file`]：落盘解密
//! - [`decrypt_buffer`]：内存解密（在线播放用）
//! - [`streaming`]：流式解密（按 Range 惰性解，不整集等待）
//! - [`crypto`]：CENC-AES-CTR 与密钥派生原语
//! - [`esds`]：`esds` → AudioSpecificConfig（AAC 解码器的 extradata）
//!
//! `fixtures` 只在测试构建里存在：解析层的测试要造 MP4 时共用同一份字节构造。

pub mod crypto;

pub mod r#box;
pub mod cenc_info;
pub mod chunk_map;
pub mod decrypt_buffer;
pub mod decrypt_file;
pub mod deprotect;
pub mod esds;
pub mod sample_table;
pub mod streaming;
pub mod timing;

// 测试辅助模块的本 crate 与下游 crate 的测试都要用（构造 MP4 样本）。用 feature
// 显式开启：依赖 crate 对下游以非 test 方式编译，`#[cfg(test)]` 对下游不可见，
// 不加 feature 就只能让测试代码常驻生产构建。见 Cargo.toml 的 `test-support`。
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;

#[cfg(any(test, feature = "test-support"))]
#[path = "streaming_tests.rs"]
pub mod streaming_tests;

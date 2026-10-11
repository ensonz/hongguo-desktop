//! 平台层编解码（hongguo-media-platform crate）。
//!
//! - [`platform`]：平台原生硬编（macOS VideoToolbox / Windows Media Foundation /
//!   WIC 静图）——分发门面在 `platform.rs`，按 `#[cfg(target_os)]` 编译期二选一
//! - [`ffmpeg`]：有 ffmpeg 时的加速路径（探测 + 执行）
//! - [`capability`]：解码能力探测（平台硬解与 ffmpeg 一起探）
//!
//! 纯 Rust 的编解码（backend / demux / hevc / remux / codec_probe / disk_space /
//! transcode / heif）在 `crates/hongguo-media`；本 crate 只做平台硬编、ffmpeg
//! 执行与能力探测，依赖方向是「平台 → 核心」，无反向（见
//! `docs/architecture-refactor.md` §4.2）。转码的**分流执行**（平台 → ffmpeg →
//! 纯 Rust 三级瀑布）在壳的 `service::transcode_service::pipeline`——编排留在上层，
//! 本 crate 只提供每条路的能力与执行。
//!
//! 本 crate 不依赖 tauri。

pub mod capability;
pub mod ffmpeg;
pub mod platform;

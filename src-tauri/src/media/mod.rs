//! 平台编解码与 ffmpeg 执行。
//!
//! - [`ffmpeg`]：有 ffmpeg 时的加速路径（探测 + 执行）
//! - [`platform`]：平台原生硬编（macOS VideoToolbox / Windows Media Foundation / WIC 静图）
//! - [`capability`]：解码能力探测（平台硬解与 ffmpeg 一起探）
//!
//! 纯 Rust 的编解码（backend / demux / hevc / remux / codec_probe / disk_space /
//! transcode / heif）已迁到 `crates/hongguo-media`；本模块**整体待迁**为
//! `hongguo-media-platform`（见 `docs/architecture-refactor.md` §4.2、§8 第 3 条 3c）。
//! 三个子模块当前都按 `hongguo_media::…` 引用核心，方向是「平台 → 核心」，无反向。

pub mod capability;
pub mod ffmpeg;
pub mod platform;

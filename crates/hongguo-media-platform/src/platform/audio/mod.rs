//! 平台 AAC 解码出口：macOS = AudioToolbox（AudioConverter），Windows =
//! Media Foundation（解码 MFT）。两端同一接口：ASC 建解码器、逐包解码出
//! 交错 f32（声道/采样率按 mp4 声明）、seek 冲流 reset——引擎与 profile
//! 无关（HE-AAC 的 SBR/PS 展开由平台解码器完成）。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::AacDecoder;

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::AacDecoder;

//! AAC 解码出口：契约在 api crate，具体类型按编译目标 re-export。
//!
//! macOS 实现走 AudioToolbox（AudioConverter，mpv 的 coreaudio AAC 解码
//! 同款），Windows 实现走 Media Foundation（解码 MFT）；同一接口：
//! ASC 建解码器、逐包解码出交错 f32、seek 冲流 reset。

pub use hongguo_media_platform_api::audio::AudioDecoder;

#[cfg(target_os = "macos")]
pub use hongguo_media_platform_macos::audio::AacDecoder;
#[cfg(target_os = "windows")]
pub use hongguo_media_platform_windows::audio::AacDecoder;

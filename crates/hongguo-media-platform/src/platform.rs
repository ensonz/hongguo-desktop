//! 平台原生编码后端。
//!
//! - macOS：VideoToolbox（[`vt`]）——HEVC 硬解 + H.264 编码（硬编优先，
//!   无硬编回落 Apple 内置软编会话），CVPixelBuffer 全程 NV12，
//!   编解码之间零拷贝是自然形态
//! - Windows：Media Foundation（[`mf`]）——硬件 MFT 枚举 + async 事件循环
//!   （保持硬编闸门：h264_mf 软编不值得替代 ffmpeg）
//!
//! 探测纪律与 ffmpeg 侧一致：**列表里有 ≠ 能用**，必须真实建会话试编一帧
//! （试编帧不能小于 256×256，nvenc 曾在 64×64 上误判不可用）。
//! 任何探测失败都安静地返回不可用，让分流链落到 ffmpeg/软解。

#[cfg(target_os = "windows")]
pub mod mf;
#[cfg(target_os = "macos")]
pub mod vt;
// WIC：Windows 的封面平台级静图解码（HEIF/HEIC 扩展在则走系统解码器）
#[cfg(target_os = "windows")]
pub mod wic;

use std::path::Path;

use hongguo_common::AppResult;

/// 平台硬编的一次转码请求（与 ffmpeg 侧的 [`crate::ffmpeg::TranscodeRequest`] 同构）。
pub struct PlatformRequest<'a> {
    /// 已解密的源 MP4
    pub input: &'a Path,
    /// 输出路径（内部先写临时文件，成功后原子替换）
    pub output: &'a Path,
    /// 统一到这个分辨率。`None` 表示保持源分辨率
    pub scale_to: Option<(u32, u32)>,
    /// 已编码秒数的回调（0.0–时长秒），用于集内进度
    pub on_progress: Option<&'a (dyn Fn(f64) + Send + Sync)>,
}

/// 平台编码层可用时执行 HEVC→H.264 转码。
///
/// 返回 `None` 表示本机平台层不接这个源（HEVC 以外；或被 `HONGGUO_NO_PLATFORM`
/// 关闭、Windows 侧无硬编 MFT），调用方落回分流链的下一条路；`Some(Err)`
/// 表示试过但失败，同样落回。
pub fn transcode_h264(req: &PlatformRequest<'_>) -> Option<AppResult<()>> {
    if disabled_by_env() {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        vt::transcode_h264(req)
    }
    #[cfg(target_os = "windows")]
    {
        mf::transcode_h264(req)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = req;
        None
    }
}

/// 平台**硬编**编码器当前是否可用（含探测缓存）。
///
/// 只喂能力徽标与并行度策略——macOS 的生产闸门是 [`encoder_available`]，
/// 软编会话也算数。
pub fn h264_hw_encoder_available() -> bool {
    if disabled_by_env() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        vt::h264_hw_encoder_available()
    }
    #[cfg(target_os = "windows")]
    {
        mf::h264_hw_encoder_available()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// 平台编码层当前是否可用（含探测缓存）。
///
/// macOS：VT 会话可建即真（含 Apple 软编会话）；Windows：仅指有硬件 MFT
/// （h264_mf 软编不接）。`selected_backend` 与分流链的准入口径。
pub fn encoder_available() -> bool {
    if disabled_by_env() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        vt::encoder_available()
    }
    #[cfg(target_os = "windows")]
    {
        mf::h264_hw_encoder_available()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// HEVC 解码 MFT 是否在位（Windows；macOS 的 WKWebView 原生可解 HEVC，恒真）。
///
/// 回答的是「在线播放能否把 HEVC 系流直接喂给 WebView」，与转码侧探测
/// （[`h264_hw_encoder_available`] 等，回答「转码走哪条路」）是两个问题；
/// 判据同源（同一个 HEVC 解码 MFT 枚举），但不吃 `HONGGUO_NO_PLATFORM`——
/// 那个开关关的是转码平台层，与播放解码无关。
pub fn hevc_decode_available() -> bool {
    #[cfg(target_os = "windows")]
    {
        mf::hevc_decoder_present()
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

/// 丢弃平台侧的探测缓存，下次读取时重新探测（设置页「重新检测」用）。
pub fn clear_probe_cache() {
    #[cfg(target_os = "macos")]
    vt::clear_probe_cache();
    #[cfg(target_os = "windows")]
    {
        mf::clear_probe_cache();
        // WIC 的 HEIF 解码器探测缓存一并清（封面平台级，见 platform/wic）
        wic::clear_probe_cache();
    }
}

/// 测试开关：强制按「平台硬编不可用」处理，验证回退链。
///
/// 与 `HONGGUO_NO_FFMPEG` 同一套约定。
pub fn disabled_by_env() -> bool {
    std::env::var_os("HONGGUO_NO_PLATFORM").is_some()
}

/// 仅测试构建：跑完整平台链路（解码/Annex-B/封装/时间戳），让没有硬编的
/// 机器（含 CI）也能验证管线本身。
///
/// macOS 的生产入口已经就是「硬编优先、软编回落」，直接走它——e2e 测的
/// 就是发布行为；Windows 仍分发到 `_for_tests`（h264_mf 软编仅测试通道），
/// 让没有硬编 MFT 的机器也能验证管线。e2e 统一从这里走，不直接引用
/// `vt::`/`mf::`（否则另一平台的测试构建编不过）。
///
/// 门控 `any(test, feature = "test-support")`：本 crate 自身测试走 `test`，
/// 壳的跨层 e2e（别的 crate）走 `test-support`——跨 crate 后 `pub(crate)`
/// 够不着，见 Cargo.toml 的 feature 说明。
#[cfg(any(test, feature = "test-support"))]
pub fn transcode_h264_for_tests(req: &PlatformRequest<'_>) -> Option<AppResult<()>> {
    #[cfg(target_os = "macos")]
    {
        vt::transcode_h264(req)
    }
    #[cfg(target_os = "windows")]
    {
        mf::transcode_h264_for_tests(req)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = req;
        None
    }
}

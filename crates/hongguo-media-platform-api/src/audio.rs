//! 平台 AAC 解码统一接口。
//!
//! 实现分居 macOS（AudioToolbox AudioConverter）与 Windows（Media Foundation
//! 解码 MFT），语义一致：ASC + 声明声道/采样率建解码器，逐包解码出交错
//! f32（HE-AAC 的 SBR/PS 展开由平台解码器完成），seek 跳点 reset 冲流。
//! 具体类型由门面按编译目标 re-export
//! （`hongguo_media_platform::platform::audio::AacDecoder`）。

use hongguo_common::AppResult;

/// 逐包 AAC 解码器契约（有状态：建会话 → 逐包解码 → seek 冲流）。
pub trait AudioDecoder: Send {
    /// 解一包（MP4 样本原样字节），返回本次产出的交错 f32
    /// （解码器流水线未满时可返回空，残余帧由后续包带出）。
    fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>>;

    /// 源流不连续（seek 跳点）时冲内部状态。
    fn reset(&mut self);
}

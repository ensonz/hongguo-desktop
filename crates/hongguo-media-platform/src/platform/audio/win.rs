//! Windows 侧 AAC 解码：Media Foundation 解码 MFT 的薄封装。
//!
//! 实现在 [`crate::platform::mf::playback`]（专用线程 + 请求-应答），本层
//! 只对齐 macOS 侧的接口形态，让引擎无感切换。

use hongguo_common::AppResult;

use crate::platform::mf::AacMftDecoder;

pub struct AacDecoder {
    inner: AacMftDecoder,
}

impl AacDecoder {
    /// 与 macOS 侧同契：ASC + 声道/采样率（mp4 声明），输出交错 f32。
    pub fn open(asc: &[u8], channels: u16, sample_rate: u32) -> AppResult<Self> {
        Ok(Self {
            inner: AacMftDecoder::new(asc, channels, sample_rate)?,
        })
    }

    pub fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>> {
        self.inner.decode(packet)
    }

    /// 源流不连续（seek 跳点）时冲内部状态。
    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

//! 平台视频解码的壳无关出口：macOS = VideoToolbox（NV12 CVPixelBuffer，
//! 渲染零拷贝上屏），Windows = Media Foundation（NV12 系统内存帧，渲染侧
//! CPU 转 RGBA 后按普通图像上传）。
//!
//! 帧类型 [`VideoFrame`] 对两端的差异（像素载体）做了单一封装：macOS 带
//! `CVPixelBuffer`，Windows 带 `Nv12Plane`（Arc 共享，进帧槽/事件零复制）。
//! 解码器接口与原 VT 侧一致（open/decode/finish，帧自带 PTS），引擎
//! （hongguo-playback）与壳（hongguo-gpui）只认这一层。

#[cfg(target_os = "macos")]
use core_video::pixel_buffer::CVPixelBuffer;

use hongguo_common::AppResult;

/// 解码出的一帧：PTS（秒，显示时间）+ 像素载体。
#[derive(Clone)]
pub struct VideoFrame {
    pub pts_secs: f64,
    pub dims: (usize, usize),
    pub buffer: FrameBuffer,
}

// 跨线程搬运的约束由本声明给出：macOS 的 CVPixelBuffer 是引用计数线程安全
// 的 CF 对象（vt 侧的 DecodedFrame 同款约定），Windows 的 Arc<Vec<u8>> 天然
// 满足。引擎任务经 tokio::spawn 传帧、宿主经事件通道收帧都依赖它。
unsafe impl Send for VideoFrame {}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts_secs", &self.pts_secs)
            .field("dims", &self.dims)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub enum FrameBuffer {
    /// macOS：NV12 CVPixelBuffer（gpui surface 零拷贝；引用计数克隆）。
    #[cfg(target_os = "macos")]
    CVPixelBuffer(CVPixelBuffer),
    /// Windows：NV12 系统内存（`data` 布局 = Y 平面 `stride×height` +
    /// UV 交错平面 `stride×height/2`）。
    #[cfg(target_os = "windows")]
    Nv12(std::sync::Arc<Nv12Plane>),
}

/// Windows 的 NV12 系统内存帧。
#[cfg(target_os = "windows")]
pub struct Nv12Plane {
    pub data: std::sync::Arc<Vec<u8>>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

/// 建解码器的参数。
pub struct DecoderInit<'a> {
    /// 参数集裸 NAL（VPS/SPS/PPS）：macOS VT 的指定形态。
    pub parameter_sets: &'a [Vec<u8>],
    /// hvcC 原始载荷：Windows MF 的 `MF_MT_MPEG_SEQUENCE_HEADER` 形态。
    /// 两端各自只认自己那份，缺了对应端会走带内参数集等兜底。
    pub hvcc: Option<&'a [u8]>,
    /// 样本的 NAL 长度前缀字节数（带内参数集前插时用，1–4）。
    pub length_size: usize,
    /// 轨道声明分辨率（MF 输入类型要帧尺寸；VT 自行从参数集读）。
    pub width: usize,
    pub height: usize,
    /// 请求 FullRange NV12（VT 侧语义；MF 侧输出恒 limited，由渲染转换
    /// 端按 BT.709 limited 处理，两端观感一致）。
    pub full_range: bool,
}

/// 帧流解码器（接口对齐原 `vt::HevcFrameDecoder`）。
pub struct HevcDecoder {
    #[cfg(target_os = "macos")]
    inner: crate::platform::vt::HevcFrameDecoder,
    #[cfg(target_os = "windows")]
    inner: crate::platform::mf::HevcMftDecoder,
}

impl HevcDecoder {
    pub fn open(init: DecoderInit<'_>) -> AppResult<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                inner: crate::platform::vt::HevcFrameDecoder::open(
                    init.parameter_sets,
                    init.full_range,
                )?,
            })
        }
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                inner: crate::platform::mf::HevcMftDecoder::new(
                    init.hvcc,
                    init.parameter_sets,
                    init.length_size,
                    init.width,
                    init.height,
                )?,
            })
        }
    }

    /// 解一个样本（MP4 长度前缀 NAL 原样字节），返回本次产生的帧（解码序，
    /// 各自带 PTS；无输出返回空表）。
    pub fn decode(
        &mut self,
        sample: &[u8],
        pts_secs: f64,
        duration_secs: f64,
    ) -> AppResult<Vec<VideoFrame>> {
        #[cfg(target_os = "macos")]
        {
            let frames = self
                .inner
                .decode(sample, pts_secs, duration_secs)?
                .into_iter()
                .map(|f| VideoFrame {
                    pts_secs: f.pts_secs,
                    dims: f.dims(),
                    buffer: FrameBuffer::CVPixelBuffer(f.buffer()),
                })
                .collect();
            Ok(frames)
        }
        #[cfg(target_os = "windows")]
        {
            let frames = self
                .inner
                .decode(sample, pts_secs, duration_secs)?
                .into_iter()
                .map(|f| VideoFrame {
                    pts_secs: f.pts_secs,
                    dims: (f.width, f.height),
                    buffer: FrameBuffer::Nv12(std::sync::Arc::new(Nv12Plane {
                        data: std::sync::Arc::new(f.data),
                        stride: f.stride,
                        width: f.width,
                        height: f.height,
                    })),
                })
                .collect();
            Ok(frames)
        }
    }

    /// 冲尾帧并返回遗留输出（会话随即不再可用）。
    pub fn finish(&mut self) -> AppResult<Vec<VideoFrame>> {
        #[cfg(target_os = "macos")]
        {
            let frames = self
                .inner
                .finish()?
                .into_iter()
                .map(|f| VideoFrame {
                    pts_secs: f.pts_secs,
                    dims: f.dims(),
                    buffer: FrameBuffer::CVPixelBuffer(f.buffer()),
                })
                .collect();
            Ok(frames)
        }
        #[cfg(target_os = "windows")]
        {
            let frames = self
                .inner
                .finish()?
                .into_iter()
                .map(|f| VideoFrame {
                    pts_secs: f.pts_secs,
                    dims: (f.width, f.height),
                    buffer: FrameBuffer::Nv12(std::sync::Arc::new(Nv12Plane {
                        data: std::sync::Arc::new(f.data),
                        stride: f.stride,
                        width: f.width,
                        height: f.height,
                    })),
                })
                .collect();
            Ok(frames)
        }
    }
}

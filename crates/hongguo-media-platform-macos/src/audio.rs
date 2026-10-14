//! 平台 AAC 解码：AudioToolbox AudioConverter 主路 + symphonia 软解兜底。
//!
//! 主路走系统解码器——与 bilibili/抖音/ExoPlayer 的选择一致：一个解码器
//! 覆盖 AAC-LC 与 HE-AAC（SBR/PS）。`AudioConverter` + `DecompressionMagicCookie`
//! （ASC 从 mp4 的 esds 来），mpv 的 coreaudio AAC 解码同款。
//!
//! ⚠️ **Apple 在 Intel + macOS 15 移除了 CoreAudio 的 AAC 解码组件**：
//! `AudioConverterNew` 直接回 `'fmt?'`（FormatNotSupported）——入参完全合法
//! 也一样。兜底走 symphonia 软解（ASC 经 `CodecParameters::extra_data` 注入，
//! MP4 裸样本直喂），只解 AAC-LC，正好覆盖现行流；HE-AAC 在此类机器上
//! 不可解（报错上抛，与系统组件不存在的行为一致）。
//!
//! 输出 = 交错 f32，声道/采样率按 mp4 声明——调用方（时钟、设备流）只认
//! 声明格式，无需感知 profile 与后端。

mod ffi;

use std::os::raw::c_void;

use hongguo_common::{AppError, AppResult};

/// 一包 AAC 解出的最大帧数（LC/HE 均为 1024）。
const FRAMES_PER_PACKET: usize = 1024;

/// OSStatus 转可读错误：FourCC 可打印就按字符打（'fmt?'/'dmgc' 这类）。
fn status_msg(status: ffi::OSStatus) -> String {
    let u = status as u32;
    let bytes = u.to_be_bytes();
    if bytes.iter().all(u8::is_ascii) {
        format!("0x{u:08X}('{}')", String::from_utf8_lossy(&bytes))
    } else {
        format!("0x{u:08X}")
    }
}

/// 输入回调的取数状态。回调发生在 `AudioConverterFillComplexBuffer` 内部、
/// 与 `decode` 同一线程串行调用，无并发；用裸结构体 + 指针传递即可。
struct PendingPacket {
    data: Vec<u8>,
    taken: bool,
    channels: u32,
    desc: ffi::AudioStreamPacketDescription,
}

/// 输入数据回调：每次调用交出至多一包；包交完后回 0 包表示输入用尽。
unsafe extern "C" fn input_proc(
    _converter: ffi::AudioConverterRef,
    io_packets: *mut u32,
    io_data: *mut ffi::AudioBufferList,
    out_desc: *mut *mut ffi::AudioStreamPacketDescription,
    user: *mut c_void,
) -> ffi::OSStatus {
    unsafe {
        let pending = &mut *(user as *mut PendingPacket);
        if pending.taken || pending.data.is_empty() {
            *io_packets = 0;
            return ffi::noErr;
        }
        pending.taken = true;
        *io_packets = 1;
        // 交错给自家缓冲（头文件明示 ioData 为空时回调自备缓冲）
        let list = &mut *io_data;
        list.mNumberBuffers = 1;
        list.mBuffers[0].mNumberChannels = pending.channels;
        list.mBuffers[0].mDataByteSize = pending.data.len() as u32;
        list.mBuffers[0].mData = pending.data.as_mut_ptr() as *mut c_void;
        if !out_desc.is_null() {
            pending.desc = ffi::AudioStreamPacketDescription {
                mStartOffset: 0,
                mVariableFramesInPacket: 0,
                mDataByteCount: pending.data.len() as u32,
            };
            *out_desc = &mut pending.desc;
        }
        ffi::noErr
    }
}

/// AudioToolbox 后端（主路）。
struct AudioToolboxDecoder {
    converter: ffi::AudioConverterRef,
}

impl AudioToolboxDecoder {
    fn open(asc: &[u8], channels: u16, sample_rate: u32) -> AppResult<Self> {
        let ch = u32::from(channels);
        // 输入：原始 AAC 包（包长可变，帧数走包描述）
        let src = ffi::AudioStreamBasicDescription {
            mSampleRate: f64::from(sample_rate),
            mFormatID: ffi::kAudioFormatMPEG4AAC,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 0,
            mBytesPerFrame: 0,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        // 输出：交错 f32（Packed + Float，NonInterleaved 位不设）
        let bytes_per_frame = 4 * ch;
        let dst = ffi::AudioStreamBasicDescription {
            mSampleRate: f64::from(sample_rate),
            mFormatID: ffi::kAudioFormatLinearPCM,
            mFormatFlags: ffi::kAudioFormatFlagIsFloat | ffi::kAudioFormatFlagIsPacked,
            mBytesPerPacket: bytes_per_frame,
            mFramesPerPacket: 1,
            mBytesPerFrame: bytes_per_frame,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        let mut converter = std::ptr::null_mut();
        let st = unsafe { ffi::AudioConverterNew(&src, &dst, &mut converter) };
        if st != ffi::noErr {
            return Err(AppError::Media(format!(
                "AudioConverterNew 失败: {}（入参 ch={ch} sr={sample_rate} asc={}B 首字节 {:02x?}）",
                status_msg(st),
                asc.len(),
                asc.iter().take(6).collect::<Vec<_>>()
            )));
        }
        // ASC 就是解码侧的 magic cookie
        let st = unsafe {
            ffi::AudioConverterSetProperty(
                converter,
                ffi::kAudioConverterDecompressionMagicCookie,
                asc.len() as u32,
                asc.as_ptr() as *const c_void,
            )
        };
        if st != ffi::noErr {
            unsafe { ffi::AudioConverterDispose(converter) };
            return Err(AppError::Media(format!(
                "设置 AAC magic cookie 失败: {}",
                status_msg(st)
            )));
        }
        Ok(Self { converter })
    }

    fn decode(&mut self, packet: &[u8], channels: usize) -> AppResult<Vec<f32>> {
        if packet.is_empty() {
            return Ok(Vec::new());
        }
        let mut pending = PendingPacket {
            data: packet.to_vec(),
            taken: false,
            channels: channels as u32,
            desc: ffi::AudioStreamPacketDescription {
                mStartOffset: 0,
                mVariableFramesInPacket: 0,
                mDataByteCount: 0,
            },
        };
        // 双包容量：本包 1024 帧 + 前包残余一次带出
        let mut out = vec![0f32; FRAMES_PER_PACKET * 2 * channels];
        let mut filled = 0usize;
        loop {
            let free = out.len() - filled;
            let free_frames = free / channels;
            let mut io_packets = free_frames as u32; // PCM 输出 1 帧 = 1 包
            let data_ptr = unsafe { out.as_mut_ptr().add(filled) };
            let mut list = ffi::AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [ffi::AudioBuffer {
                    mNumberChannels: channels as u32,
                    mDataByteSize: (free * 4) as u32,
                    mData: data_ptr as *mut c_void,
                }],
            };
            let st = unsafe {
                ffi::AudioConverterFillComplexBuffer(
                    self.converter,
                    input_proc,
                    &mut pending as *mut PendingPacket as *mut c_void,
                    &mut io_packets,
                    &mut list,
                    std::ptr::null_mut(),
                )
            };
            if st != ffi::noErr {
                return Err(AppError::Media(format!("AAC 解包失败: {}", status_msg(st))));
            }
            let produced_frames = io_packets as usize;
            filled += produced_frames * channels;
            // 无进展（既没拿包也没产出）或包已交出：等下一包再继续
            if produced_frames == 0 || pending.taken {
                break;
            }
        }
        out.truncate(filled);
        Ok(out)
    }
}

impl Drop for AudioToolboxDecoder {
    fn drop(&mut self) {
        unsafe { ffi::AudioConverterDispose(self.converter) };
    }
}

// ---------------------------------------------------------------- symphonia 软解兜底

/// symphonia 软解后端（兜底）：ASC 经 `extra_data` 注入，MP4 裸样本直喂。
/// 只支持 AAC-LC（symphonia 的能力边界，见模块注释）。
struct SymphoniaDecoder {
    decoder: Box<dyn symphonia_core::codecs::Decoder>,
    track_id: u32,
    pts: u64,
    /// 每包帧数（从 ASC 解出；LC 恒 1024）。
    frames_per_packet: u64,
    sample_buf: Option<symphonia_core::audio::SampleBuffer<f32>>,
}

impl SymphoniaDecoder {
    fn open(asc: &[u8], channels: u16, sample_rate: u32) -> AppResult<Self> {
        use symphonia_core::audio::Channels;
        use symphonia_core::codecs::{CodecParameters, CodecRegistry, CODEC_TYPE_AAC};

        // ASC 里的声道数 → symphonia 声道位标志（现行流恒单/双声道）。
        let channels_flag = match channels {
            1 => Channels::FRONT_LEFT,
            2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            _ => {
                return Err(AppError::Media(format!(
                    "symphonia 兜底不支持 {channels} 声道"
                )))
            }
        };
        let asc_owned: Box<[u8]> = asc.into();
        let mut params = CodecParameters::new();
        params
            .for_codec(CODEC_TYPE_AAC)
            .with_sample_rate(sample_rate)
            .with_channels(channels_flag)
            .with_extra_data(asc_owned);

        let mut registry = CodecRegistry::new();
        registry.register_all::<symphonia_codec_aac::AacDecoder>();
        let decoder = registry
            .make(&params, &symphonia_core::codecs::DecoderOptions::default())
            .map_err(|e| AppError::Media(format!("symphonia AAC 解码器建立失败: {e}")))?;

        Ok(Self {
            decoder,
            track_id: 1,
            pts: 0,
            frames_per_packet: 1024,
            sample_buf: None,
        })
    }

    fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>> {
        use symphonia_core::audio::SampleBuffer;
        use symphonia_core::formats::Packet;

        let pkt = Packet::new_from_slice(
            self.track_id,
            self.pts,
            self.frames_per_packet,
            packet,
        );
        self.pts += self.frames_per_packet;
        let buf_ref = self
            .decoder
            .decode(&pkt)
            .map_err(|e| AppError::Media(format!("symphonia AAC 解码失败: {e}")))?;
        // symphonia 产出 planar/原生格式，统一转交错 f32
        let spec = *buf_ref.spec();
        let cap = buf_ref.capacity() as u64;
        let sample_buf =
            self.sample_buf.get_or_insert_with(|| SampleBuffer::<f32>::new(cap, spec));
        sample_buf.copy_interleaved_ref(buf_ref);
        Ok(sample_buf.samples().to_vec())
    }

    fn reset(&mut self) {
        // symphonia 无 in-place reset：解码器带状态重建（seek 场景本来就该丢）
        self.decoder.reset();
    }
}

// ---------------------------------------------------------------- 双后端门面

/// AAC 逐包解码器。一次 `decode` 喂一包（MP4 样本原样字节），返回本次产出
/// 的交错 f32（内部流水线未满时可返回空；残余帧由后续包带出）。
pub struct AacDecoder {
    backend: AacBackend,
    channels: usize,
    sample_rate: u32,
}

enum AacBackend {
    AudioToolbox(AudioToolboxDecoder),
    Symphonia(SymphoniaDecoder),
}

// AudioConverter 对象可跨线程搬移（Apple 文档：非重入但线程间传递安全），
// symphonia 解码器为纯 Rust 数据。使用纪律（始终单任务串行调用）由调用方保证。
unsafe impl Send for AacDecoder {}

impl AacDecoder {
    /// 用 mp4 声明的声道/采样率与 esds 里的 AudioSpecificConfig 建解码器。
    /// 输出格式 = 交错 f32 × 声明声道 × 声明采样率。
    ///
    /// 主路 AudioToolbox（HE-AAC 也覆盖）；这台机器的系统 AAC 组件不可用
    /// （Intel + macOS 15，见模块注释）时回退 symphonia 软解（仅 AAC-LC）。
    pub fn open(asc: &[u8], channels: u16, sample_rate: u32) -> AppResult<Self> {
        if asc.is_empty() || channels == 0 || sample_rate == 0 {
            return Err(AppError::Media(format!(
                "AAC 参数不全（asc={}B ch={channels} sr={sample_rate}）",
                asc.len()
            )));
        }
        match AudioToolboxDecoder::open(asc, channels, sample_rate) {
            Ok(at) => Ok(Self {
                backend: AacBackend::AudioToolbox(at),
                channels: usize::from(channels),
                sample_rate,
            }),
            Err(at_err) => match SymphoniaDecoder::open(asc, channels, sample_rate) {
                Ok(sp) => {
                    log::warn!(
                        "[Platform/audio] AudioToolbox AAC 不可用（{at_err}），回退 symphonia 软解（仅 LC）"
                    );
                    Ok(Self {
                        backend: AacBackend::Symphonia(sp),
                        channels: usize::from(channels),
                        sample_rate,
                    })
                }
                Err(sp_err) => Err(AppError::Media(format!(
                    "AAC 解码器两个后端都失败：主路 {at_err}；软解 {sp_err}"
                ))),
            },
        }
    }

    /// 解一包。包解不出来返回 `Err`（调用方决定补静音等策略），正常返回
    /// 本包带出的样本（可为空——解码器流水线未满）。
    pub fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>> {
        match &mut self.backend {
            AacBackend::AudioToolbox(at) => at.decode(packet, self.channels),
            AacBackend::Symphonia(sp) => sp.decode(packet),
        }
    }

    /// 源流不连续（seek 跳点）时冲内部状态。
    pub fn reset(&mut self) {
        match &mut self.backend {
            AacBackend::AudioToolbox(at) => {
                let st = unsafe { ffi::AudioConverterReset(at.converter) };
                if st != ffi::noErr {
                    log::warn!("[Platform/audio] AudioConverterReset: {}", status_msg(st));
                }
            }
            AacBackend::Symphonia(sp) => sp.reset(),
        }
    }

    /// 声明采样率（诊断日志用）。
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl hongguo_media_platform_api::AudioDecoder for AacDecoder {
    fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>> {
        AacDecoder::decode(self, packet)
    }

    fn reset(&mut self) {
        AacDecoder::reset(self);
    }
}

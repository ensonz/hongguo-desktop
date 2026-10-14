//! 平台 AAC 解码（AudioToolbox AudioConverter）。
//!
//! 播放引擎的声音轨走系统解码器——与 bilibili/抖音/ExoPlayer 的选择一致：
//! 一个解码器覆盖 AAC-LC 与 HE-AAC（SBR/PS），纯 Rust 解码器只有 LC。
//! 路线：`AudioConverter` + `DecompressionMagicCookie`（ASC 从 mp4 的 esds 来），
//! mpv 的 coreaudio AAC 解码同款。
//!
//! 输出 = 交错 f32，声道/采样率按 mp4 声明——HE-AAC 的上变频/参数立体声
//! 展开由解码器完成，调用方（时钟、设备流）只认声明格式，无需感知 profile。

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

/// AAC 逐包解码器。一次 `decode` 喂一包（MP4 样本原样字节），返回本次产出
/// 的交错 f32（内部流水线未满时可返回空；残余帧由后续包带出）。
pub struct AacDecoder {
    converter: ffi::AudioConverterRef,
    channels: usize,
    sample_rate: u32,
}

// AudioConverter 对象可跨线程搬移（Apple 文档：非重入但线程间传递安全），
// 使用纪律（始终单任务串行调用）由调用方保证。
unsafe impl Send for AacDecoder {}

impl AacDecoder {
    /// 用 mp4 声明的声道/采样率与 esds 里的 AudioSpecificConfig 建解码器。
    /// 输出格式 = 交错 f32 × 声明声道 × 声明采样率。
    pub fn open(asc: &[u8], channels: u16, sample_rate: u32) -> AppResult<Self> {
        if asc.is_empty() || channels == 0 || sample_rate == 0 {
            return Err(AppError::Media(format!(
                "AAC 参数不全（asc={}B ch={channels} sr={sample_rate}）",
                asc.len()
            )));
        }
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
                "AudioConverterNew 失败: {}（入参 ch={ch} sr={} asc={}B 首字节 {:02x?}）",
                status_msg(st),
                sample_rate,
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
        Ok(Self {
            converter,
            channels: usize::from(channels),
            sample_rate,
        })
    }

    /// 解一包。包解不出来返回 `Err`（调用方决定补静音等策略），正常返回
    /// 本包带出的样本（可为空——解码器流水线未满）。
    pub fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>> {
        if packet.is_empty() {
            return Ok(Vec::new());
        }
        let mut pending = PendingPacket {
            data: packet.to_vec(),
            taken: false,
            channels: self.channels as u32,
            desc: ffi::AudioStreamPacketDescription {
                mStartOffset: 0,
                mVariableFramesInPacket: 0,
                mDataByteCount: 0,
            },
        };
        // 双包容量：本包 1024 帧 + 前包残余一次带出
        let mut out = vec![0f32; FRAMES_PER_PACKET * 2 * self.channels];
        let mut filled = 0usize;
        loop {
            let free = out.len() - filled;
            let free_frames = free / self.channels;
            let mut io_packets = free_frames as u32; // PCM 输出 1 帧 = 1 包
            let data_ptr = unsafe { out.as_mut_ptr().add(filled) };
            let mut list = ffi::AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [ffi::AudioBuffer {
                    mNumberChannels: self.channels as u32,
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
            filled += produced_frames * self.channels;
            // 无进展（既没拿包也没产出）或包已交出：等下一包再继续
            if produced_frames == 0 || pending.taken {
                break;
            }
        }
        out.truncate(filled);
        Ok(out)
    }

    /// 源流不连续（seek 跳点）时冲内部状态。
    pub fn reset(&mut self) {
        let st = unsafe { ffi::AudioConverterReset(self.converter) };
        if st != ffi::noErr {
            log::warn!("[Platform/audio] AudioConverterReset: {}", status_msg(st));
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

impl Drop for AacDecoder {
    fn drop(&mut self) {
        unsafe { ffi::AudioConverterDispose(self.converter) };
    }
}

//! AudioToolbox / CoreAudioTypes 的最小 FFI 声明（AAC 解码用）。
//!
//! 只声明解码链路用到的符号。字段/签名/常量以本机 SDK 头文件为准
//! （AudioToolbox.framework/Headers/AudioConverter.h、
//! CoreAudioTypes.framework/Headers/CoreAudioBaseTypes.h）。
//! `ASBD` 等结构在现行 SDK 里已无 `#pragma pack`，自然对齐即可。
#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code
)]

use std::os::raw::c_void;

pub type OSStatus = i32;
pub type AudioConverterRef = *mut c_void;

/// `AudioStreamBasicDescription`（CoreAudioBaseTypes.h）。f64 打头自然对齐。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: u32,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [AudioBuffer; 1],
}

/// 压缩包的描述（可变包长时输入回调必须逐包给出）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioStreamPacketDescription {
    pub mStartOffset: i64,
    pub mVariableFramesInPacket: u32,
    pub mDataByteCount: u32,
}

/// 输入数据回调：`*ioNumberDataPackets` 入参是期望包数，出参是实际提供数；
/// 返回 0 包即「输入用尽」。`outDataPacketDescription` 非空时须指向本包描述。
pub type AudioConverterComplexInputDataProc = unsafe extern "C" fn(
    inAudioConverter: AudioConverterRef,
    ioNumberDataPackets: *mut u32,
    ioData: *mut AudioBufferList,
    outDataPacketDescription: *mut *mut AudioStreamPacketDescription,
    inUserData: *mut c_void,
) -> OSStatus;

// —— 常量（FourCC 直取自头文件）——
pub const noErr: OSStatus = 0;
/// `kAudioFormatLinearPCM`（'lpcm'）
pub const kAudioFormatLinearPCM: u32 = 0x6C70_636D;
/// `kAudioFormatMPEG4AAC`（'aac '，含 LC/HE 各 profile，具体由 magic cookie 决定）
pub const kAudioFormatMPEG4AAC: u32 = 0x6163_6320;
/// `kAudioFormatFlagIsFloat`
pub const kAudioFormatFlagIsFloat: u32 = 1 << 0;
/// `kAudioFormatFlagIsPacked`（交错；NonInterleaved 位不设即交错）
pub const kAudioFormatFlagIsPacked: u32 = 1 << 3;
/// `kAudioConverterDecompressionMagicCookie`（'dmgc'）——解码器的
/// AudioSpecificConfig 走这里传入
pub const kAudioConverterDecompressionMagicCookie: u32 = 0x646D_6763;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    pub fn AudioConverterNew(
        inSourceFormat: *const AudioStreamBasicDescription,
        inDestinationFormat: *const AudioStreamBasicDescription,
        outAudioConverter: *mut AudioConverterRef,
    ) -> OSStatus;
    pub fn AudioConverterSetProperty(
        inAudioConverter: AudioConverterRef,
        inPropertyID: u32,
        inPropertyDataSize: u32,
        inPropertyData: *const c_void,
    ) -> OSStatus;
    pub fn AudioConverterFillComplexBuffer(
        inAudioConverter: AudioConverterRef,
        inInputDataProc: AudioConverterComplexInputDataProc,
        inInputDataProcUserData: *mut c_void,
        ioOutputDataPacketSize: *mut u32,
        outOutputData: *mut AudioBufferList,
        outPacketDescription: *mut *mut AudioStreamPacketDescription,
    ) -> OSStatus;
    /// 源流不连续（seek）时冲内部缓冲。
    pub fn AudioConverterReset(inAudioConverter: AudioConverterRef) -> OSStatus;
    pub fn AudioConverterDispose(inAudioConverter: AudioConverterRef) -> OSStatus;
}

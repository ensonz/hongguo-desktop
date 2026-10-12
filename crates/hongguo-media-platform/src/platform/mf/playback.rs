//! 播放专用解码 MFT：HEVC → NV12 系统内存帧、AAC → 交错 f32。
//!
//! 转码链（[`super`]）的解码走 SourceReader 文件流；播放没有文件——样本经
//! serve 逐段进内存，这里**直接驱动解码 MFT**（复用 [`super`] 的 Driver 统一
//! 驱动、MFT 枚举/激活与样本工具）。每个解码器一条常驻线程：COM/MTA 状态
//! 不落在 tokio 工作线程上，请求-应答式阻塞调用与 macOS 侧 VT 的异步回调
//! 等价（调用方都在引擎的解码循环里串行推进）。

use std::sync::mpsc;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use hongguo_common::{AppError, AppResult};

use super::{
    Driver, MF_VERSION, SESSION_INIT, enum_activates, frame_size_of, hr, sample_to_bytes,
    secs_to_hns, type_info, type_stride, video_type,
};

/// 解码线程的一次会话初始化（进程级一次，永不拆卸——理由见
/// [`super::SESSION_INIT`] 的注释）。
fn session_init_once() {
    SESSION_INIT.call_once(|| {
        // RPC_E_CHANGED_MODE：沿用进程既有 COM 模式即可
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if let Err(e) = unsafe { MFStartup(MF_VERSION, 0) } {
            log::error!("[Platform/mf] MFStartup 失败: {e}");
        }
    });
}

/// 喂入字节数据 → IMFSample（压缩样本共用的构造：锁缓冲、拷贝、定时戳）。
/// 流程对齐 super 的 nv12_sample，只是源是裸字节不是 NV12 平面。
unsafe fn bytes_sample(data: &[u8], pts_100ns: i64, dur_100ns: i64) -> AppResult<IMFSample> {
    unsafe {
        let sample = hr(MFCreateSample(), "建样本")?;
        let buf = hr(MFCreateMemoryBuffer(data.len() as u32), "建样本缓冲")?;
        let mut ptr = std::ptr::null_mut();
        hr(buf.Lock(&mut ptr, None, None), "锁样本缓冲")?;
        let dst = std::slice::from_raw_parts_mut(ptr, data.len());
        dst.copy_from_slice(data);
        let _ = buf.Unlock();
        hr(buf.SetCurrentLength(data.len() as u32), "设样本长度")?;
        hr(sample.AddBuffer(&buf), "挂样本缓冲")?;
        if pts_100ns >= 0 {
            hr(sample.SetSampleTime(pts_100ns), "设样本时间")?;
        }
        if dur_100ns >= 0 {
            hr(sample.SetSampleDuration(dur_100ns), "设样本时长")?;
        }
        Ok(sample)
    }
}

/// 一个解码出的 NV12 帧（系统内存，带字节行距）。
pub(crate) struct Nv12Out {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    pub pts_secs: f64,
}

/// 解码线程的请求/应答（视频与音频共用形态，各自只用到相关变体）。
enum Req {
    /// 喂一个样本，收本次带出的输出。
    Decode {
        sample: Vec<u8>,
        pts: f64,
        dur: f64,
    },
    /// 冲尾（END_OF_STREAM + DRAIN），视频收尾用。
    Finish,
    /// 冲流不收尾（音频 seek 用：丢内部缓冲，继续接新位置的包）。
    Flush,
    Shutdown,
}

enum Rsp {
    Frames(Vec<Nv12Out>),
    Samples(Vec<f32>),
    Done,
    Err(AppError),
}

/// 会话失败后的兜底应答：给每个未决请求回同一个错误（转成 String 再造，
/// AppError 不 Clone），调用方不至于在 recv 上挂死。
fn replay_error(rx: &mpsc::Receiver<Req>, tx: &mpsc::Sender<Rsp>, msg: String) {
    while let Ok(req) = rx.recv() {
        match req {
            Req::Shutdown => break,
            _ => {
                let _ = tx.send(Rsp::Err(AppError::Media(msg.clone())));
            }
        }
    }
}

// ————————————————————————————————————————————————————————————
// HEVC 视频
// ————————————————————————————————————————————————————————————

/// 播放用 HEVC 解码器：专用线程持有 MFT，请求-应答式驱动。
pub(crate) struct HevcMftDecoder {
    tx: mpsc::Sender<Req>,
    rx: mpsc::Receiver<Rsp>,
}

impl HevcMftDecoder {
    /// `hvcc` 是 hvcC 原始载荷（`MF_MT_MPEG_SEQUENCE_HEADER` 的指定形态）；
    /// `parameter_sets`/`length_size` 用来给首样本前插带内参数集——个别解码
    /// 器拒收 hvcC blob 时，带内参数集是第二道保险。
    pub(crate) fn new(
        hvcc: Option<&[u8]>,
        parameter_sets: &[Vec<u8>],
        length_size: usize,
        width: usize,
        height: usize,
    ) -> AppResult<Self> {
        let (tx_req, rx_req) = mpsc::channel::<Req>();
        let (tx_rsp, rx_rsp) = mpsc::channel::<Rsp>();
        let hvcc = hvcc.map(<[u8]>::to_vec);
        let parameter_sets = parameter_sets.to_vec();
        let spawned = std::thread::Builder::new()
            .name("hg-mf-hevc".into())
            .spawn(move || {
                session_init_once();
                if let Err(e) = video_session(
                    &hvcc,
                    &parameter_sets,
                    length_size,
                    width,
                    height,
                    &rx_req,
                    &tx_rsp,
                ) {
                    replay_error(&rx_req, &tx_rsp, e.to_string());
                }
            });
        if let Err(e) = spawned {
            return Err(AppError::Media(format!("HEVC 解码线程起不来: {e}")));
        }
        Ok(Self {
            tx: tx_req,
            rx: rx_rsp,
        })
    }

    fn call(&mut self, req: Req) -> AppResult<Vec<Nv12Out>> {
        self.tx
            .send(req)
            .map_err(|_| AppError::Media("HEVC 解码线程已退出".into()))?;
        match self.rx.recv() {
            Ok(Rsp::Frames(frames)) => Ok(frames),
            Ok(Rsp::Err(e)) => Err(e),
            Ok(_) => Ok(Vec::new()),
            Err(_) => Err(AppError::Media("HEVC 解码线程应答缺失".into())),
        }
    }

    pub(crate) fn decode(
        &mut self,
        sample: &[u8],
        pts_secs: f64,
        dur_secs: f64,
    ) -> AppResult<Vec<Nv12Out>> {
        self.call(Req::Decode {
            sample: sample.to_vec(),
            pts: pts_secs,
            dur: dur_secs,
        })
    }

    pub(crate) fn finish(&mut self) -> AppResult<Vec<Nv12Out>> {
        self.call(Req::Finish)
    }
}

impl Drop for HevcMftDecoder {
    fn drop(&mut self) {
        let _ = self.tx.send(Req::Shutdown);
    }
}

/// 视频会话主体：建 MFT、协商类型、循环应答。返回 Err = 建会话失败
/// （由调用侧回放给所有未决请求）。
fn video_session(
    hvcc: &Option<Vec<u8>>,
    parameter_sets: &[Vec<u8>],
    length_size: usize,
    width: usize,
    height: usize,
    rx: &mpsc::Receiver<Req>,
    tx: &mpsc::Sender<Rsp>,
) -> AppResult<()> {
    unsafe {
        let decoders = enum_activates(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT
                | MFT_ENUM_FLAG_ASYNCMFT
                | MFT_ENUM_FLAG_HARDWARE
                | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&type_info(MFMediaType_Video, MFVideoFormat_HEVC)),
            Some(&type_info(MFMediaType_Video, MFVideoFormat_NV12)),
        )?;
        let Some(activate) = decoders.first() else {
            return Err(AppError::Media(
                "没有 HEVC 解码 MFT（装系统的「HEVC 视频扩展」后重试）".into(),
            ));
        };
        let (mut driver, xform) = Driver::new(activate)?;

        // 输入类型：HEVC + 帧尺寸 + hvcC 序列头（被拒时去 blob 重试，
        // 带内参数集兜底——见首样本前插）
        let in_t = video_type(MFVideoFormat_HEVC)?;
        let frame_size = (width as u64) << 32 | height as u64;
        hr(
            in_t.SetUINT64(&MF_MT_FRAME_SIZE, frame_size),
            "设解码帧尺寸",
        )?;
        if let Some(hvcc) = hvcc {
            hr(
                in_t.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, hvcc),
                "设 hvcC 序列头",
            )?;
        }
        if let Err(e) = xform.SetInputType(0, &in_t, 0) {
            if hvcc.is_some() {
                log::warn!("[Platform/mf] hvcC 序列头被拒（{e}），改走带内参数集");
                let bare = video_type(MFVideoFormat_HEVC)?;
                hr(
                    bare.SetUINT64(&MF_MT_FRAME_SIZE, frame_size),
                    "设解码帧尺寸",
                )?;
                hr(xform.SetInputType(0, &bare, 0), "设解码输入类型")?;
            } else {
                return Err(AppError::Media(format!(
                    "Media Foundation 设解码输入类型失败（{e}）"
                )));
            }
        }
        let out_t = video_type(MFVideoFormat_NV12)?;
        hr(xform.SetOutputType(0, &out_t, 0), "设 NV12 解码输出")?;
        let cur = hr(xform.GetOutputCurrentType(0), "读解码输出类型")?;
        let (mut w, mut h) = frame_size_of(&cur)?;
        let mut stride = type_stride(&cur, w);
        driver.begin_streaming()?;
        log::info!("[Platform/mf] HEVC 解码会话就绪（{w}x{h}，stride {stride}）");

        // 首样本前插带内参数集（MP4 样本自身不带 VPS/SPS/PPS）；前缀长度
        // 按 hvcC 声明的 length_size 构造（1–4 字节）
        let mut first_sample = true;
        let mut inband_params = Vec::new();
        for nalu in parameter_sets {
            let len = (nalu.len() as u32).to_be_bytes();
            let skip = 4 - length_size.clamp(1, 4);
            inband_params.extend_from_slice(&len[skip..]);
            inband_params.extend_from_slice(nalu);
        }

        while let Ok(req) = rx.recv() {
            match req {
                Req::Shutdown => return Ok(()),
                Req::Flush => {
                    // 视频侧不走冲流（引擎 seek 重建会话），收到当空应答
                    let _ = xform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
                    let _ = tx.send(Rsp::Frames(Vec::new()));
                }
                Req::Finish => {
                    if let Err(e) = driver.finish() {
                        let _ = tx.send(Rsp::Err(e));
                        continue;
                    }
                    let mut frames = Vec::new();
                    for s in driver.take() {
                        if let Ok(f) = nv12_of(&s, stride, w, h) {
                            frames.push(f);
                        }
                    }
                    let _ = tx.send(Rsp::Frames(frames));
                }
                Req::Decode { sample, pts, dur } => {
                    let mut data = Vec::with_capacity(inband_params.len() + sample.len());
                    if first_sample {
                        data.extend_from_slice(&inband_params);
                        first_sample = false;
                    }
                    data.extend_from_slice(&sample);
                    let input = match bytes_sample(&data, secs_to_hns(pts), secs_to_hns(dur)) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.send(Rsp::Err(e));
                            continue;
                        }
                    };
                    if let Err(e) = driver.push(input) {
                        let _ = tx.send(Rsp::Err(e));
                        continue;
                    }
                    if driver.stream_changed {
                        // 输出类型重谈（分辨率/行距变化）：再设一次 NV12 后刷新
                        let out_t = video_type(MFVideoFormat_NV12)?;
                        if let Err(e) = xform.SetOutputType(0, &out_t, 0) {
                            let _ = tx.send(Rsp::Err(AppError::Media(format!(
                                "Media Foundation 重设输出类型失败（{e}）"
                            ))));
                            continue;
                        }
                        if let Ok(cur) = xform.GetOutputCurrentType(0)
                            && let Ok((nw, nh)) = frame_size_of(&cur)
                        {
                            w = nw;
                            h = nh;
                            stride = type_stride(&cur, w);
                        }
                        driver.stream_changed = false;
                        let _ = driver.harvest();
                    }
                    let mut frames = Vec::new();
                    for s in driver.take() {
                        match nv12_of(&s, stride, w, h) {
                            Ok(f) => frames.push(f),
                            Err(e) => log::warn!("[Platform/mf] 丢一帧无法解析的输出: {e}"),
                        }
                    }
                    let _ = tx.send(Rsp::Frames(frames));
                }
            }
        }
        Ok(())
    }
}

/// 输出样本 → NV12 帧。PTS 读样本自带的 100ns 时间戳（喂进去的媒体时间）；
/// 缺失（-1）按 0 处理——引擎侧重排缓冲只认我们自己喂的 pts 序。
unsafe fn nv12_of(s: &IMFSample, stride: usize, width: usize, height: usize) -> AppResult<Nv12Out> {
    let (data, t, _) = sample_to_bytes(s)?;
    let need = stride * height * 3 / 2;
    if data.len() < need {
        return Err(AppError::Media(format!(
            "NV12 输出帧字节不足（{} < 需要 {need}）",
            data.len()
        )));
    }
    Ok(Nv12Out {
        data,
        stride,
        width,
        height,
        pts_secs: if t >= 0 { t as f64 / 10_000_000.0 } else { 0.0 },
    })
}

// ————————————————————————————————————————————————————————————
// AAC 音频
// ————————————————————————————————————————————————————————————

/// 播放用 AAC 解码器：in-box 软件 MFT（同步），输出交错 f32。
pub(crate) struct AacMftDecoder {
    tx: mpsc::Sender<Req>,
    rx: mpsc::Receiver<Rsp>,
}

impl AacMftDecoder {
    pub(crate) fn new(asc: &[u8], channels: u16, sample_rate: u32) -> AppResult<Self> {
        let (tx_req, rx_req) = mpsc::channel::<Req>();
        let (tx_rsp, rx_rsp) = mpsc::channel::<Rsp>();
        let asc = asc.to_vec();
        let spawned = std::thread::Builder::new()
            .name("hg-mf-aac".into())
            .spawn(move || {
                session_init_once();
                if let Err(e) = audio_session(&asc, channels, sample_rate, &rx_req, &tx_rsp) {
                    replay_error(&rx_req, &tx_rsp, e.to_string());
                }
            });
        if let Err(e) = spawned {
            return Err(AppError::Media(format!("AAC 解码线程起不来: {e}")));
        }
        Ok(Self {
            tx: tx_req,
            rx: rx_rsp,
        })
    }

    fn call(&mut self, req: Req) -> AppResult<Vec<f32>> {
        self.tx
            .send(req)
            .map_err(|_| AppError::Media("AAC 解码线程已退出".into()))?;
        match self.rx.recv() {
            Ok(Rsp::Samples(s)) => Ok(s),
            Ok(Rsp::Err(e)) => Err(e),
            Ok(_) => Ok(Vec::new()),
            Err(_) => Err(AppError::Media("AAC 解码线程应答缺失".into())),
        }
    }

    pub(crate) fn decode(&mut self, packet: &[u8]) -> AppResult<Vec<f32>> {
        self.call(Req::Decode {
            sample: packet.to_vec(),
            pts: -1.0,
            dur: -1.0,
        })
    }

    /// seek 冲流：丢掉 MFT 内部缓冲（时间轴换轨），继续接新位置的包。
    pub(crate) fn reset(&mut self) {
        let _ = self.call(Req::Flush);
    }
}

impl Drop for AacMftDecoder {
    fn drop(&mut self) {
        let _ = self.tx.send(Req::Shutdown);
    }
}

fn audio_session(
    asc: &[u8],
    channels: u16,
    sample_rate: u32,
    rx: &mpsc::Receiver<Req>,
    tx: &mpsc::Sender<Rsp>,
) -> AppResult<()> {
    unsafe {
        let decoders = enum_activates(
            MFT_CATEGORY_AUDIO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&type_info(MFMediaType_Audio, MFAudioFormat_AAC)),
            Some(&type_info(MFMediaType_Audio, MFAudioFormat_Float)),
        )?;
        let Some(activate) = decoders.first() else {
            return Err(AppError::Media("没有 AAC 解码 MFT".into()));
        };
        let (mut driver, xform) = Driver::new(activate)?;

        // 输入：裸 AAC 包（PAYLOAD_TYPE=0，非 ADTS），ASC 走 MF_MT_USER_DATA
        let in_t = hr(MFCreateMediaType(), "建音频输入类型")?;
        hr(
            in_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio),
            "设音频 major type",
        )?;
        hr(
            in_t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC),
            "设 AAC subtype",
        )?;
        hr(
            in_t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate),
            "设采样率",
        )?;
        hr(
            in_t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32),
            "设声道数",
        )?;
        // 默认值随系统版本有出入，显式钉死 raw 形态
        if let Err(e) = in_t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0) {
            log::debug!("[Platform/mf] 设 AAC payload type 被拒（按默认）: {e}");
        }
        if let Err(e) = in_t.SetBlob(&MF_MT_USER_DATA, asc) {
            log::debug!("[Platform/mf] 设 ASC 被拒（按默认）: {e}");
        }
        hr(xform.SetInputType(0, &in_t, 0), "设音频输入类型")?;

        let out_t = hr(MFCreateMediaType(), "建音频输出类型")?;
        hr(
            out_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio),
            "设音频 major type",
        )?;
        hr(
            out_t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float),
            "设 f32 subtype",
        )?;
        hr(
            out_t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate),
            "设输出采样率",
        )?;
        hr(
            out_t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32),
            "设输出声道数",
        )?;
        hr(xform.SetOutputType(0, &out_t, 0), "设音频输出类型")?;
        driver.begin_streaming()?;

        while let Ok(req) = rx.recv() {
            match req {
                Req::Shutdown => return Ok(()),
                Req::Flush => {
                    let _ = xform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
                    let _ = tx.send(Rsp::Samples(Vec::new()));
                }
                Req::Finish => {
                    let _ = tx.send(Rsp::Done);
                }
                Req::Decode { sample, .. } => {
                    let input = match bytes_sample(&sample, -1, -1) {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.send(Rsp::Err(e));
                            continue;
                        }
                    };
                    if let Err(e) = driver.push(input) {
                        let _ = tx.send(Rsp::Err(e));
                        continue;
                    }
                    let mut pcm = Vec::new();
                    for s in driver.take() {
                        match sample_to_bytes(&s) {
                            Ok((bytes, _, _)) => pcm.extend(
                                bytes
                                    .chunks_exact(4)
                                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                            ),
                            Err(e) => log::warn!("[Platform/mf] 丢一段音频输出: {e}"),
                        }
                    }
                    let _ = tx.send(Rsp::Samples(pcm));
                }
            }
        }
        Ok(())
    }
}

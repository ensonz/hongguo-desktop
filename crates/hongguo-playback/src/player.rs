//! 引擎编排与对外句柄。
//!
//! [`open`] 做完所有贵的前置（读样本表、开解码器、起音频设备）才返回句柄；
//! 随后的播放由两个任务自治：音频喂送（[`audio_feeder`]）与视频循环
//! （[`video::run`]）。宿主只拿 [`PlayerHandle`]：取帧、暂停/继续、读进度、
//! 等事件。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use hongguo_mp4::sample_table::TrackInfo;

use crate::audio::{AudioOut, Clock, RingFeed};
use crate::source::{SampleSource, open_stream};
use crate::video;

/// 引擎事件（经调用方给的通道送达；通道关闭 = 宿主已弃，引擎自停）。
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// 首帧已进上屏槽（宿主据此收封面占位、触发下一部预取）。
    FirstFrame { width: usize, height: usize },
    /// 有新帧可取（[`PlayerHandle::latest_frame`]）。
    Frame,
    /// 自然播完。
    Ended,
    /// 致命失败（引擎已停）。
    Failed(String),
}

/// 打开参数。
pub struct OpenOptions {
    /// 流的 vid（调用方已 `prepare`，缓存里有这一档）。
    pub vid: String,
    pub definition: u32,
    /// 起播媒体时间（秒）：续播位置。0 = 从头。
    pub start_secs: f64,
    /// 事件通道。
    pub events: mpsc::Sender<PlayerEvent>,
}

/// 内部共享态：任务族与句柄的共同视图。
pub(crate) struct Shared {
    pub paused: AtomicBool,
    pub shutdown: AtomicBool,
    pub clock: Arc<Clock>,
    pub latest: parking_lot::Mutex<Option<hongguo_media_platform::platform::vt::DecodedFrame>>,
    pub dims: parking_lot::Mutex<(usize, usize)>,
    pub duration_secs: parking_lot::Mutex<f64>,
    pub events: mpsc::Sender<PlayerEvent>,
}

impl Shared {
    /// 发事件。宿主弃了（通道关闭）返回 Err，调用方应顺势退出。
    pub(crate) async fn emit(&self, event: PlayerEvent) {
        let _ = self.events.send(event).await;
    }

    /// 致命失败：通知宿主并自停。
    pub(crate) async fn fail(&self, msg: String) {
        log::warn!("[Playback] {msg}");
        self.shutdown.store(true, Ordering::Relaxed);
        self.emit(PlayerEvent::Failed(msg)).await;
    }
}

/// 引擎句柄。Drop 即停（任务中止、音频设备释放）。
pub struct PlayerHandle {
    shared: Arc<Shared>,
    audio: Option<AudioOut>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl PlayerHandle {
    /// 当前上屏帧（NV12 CVPixelBuffer，引用计数克隆）。渲染线程调用。
    pub fn latest_frame(&self) -> Option<hongguo_media_platform::platform::vt::DecodedFrame> {
        self.shared.latest.lock().clone()
    }

    /// 视频分辨率。
    pub fn dims(&self) -> (usize, usize) {
        *self.shared.dims.lock()
    }

    /// 时长（秒）。
    pub fn duration_secs(&self) -> f64 {
        *self.shared.duration_secs.lock()
    }

    /// 当前播放位置（秒，主时钟）。
    pub fn position_secs(&self) -> f64 {
        self.shared.clock.now()
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// 暂停：音频设备停消费（时钟冻结），视频循环停在当前帧。
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
        if let Some(audio) = &self.audio {
            audio.set_paused(true);
        }
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        if let Some(audio) = &self.audio {
            audio.set_paused(false);
        }
    }
}

impl Drop for PlayerHandle {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(audio) = &self.audio {
            audio.set_paused(true);
        }
    }
}

/// 打开并起播。必须在 tokio 运行时上下文内 await（引擎任务经 `tokio::spawn`
/// 挂在调用方的运行时上）。
pub async fn open(opts: OpenOptions) -> hongguo_common::AppResult<PlayerHandle> {
    let meta = open_stream(&opts.vid, opts.definition).await?;
    let video_info = meta.video;

    // 时长 = 视/音两轨谁的尾巴长
    let video_end = media_end(&video_info);
    let audio_end = meta.audio.as_ref().map(media_end).unwrap_or(0.0);
    let duration = video_end.max(audio_end);

    // 音频设备：建不出来就挂钟（有画面没声音仍可看，不该整场失败）
    let (audio_out, feed): (Option<AudioOut>, Option<RingFeed>) =
        match meta.audio.as_ref().map(|a| (a.channels, a.sample_rate)) {
            Some((ch, sr)) if ch > 0 && sr > 0 => match AudioOut::open(ch, sr) {
                Some((out, feed)) => (Some(out), Some(feed)),
                None => {
                    log::warn!("[Playback] 音频设备不可用，退化为无声播放（挂钟）");
                    (None, None)
                }
            },
            _ => (None, None),
        };
    let clock = match &audio_out {
        Some(out) => Arc::clone(&out.clock_src),
        None => Arc::new(Clock::wall()),
    };
    clock.reset(opts.start_secs);

    let shared = Arc::new(Shared {
        paused: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        clock: Arc::clone(&clock),
        latest: parking_lot::Mutex::new(None),
        dims: parking_lot::Mutex::new((
            video_info.width as usize,
            video_info.height as usize,
        )),
        duration_secs: parking_lot::Mutex::new(duration),
        events: opts.events,
    });

    let mut tasks = Vec::new();

    // 音频喂送：解 AAC → 交错 f32 → 环形缓冲（设备消费速率即节拍）
    if let (Some(info), Some(feed)) = (meta.audio.clone(), feed) {
        let shared = Arc::clone(&shared);
        let source = SampleSource::new(&opts.vid, opts.definition, Arc::new(info.samples.clone()));
        tasks.push(tokio::spawn(async move {
            audio_feeder(shared, feed, source, info, opts.start_secs).await;
        }));
    } else if meta.audio.is_some() {
        log::warn!("[Playback] 有音轨但设备不可用：无声播放");
    }

    // 视频循环：VT 解码 → 帧槽，节奏对主时钟
    {
        let shared = Arc::clone(&shared);
        let info = video_info.clone();
        let parameter_sets = meta.video_parameter_sets.clone();
        let source = SampleSource::new(&opts.vid, opts.definition, Arc::new(info.samples.clone()));
        tasks.push(tokio::spawn(async move {
            video::run(shared, clock, source, info, parameter_sets, opts.start_secs).await;
        }));
    }

    Ok(PlayerHandle {
        shared,
        audio: audio_out,
        tasks,
    })
}

/// 轨道的媒体时长（秒）。
fn media_end(info: &TrackInfo) -> f64 {
    if info.media_timescale > 0 && info.media_duration > 0 {
        return info.media_duration as f64 / f64::from(info.media_timescale);
    }
    info.sample_pts().last().copied().unwrap_or(0.0)
}

/// 音频喂送循环：样本 → symphonia → 交错 f32 → 环形缓冲。
///
/// 播放位置不在这里记账——时钟由输出回调按设备消费推进，喂送端只管把解码
/// 好的 PCM 源源不断放进缓冲。
async fn audio_feeder(
    shared: Arc<Shared>,
    mut feed: RingFeed,
    mut source: SampleSource,
    info: TrackInfo,
    start_secs: f64,
) {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{CODEC_TYPE_AAC, CodecParameters, DecoderOptions};
    use symphonia::core::formats::Packet;
    use symphonia::core::units::TimeBase;
    use symphonia::default::get_codecs;

    let asc = info.decoder_config.clone();
    let (channels, sample_rate) = (info.channels, info.sample_rate);
    if channels == 0 || sample_rate == 0 || asc.is_empty() {
        shared
            .fail(format!(
                "音频轨信息不全（ch={channels} sr={sample_rate} asc={}B）",
                asc.len()
            ))
            .await;
        return;
    }

    let mut params = CodecParameters::new();
    params
        .for_codec(CODEC_TYPE_AAC)
        .with_sample_rate(sample_rate)
        .with_time_base(TimeBase::new(1, sample_rate));
    params.with_extra_data(asc.into_boxed_slice());
    let mut decoder = match get_codecs().make(&params, &DecoderOptions::default()) {
        Ok(d) => d,
        Err(e) => {
            shared.fail(format!("AAC 解码器初始化失败: {e}")).await;
            return;
        }
    };

    let mut pts = info.sample_pts();
    if pts.is_empty() {
        // 没有时间表：按每样本 1024 帧摊
        pts = (0..info.samples.len())
            .map(|i| i as f64 * 1024.0 / f64::from(sample_rate))
            .collect();
    }
    // 起点对齐：第一个 PTS ≥ 起点的样本
    let begin = pts.partition_point(|&p| p < start_secs - 1e-6);

    for (idx, &pt) in pts.iter().enumerate().skip(begin).take(source.len()) {
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        // 暂停 = 停止喂（设备也停消费，时钟冻结）
        while shared.paused.load(Ordering::Relaxed)
            && !shared.shutdown.load(Ordering::Relaxed)
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let sample = match source.read(idx).await {
            Ok(s) => s,
            Err(e) => {
                shared.fail(format!("取音频样本 {idx} 失败: {e}")).await;
                return;
            }
        };
        let packet = Packet::new_from_slice(
            0,
            (pt * f64::from(sample_rate)).round() as u64,
            1024,
            &sample,
        );
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(e) => {
                // 单包解不出来不该整场静音：跳过记日志
                log::warn!("[Playback] 音频样本 {idx} 解码失败（跳过）: {e}");
                continue;
            }
        };
        if idx % 100 == 0 {
            log::debug!("[Playback][feed] 样本 {idx}（{:.1}s），clock={:.2}", pt, shared.clock.now());
        }
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sbuf.copy_interleaved_ref(decoded);
        if !feed.push(sbuf.samples(), &shared.shutdown).await {
            return;
        }
    }
}

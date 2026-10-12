//! 引擎编排与对外句柄。
//!
//! [`open`] 做完所有贵的前置（读样本表、开解码器、起音频设备）才返回句柄；
//! 随后的播放由两个任务自治：音频喂送（[`audio_feeder`]）与视频循环
//! （[`video::run`]）。宿主只拿 [`PlayerHandle`]：取帧、暂停/继续、读进度、
//! 等事件。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

use hongguo_mp4::sample_table::TrackInfo;

use crate::audio::{AudioOut, Clock, RingFeed};
use crate::source::{SampleSource, open_stream};
use crate::video;

/// 引擎事件（经调用方给的通道送达；通道关闭 = 宿主已弃，引擎自停）。
#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// 首帧已进上屏槽（宿主据此触发下一部预取）。
    FirstFrame { width: usize, height: usize },
    /// 新一帧可上屏，**帧随事件一起送达**。
    ///
    /// 不带帧、让宿主回头去 [`PlayerHandle::latest_frame`] 取，会有致命竞态：
    /// 宿主在「引擎就位」时清一次帧槽，首帧事件若先于就位到达，清空就把
    /// 「已出画」的事实抹掉了，封面永远收不掉。帧跟着事件走，宿主只认事件
    /// 里的帧，顺序天然正确。
    Frame(hongguo_media_platform::platform::vt::DecodedFrame),
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
    /// 主时钟，**可替换**：音频设备迟到时（见 [`open`]）会把它换成音频时钟，
    /// 让画面立刻跟上真实播放节奏。
    pub clock: parking_lot::Mutex<Arc<Clock>>,
    pub latest: parking_lot::Mutex<Option<hongguo_media_platform::platform::vt::DecodedFrame>>,
    pub dims: parking_lot::Mutex<(usize, usize)>,
    pub duration_secs: parking_lot::Mutex<f64>,
    pub events: mpsc::Sender<PlayerEvent>,
}

impl Shared {
    /// 当前播放位置（主时钟）。视频循环与进度上报都走这里，时钟被换掉后
    /// 各处的读数自动跟上。
    pub(crate) fn now(&self) -> f64 {
        self.clock.lock().now()
    }

    /// 换主时钟（音频迟到接管时用）。
    pub(crate) fn set_clock(&self, clock: Arc<Clock>) {
        *self.clock.lock() = clock;
    }

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
    /// 音频设备槽：起播时就绪则当场填入；迟到接管的设备由接管任务填进来。
    /// 句柄经它保活——晚到的流若没人持有，出了接管任务的作用域就被析构。
    audio: Arc<parking_lot::Mutex<Option<AudioOut>>>,
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
        self.shared.now()
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    /// 暂停：音频设备停消费（时钟冻结），视频循环停在当前帧。
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.set_paused(true);
        }
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        if let Some(audio) = self.audio.lock().as_ref() {
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
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.set_paused(true);
        }
    }
}

/// 打开并起播。必须在 tokio 运行时上下文内 await（引擎任务经 `tokio::spawn`
/// 挂在调用方的运行时上）。
pub async fn open(opts: OpenOptions) -> hongguo_common::AppResult<PlayerHandle> {
    // 起播链任何一段卡住，界面上都只是「一直转圈」——三个阶段各自打点，
    // 卡在哪一段一目了然（实测有过音频设备打开前静默两位数的秒数）。
    let t0 = std::time::Instant::now();
    let meta = open_stream(&opts.vid, opts.definition).await?;
    let video_info = meta.video;
    log::info!(
        "[Playback] 头部就绪（{}ms）：{}x{} 音频{}",
        t0.elapsed().as_millis(),
        video_info.width,
        video_info.height,
        meta.audio
            .as_ref()
            .map(|a| format!(" {}ch {}Hz", a.channels, a.sample_rate))
            .unwrap_or_else(|| "无".into())
    );

    // 时长 = 视/音两轨谁的尾巴长
    let video_end = media_end(&video_info);
    let audio_end = meta.audio.as_ref().map(media_end).unwrap_or(0.0);
    let duration = video_end.max(audio_end);

    // 先立挂钟与共享态：音频可能迟到，视频任务不能等它（见下）
    let wall = Arc::new(Clock::wall());
    wall.reset(opts.start_secs);
    let shared = Arc::new(Shared {
        paused: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        clock: parking_lot::Mutex::new(Arc::clone(&wall)),
        latest: parking_lot::Mutex::new(None),
        dims: parking_lot::Mutex::new((video_info.width as usize, video_info.height as usize)),
        duration_secs: parking_lot::Mutex::new(duration),
        events: opts.events,
    });
    let audio_slot: Arc<parking_lot::Mutex<Option<AudioOut>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let mut tasks = Vec::new();

    // 音频设备：等一个上界；超时**不丢声音**，交给接管任务（见下）。
    //
    // cpal/CoreAudio 的建流会在 HAL 层实测卡住（本机见过 570 秒才失败，栈停在
    // audio_unit_from_device），而它在起播的必经路径上——干等等于整条链路卡住：
    // 视频任务根本还没起，画面上只有封面一直盖着。所以只等一小段；没等到也
    // 先出画，同时留一个任务守着建流结果，设备迟到建成时把它接进来：换主
    // 时钟、从当前位置开始喂音频，声画立刻衔接上。
    let t_audio = std::time::Instant::now();
    match audio_attempt(meta.audio.as_ref()) {
        AudioAttempt::Ready(pair) => {
            log::info!(
                "[Playback] 音频设备阶段结束（{}ms，累计 {}ms）",
                t_audio.elapsed().as_millis(),
                t0.elapsed().as_millis()
            );
            if let Some(info) = meta.audio.clone() {
                // 等预填完成再往下走：视频任务在下面才起，两者从同一媒体
                // 位置出发（不 await 的话画面先跑、音频时钟后换，中间那截
                // 就是音画差）
                adopt_audio(
                    &shared,
                    &audio_slot,
                    pair,
                    &opts.vid,
                    opts.definition,
                    info,
                    opts.start_secs,
                )
                .await;
            }
        }
        AudioAttempt::Unusable => {
            log::info!(
                "[Playback] 音频设备阶段结束（{}ms，累计 {}ms，无声）",
                t_audio.elapsed().as_millis(),
                t0.elapsed().as_millis()
            );
        }
        AudioAttempt::Pending(rx) => {
            log::info!(
                "[Playback] 音频设备 {}ms 未就绪，先出画；建成后自动接上",
                t_audio.elapsed().as_millis()
            );
            let shared = Arc::clone(&shared);
            let slot = Arc::clone(&audio_slot);
            let (vid, definition) = (opts.vid.clone(), opts.definition);
            let audio_info = meta.audio.clone();
            tasks.push(tokio::spawn(async move {
                let Ok(Some(pair)) = rx.await else {
                    return;
                };
                if shared.shutdown.load(Ordering::Relaxed) {
                    return; // 玩家已走：产物随作用域析构，设备释放
                }
                let Some(info) = audio_info else {
                    return;
                };
                // 接管点 = 当前已经播到的位置：视频先走了，音频从这里接上
                let at = shared.now();
                log::info!("[Playback] 音频设备迟到接管（从 {at:.1}s 接上）");
                adopt_audio(&shared, &slot, pair, &vid, definition, info, at).await;
            }));
        }
    }

    // 视频循环：VT 解码 → 帧槽，节奏对主时钟
    {
        let shared = Arc::clone(&shared);
        let info = video_info.clone();
        let parameter_sets = meta.video_parameter_sets.clone();
        let source = SampleSource::new(&opts.vid, opts.definition, Arc::new(info.samples.clone()));
        let start = opts.start_secs;
        tasks.push(tokio::spawn(async move {
            video::run(shared, source, info, parameter_sets, start).await;
        }));
    }

    Ok(PlayerHandle {
        shared,
        audio: audio_slot,
        tasks,
    })
}

/// 把一对音频产物接进播放：起喂送 → 预填 → 启动设备 → 换主时钟。
///
/// 立即就绪与迟到接管共用这一段——两者唯一的差别是 `start_secs`：立即就绪
/// 时是用户要的续播点，迟到接管时是**当前已经播到的位置**（视频先走了）。
///
/// 顺序很重要：**先喂满一截再启动设备**。反过来的话设备开局先消费空缓冲，
/// 放出去的是静音；而时钟只计真实帧（见 `audio.rs` 回调），这段静音虽然
/// 不再推进时钟，但会白白拉长起播——设备在响、画面不动，读起来也是「卡」。
async fn adopt_audio(
    shared: &Arc<Shared>,
    slot: &Arc<parking_lot::Mutex<Option<AudioOut>>>,
    pair: (AudioOut, RingFeed),
    vid: &str,
    definition: u32,
    info: TrackInfo,
    start_secs: f64,
) {
    let (out, feed) = pair;
    let clock = Arc::clone(&out.clock_src);
    clock.reset(start_secs);
    // 暂停状态要带给刚接上的设备（起播时若已是暂停态）
    out.set_paused(shared.paused.load(Ordering::Relaxed));

    let (prime_tx, prime_rx) = tokio::sync::oneshot::channel();
    let feeder = {
        let shared = Arc::clone(shared);
        let source = SampleSource::new(vid, definition, Arc::new(info.samples.clone()));
        let start = start_secs;
        // 喂送任务挂在调用方的运行时上（`open` 的契约）；接管路径同样已在
        // tokio 上下文内（引擎任务都是），直接 spawn
        tokio::spawn(async move {
            audio_feeder(shared, feed, source, info, start, Some(prime_tx)).await;
        })
    };

    // 等预填（超时就把设备先启动：时钟只计真实帧，短期欠载不跑偏）
    if tokio::time::timeout(AUDIO_PRIME_TIMEOUT, prime_rx)
        .await
        .is_err()
    {
        log::info!(
            "[Playback] 音频预填 {}s 未就绪（字节还没供上来），先启动设备",
            AUDIO_PRIME_TIMEOUT.as_secs()
        );
    }
    out.start();
    // 设备起来了才把主时钟换成音频时钟：起播瞬间到设备就绪之间，画面仍按
    // 挂钟走（此时本来就还没有声音，不存在对齐对象）
    shared.set_clock(clock);
    *slot.lock() = Some(out);
    // 喂送任务已 spawn（detach），停机由 shared.shutdown 传递——
    // push 循环每 4ms 检查一次，句柄不需要留在 tasks 里
    drop(feeder);
}

/// 轨道的媒体时长（秒）。
fn media_end(info: &TrackInfo) -> f64 {
    if info.media_timescale > 0 && info.media_duration > 0 {
        return info.media_duration as f64 / f64::from(info.media_timescale);
    }
    info.sample_pts().last().copied().unwrap_or(0.0)
}

/// 建音频设备的上界等待。见 [`open`] 里该段的说明。
const AUDIO_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// 连续两次尝试之间至少要隔的时间。
///
/// 卡住的建流线程永远回不来（CoreAudio 的 HAL 调用没有超时语义），放任每集
/// 都起一个新的就会叠出一堆僵尸线程、各自占着 HAL 连接。所以两次尝试之间
/// 拉开这个间隔——但**不是永久放弃**：设备重新插拔/系统恢复后，下一次
/// 尝试还能把声音找回来。
const AUDIO_RETRY_COOLDOWN: Duration = Duration::from_secs(30);

/// 上一次建流尝试的时刻（Unix 毫秒；0 = 从未试过）。
static AUDIO_LAST_ATTEMPT_MS: AtomicU64 = AtomicU64::new(0);

/// 现在的 Unix 毫秒。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 音频设备尝试的结果。
enum AudioAttempt {
    /// 在超时内建好：直接接上。
    Ready((AudioOut, RingFeed)),
    /// 设备/音轨不可用（没有音轨、设备打不开、上次尝试还在冷却）。
    Unusable,
    /// 还没回来：视频先走，这个接收端等结果，到了就接管。
    Pending(tokio::sync::oneshot::Receiver<Option<(AudioOut, RingFeed)>>),
}

/// 建音频输出，最多等 [`AUDIO_OPEN_TIMEOUT`]。
fn audio_attempt(audio: Option<&TrackInfo>) -> AudioAttempt {
    let Some((channels, sample_rate)) = audio.map(|a| (a.channels, a.sample_rate)) else {
        return AudioAttempt::Unusable;
    };
    if channels == 0 || sample_rate == 0 {
        return AudioAttempt::Unusable;
    }
    let now = now_millis();
    let last = AUDIO_LAST_ATTEMPT_MS.load(Ordering::Acquire);
    if last != 0 && now.saturating_sub(last) < AUDIO_RETRY_COOLDOWN.as_millis() as u64 {
        log::info!(
            "[Playback] 距上次音频建流尝试不足 {}s，本次直接无声起播",
            AUDIO_RETRY_COOLDOWN.as_secs()
        );
        return AudioAttempt::Unusable;
    }
    AUDIO_LAST_ATTEMPT_MS.store(now, Ordering::Release);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("hg-audio-open".into())
        .spawn(move || {
            // 接收端已弃（超时且玩家已走）时 send 失败即丢弃产物：设备释放
            let _ = tx.send(AudioOut::open(channels, sample_rate));
        });
    if let Err(e) = spawned {
        log::warn!("[Playback] 音频建流线程起不来（{e}），无声播放");
        return AudioAttempt::Unusable;
    }

    // 超时后仍要拿回接收端（接管任务等它），所以按 &mut 轮询而不是把
    // future 整个交给 timeout（超时会把 future 一起丢掉）
    let mut rx = rx;
    let deadline = std::time::Instant::now() + AUDIO_OPEN_TIMEOUT;
    loop {
        match rx.try_recv() {
            Ok(Some(pair)) => {
                // 建流成功：冷却重置，后续集数立刻可用
                AUDIO_LAST_ATTEMPT_MS.store(0, Ordering::Release);
                return AudioAttempt::Ready(pair);
            }
            Ok(None) => {
                log::warn!("[Playback] 音频设备不可用，退化为无声播放（挂钟）");
                return AudioAttempt::Unusable;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                log::warn!("[Playback] 音频建流线程异常退出，无声播放");
                return AudioAttempt::Unusable;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
        if std::time::Instant::now() >= deadline {
            log::warn!(
                "[Playback] 音频设备建流超过 {}s 未返回（CoreAudio 卡住），先出画、等它迟到接管",
                AUDIO_OPEN_TIMEOUT.as_secs()
            );
            return AudioAttempt::Pending(rx);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 音频预填包数：设备启动前先解这么多包推进缓冲（AAC 一包 1024 帧 ≈ 23ms，
/// 8 包 ≈ 186ms，留在 ¼ 秒环形缓冲的容量内）。
///
/// 不预填就启动设备的话，设备开局先消费到一段空缓冲——放出来的是静音，而
/// 「听过静音」的这段时间在音频时钟上会被记成播放位置（不修的话就是音画
/// 不同步的头号来源）。
const AUDIO_PRIME_PACKETS: usize = 8;

/// 预填的等待上界：超时（网络还没把音频字节供上来）就先启动设备——
/// 时钟只计真实帧，短期欠载不会跑偏，只是画面会等音频一下。
const AUDIO_PRIME_TIMEOUT: Duration = Duration::from_secs(2);

/// 音频喂送循环：样本 → symphonia → 交错 f32 → 环形缓冲。
///
/// 播放位置不在这里记账——时钟由输出回调按设备消费推进，喂送端只管把解码
/// 好的 PCM 源源不断放进缓冲。
///
/// `primed`：预填够 [`AUDIO_PRIME_PACKETS`] 包时发一记信号（调用方据此启动
/// 设备，保证设备一响就有真实数据）。
async fn audio_feeder(
    shared: Arc<Shared>,
    mut feed: RingFeed,
    mut source: SampleSource,
    info: TrackInfo,
    start_secs: f64,
    primed: Option<tokio::sync::oneshot::Sender<()>>,
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

    let mut primed = primed;
    let mut pushed = 0usize;

    for (idx, &pt) in pts.iter().enumerate().skip(begin).take(source.len()) {
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        // 暂停 = 停止喂（设备也停消费，时钟冻结）
        while shared.paused.load(Ordering::Relaxed) && !shared.shutdown.load(Ordering::Relaxed) {
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
                // 单包解不出来不该整场静音，但也**不能跳过**：跳过等于把这段
                // 时间从音频里抹掉，音轨比画面短一截——每跳一包就多攒一份
                // 音画差。补一包等长静音，时间轴严丝合缝。
                log::warn!("[Playback] 音频样本 {idx} 解码失败（补静音）: {e}");
                let silence = vec![0f32; 1024 * usize::from(channels)];
                if !feed.push(&silence, &shared.shutdown).await {
                    return;
                }
                continue;
            }
        };
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sbuf.copy_interleaved_ref(decoded);
        let samples = sbuf.samples();
        // 峰值电平：**能把「喂的是静音」和「设备/音量问题」分开**的唯一
        // 观测点。全 0 意味着解码/解密没解出波形（此时画面正常但无声，
        // 只看设备状态会被误导）；有电平则问题在设备或音量侧。
        if idx % 100 == 0 {
            let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
            log::debug!(
                "[Playback][feed] 样本 {idx}（{:.1}s），clock={:.2}，峰值 {:.4}",
                pt,
                shared.now(),
                peak
            );
        }
        if !feed.push(samples, &shared.shutdown).await {
            return;
        }
        pushed += 1;
        // 预填够一截就放行设备（见 AUDIO_PRIME_PACKETS）
        if let Some(tx) = primed.take()
            && pushed >= AUDIO_PRIME_PACKETS
        {
            let _ = tx.send(());
        }
    }
    // 流很短或一直没喂满：别让调用方等满超时，直接放行
    if let Some(tx) = primed.take() {
        let _ = tx.send(());
    }
}

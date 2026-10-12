//! 引擎编排与对外句柄。
//!
//! [`open`] 做完所有贵的前置（读样本表、开解码器、起音频设备）才返回句柄；
//! 随后的播放由两个任务自治：音频喂送（[`audio_feeder`]）与视频循环
//! （[`video::run`]）。宿主只拿 [`PlayerHandle`]：取帧、暂停/继续、倍速/
//! 音量/静音、seek、读进度、等事件。
//!
//! 控制面语义与 Tauri 版一致：倍速 0.25–4（音频 WSOLA 变速不变调，时钟乘
//! rate）、音量 0–1 与静音独立、seek 冲解码器与设备缓冲后从关键帧续播。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;

use hongguo_mp4::sample_table::TrackInfo;

use crate::audio::{AudioOut, Clock, RingFeed};
use crate::source::{SampleSource, open_stream};
use crate::video;
use crate::wsola::Wsola;

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
    Frame(hongguo_media_platform::platform::video::VideoFrame),
    /// 缓冲断流：设备已暂停、时钟冻结（水位见 [`BUFFER_LOW_SECS`]）。
    Buffering,
    /// 断流恢复：设备重新起跑。
    Ready,
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
    /// 主时钟，**可替换**：音频设备迟到接管、音频播完切挂钟，都会换它，
    /// 画面与进度读数自动跟上。
    pub clock: parking_lot::Mutex<Arc<Clock>>,
    /// seek 广播：(代次, 目标媒体秒)。代次单调递增；每个消费者（视频循环/
    /// 音频喂送）各自记住处理到的代次。**不能是单槽 take**——先到的消费者
    /// 会把请求「抢走」，另一位永远不知道：音频不重定位、它名下的冲流也
    /// 永远收不了口（回调一直丢弃 → 环刚填满就被清空 → 水位无限抖动、
    /// 时钟冻结，用户看到的就是「拖进度一直缓冲中」）。
    pub seek_broadcast: parking_lot::Mutex<(u64, f64)>,
    /// 读取代次：seek 时 +1，堵在旧位置上的 serve 读取据此尽早退场
    /// （注销读者登记，让填充调度器立刻转向新位置）。
    pub read_epoch: AtomicU64,
    /// 缓冲徽标的 UI 态（Buffering/Ready 事件只按它翻转发）。与设备的
    /// starved 位解耦：seek 会先解除 starved 让回调跑起来收冲流的口，
    /// 若靠 starved 翻转发事件，这条路径上 Ready 会永远丢。
    pub buffered_ui: AtomicBool,
    /// 音量（f32 位型存储，0–1）与静音：喂给设备回调，同时是音频设备迟到
    /// 接管时的初值来源。
    pub volume: AtomicU32,
    pub muted: AtomicBool,
    pub latest: parking_lot::Mutex<Option<hongguo_media_platform::platform::video::VideoFrame>>,
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

    /// 换主时钟（音频迟到接管/音频播完切挂钟时用）。
    pub(crate) fn set_clock(&self, clock: Arc<Clock>) {
        *self.clock.lock() = clock;
    }

    /// 消费者视角取新 seek：代次比 `last_gen` 新则返回目标并就地记账。
    /// 每个消费者独立记账（单槽 take 会被先到者抢走，见 seek_broadcast）。
    pub(crate) fn poll_seek(&self, last_gen: &mut u64) -> Option<f64> {
        let (current, target) = *self.seek_broadcast.lock();
        if current != *last_gen {
            *last_gen = current;
            Some(target)
        } else {
            None
        }
    }

    /// 是否有比 `last_gen` 新的 seek 未处理（视频前瞻闸门用它及时让路）。
    pub(crate) fn seek_pending_since(&self, last_gen: u64) -> bool {
        self.seek_broadcast.lock().0 != last_gen
    }
    pub(crate) fn rate(&self) -> f64 {
        self.clock.lock().rate()
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

/// 倍速范围（与 Tauri 版 playback-prefs 的 MIN_RATE/MAX_RATE 一致）。
pub const MIN_RATE: f64 = 0.25;
pub const MAX_RATE: f64 = 4.0;

impl PlayerHandle {
    /// 当前上屏帧（NV12 CVPixelBuffer，引用计数克隆）。渲染线程调用。
    /// 当前上屏帧（macOS NV12 CVPixelBuffer / Windows NV12 系统内存，引用
    /// 计数克隆）。渲染线程调用。
    pub fn latest_frame(&self) -> Option<hongguo_media_platform::platform::video::VideoFrame> {
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
            audio.device.set_user_paused(true);
        }
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.device.set_user_paused(false);
        }
    }

    /// 当前倍速。
    pub fn rate(&self) -> f64 {
        self.shared.rate()
    }

    /// 换倍速（0.25–4）：时钟换轴不跳，音频经 WSOLA 变速不变调，
    /// 视频侧无需感知（媒体时间已含 rate）。
    pub fn set_rate(&self, rate: f64) {
        let rate = rate.clamp(MIN_RATE, MAX_RATE);
        self.shared.clock.lock().set_rate(rate);
    }

    /// 当前音量（0–1）。
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.shared
            .volume
            .store(volume.to_bits(), Ordering::Relaxed);
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.set_volume(volume);
        }
    }

    /// 静音独立于音量（对齐 Tauri 版 playback-prefs：静音后音量值保留）。
    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.set_muted(muted);
        }
    }

    /// 跳到目标媒体时间：时钟立即换轴，音频设备进入冲流（丢弃旧缓冲），
    /// 视频/音频循环取到请求后各自冲解码器、从目标前的关键帧/首个样本续播。
    ///
    /// 堵在旧位置上的流读取会收到代次变更信号尽快退场（注销读者登记，
    /// 填充调度器立刻转向新位置）；水位暂停会被解除——冲流的丢弃收口要靠
    /// 设备回调跑起来，暂停着的设备会让冲流永远收不了口（三方互等 = 永久
    /// 缓冲）。徽标的 Buffering 事件由喂送循环在处理 seek 时发出。
    pub fn seek(&self, secs: f64) {
        let dur = (*self.shared.duration_secs.lock()).max(0.0);
        let target = secs.clamp(0.0, dur);
        self.shared.read_epoch.fetch_add(1, Ordering::Relaxed);
        self.shared.clock.lock().reset(target);
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.begin_flush();
        }
        let mut broadcast = self.shared.seek_broadcast.lock();
        broadcast.0 += 1;
        broadcast.1 = target;
    }
}

impl Drop for PlayerHandle {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(audio) = self.audio.lock().as_ref() {
            audio.device.set_user_paused(true);
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

    // 时长 = 视/音两轨谁的尾巴长（elst 修剪过的 presentation 轴）
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
        seek_broadcast: parking_lot::Mutex::new((0, 0.0)),
        read_epoch: AtomicU64::new(0),
        buffered_ui: AtomicBool::new(false),
        volume: AtomicU32::new(1.0f32.to_bits()),
        muted: AtomicBool::new(false),
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
    match audio_attempt(meta.audio.as_ref(), &shared) {
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
                    *pair,
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

    // 视频循环：平台解码（macOS VT / Windows MF，见 platform::video）→ 帧槽，
    // 节奏对主时钟
    {
        let shared = Arc::clone(&shared);
        let info = video_info.clone();
        let decoder_init = video::DecoderInitOwned {
            parameter_sets: meta.video_parameter_sets.clone(),
            hvcc: meta.video_hvcc.clone(),
            length_size: meta.video_length_size,
            width: info.width as usize,
            height: info.height as usize,
        };
        let source = SampleSource::new(&opts.vid, opts.definition, Arc::new(info.samples.clone()));
        let start = opts.start_secs;
        tasks.push(tokio::spawn(async move {
            video::run(shared, source, info, decoder_init, start).await;
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
    // 倍速/音量/静音是用户偏好：新时钟与设备初值都要继承
    clock.set_rate(shared.rate());
    clock.reset(start_secs);
    out.set_volume(f32::from_bits(shared.volume.load(Ordering::Relaxed)));
    out.set_muted(shared.muted.load(Ordering::Relaxed));
    // 暂停状态要带给刚接上的设备（起播时若已是暂停态）
    out.device
        .set_user_paused(shared.paused.load(Ordering::Relaxed));

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
    // 设备起跑（用户暂停位已带上；缓冲位起播恒清零）
    out.device.start();
    // 设备起来了才把主时钟换成音频时钟：起播瞬间到设备就绪之间，画面仍按
    // 挂钟走（此时本来就还没有声音，不存在对齐对象）
    shared.set_clock(clock);
    *slot.lock() = Some(out);
    // 喂送任务已 spawn（detach），停机由 shared.shutdown 传递——
    // push 循环每 4ms 检查一次，句柄不需要留在 tasks 里
    drop(feeder);
}

/// 轨道的媒体时长（秒，presentation 轴：mdhd 时长 − elst 起点）。
fn media_end(info: &TrackInfo) -> f64 {
    if info.media_timescale > 0 && info.media_duration > 0 {
        let end = info.media_duration as f64 / f64::from(info.media_timescale);
        return (end - info.edit_media_start_secs).max(0.0);
    }
    info.sample_pts().last().copied().unwrap_or(0.0)
}

/// ASC 的 audioObjectType（诊断：2=AAC-LC，5=HE-AAC v1/SBR，29=HE-AAC v2/PS）。
fn asc_aot(asc: &[u8]) -> Option<u32> {
    let b0 = *asc.first()?;
    let b1 = *asc.get(1)?;
    let aot = u32::from((b0 >> 3) & 0x1f);
    if aot == 31 {
        // 转义：再取 6 位，实际 profile = 32 + ext
        let ext = u32::from(((b0 & 0x07) << 3) | (b1 >> 5));
        Some(32 + ext)
    } else {
        Some(aot)
    }
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
    /// 在超时内建好：直接接上（Box 抹平与 Pending 的尺寸差，别让这个
    /// 枚举在栈上按最大变体展开）。
    Ready(Box<(AudioOut, RingFeed)>),
    /// 设备/音轨不可用（没有音轨、设备打不开、上次尝试还在冷却）。
    Unusable,
    /// 还没回来：视频先走，这个接收端等结果，到了就接管。
    Pending(tokio::sync::oneshot::Receiver<Option<(AudioOut, RingFeed)>>),
}

/// 建音频输出，最多等 [`AUDIO_OPEN_TIMEOUT`]。
fn audio_attempt(audio: Option<&TrackInfo>, shared: &Shared) -> AudioAttempt {
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
    let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
    let muted = shared.muted.load(Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("hg-audio-open".into())
        .spawn(move || {
            // 接收端已弃（超时且玩家已走）时 send 失败即丢弃产物：设备释放
            let _ = tx.send(AudioOut::open(channels, sample_rate, volume, muted));
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
                return AudioAttempt::Ready(Box::new(pair));
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
/// 8 包 ≈ 186ms）。不预填就启动设备的话，开局先放一段欠载静音。
const AUDIO_PRIME_PACKETS: usize = 8;

/// 预填的等待上界：超时（网络还没把音频字节供上来）就先启动设备——
/// 时钟只计真实帧，短期欠载不会跑偏，只是画面会等音频一下。
const AUDIO_PRIME_TIMEOUT: Duration = Duration::from_secs(2);

/// 缓冲低水位：环形缓冲低于这个秒数 → 暂停设备 + Buffering（设备绝不
/// 空转放静音——主流一致，宁停不响空洞）。
const BUFFER_LOW_SECS: f64 = 0.25;

/// 缓冲高水位：回填到这个秒数才恢复播放（滞回避免抖动）。
const BUFFER_HIGH_SECS: f64 = 0.6;

/// 喂送循环的游标账本：冲流收口状态（重预填包计数 + 是否在冲流中）与
/// 样本游标。seek 重定位一次改写全部三样，收拢一处免得散参乱序。
struct FeederCursor {
    idx: usize,
    pushed: usize,
    flush_pending: bool,
}

/// 喂送侧的 seek 重定位：冲解码器与适配器相位、开冲流、登记重预填。
///
/// 附带两件联动：徽标置缓冲态并发 Buffering（seek 后必然要重新缓冲）；
/// 解除水位暂停——冲流的丢弃收口要靠设备回调跑起来，暂停着的设备回调
/// 不来，凭证永远拿不到，冲流收不了口，水位重判又被冲流闸着，三方互等
/// 就是「拖进度一直缓冲中」的死局。用户暂停位不受影响（该静着还是静着）。
async fn feeder_relocate(
    shared: &Arc<Shared>,
    feed: &mut RingFeed,
    decoder: &mut hongguo_media_platform::platform::audio::AacDecoder,
    pts: &[f64],
    target: f64,
    cursor: &mut FeederCursor,
) {
    decoder.reset();
    feed.begin_flush();
    cursor.flush_pending = true;
    cursor.pushed = 0;
    cursor.idx = pts.partition_point(|&p| p < target - 1e-6);
    // 徽标只在**播放中**置缓冲态：暂停里 seek 的是静止画面（Tauri 同款
    // 语义——新位置的首帧立即可见），没有「在缓冲」这回事；恢复播放后
    // 水位状态机会按实际情况补发边沿。
    if !shared.paused.load(Ordering::Relaxed) {
        shared.buffered_ui.store(true, Ordering::Relaxed);
        shared.emit(PlayerEvent::Buffering).await;
    }
    feed.unstarve();
    log::info!(
        "[Playback] 音频 seek 重定位到 {target:.1}s（样本 {}/{}，冲流收口待重预填）",
        cursor.idx,
        pts.len()
    );
}

/// 音频喂送循环：样本 → 平台 AAC 解码 → WSOLA 倍速 → 环形缓冲。
///
/// 播放位置不在这里记账——时钟由输出回调按设备消费推进，喂送端只管把解码
/// 好的 PCM 源源不断放进缓冲。附带三件事：seek 重定位（冲流协议）、rebuffer
/// 水位状态机、播完后的挂钟接管。
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
    // profile 留证：LC 还是 HE，一行日志钉死，后续排障不用猜
    match asc_aot(&asc) {
        Some(2) => log::info!("[Playback] 音频 profile：AOT 2（AAC-LC）"),
        Some(5) => log::info!("[Playback] 音频 profile：AOT 5（HE-AAC v1 / SBR）"),
        Some(29) => log::info!("[Playback] 音频 profile：AOT 29（HE-AAC v2 / PS）"),
        Some(other) => log::info!("[Playback] 音频 profile：AOT {other}"),
        None => log::warn!("[Playback] 音频 profile 解析不出（asc 首字节缺失）"),
    }

    let mut decoder = match hongguo_media_platform::platform::audio::AacDecoder::open(
        &asc,
        channels,
        sample_rate,
    ) {
        Ok(d) => d,
        Err(e) => {
            shared.fail(format!("AAC 解码器初始化失败: {e}")).await;
            return;
        }
    };
    let mut wsola = Wsola::new(sample_rate, channels);
    let ch = usize::from(channels);

    let mut pts = info.sample_pts();
    if pts.is_empty() {
        // 没有时间表：按每样本 1024 帧摊
        pts = (0..info.samples.len())
            .map(|i| i as f64 * 1024.0 / f64::from(sample_rate))
            .collect();
    }
    // 起点对齐：第一个 PTS ≥ 起点的样本（elst 修剪后，priming 样本已在 0 前）
    let mut cursor = FeederCursor {
        idx: pts.partition_point(|&p| p < start_secs - 1e-6),
        pushed: 0,
        flush_pending: false,
    };
    let total = pts.len().min(source.len());

    let mut primed = primed;
    // seek 广播的记账位（独立于视频循环，见 Shared::seek_broadcast）
    let mut seek_gen = 0u64;
    // 临时诊断累计：解码入量 / WSOLA 出量 / 冲流丢弃量（解码域帧）
    let mut diag_in = 0u64;
    let mut diag_out = 0u64;
    let mut diag_drop = 0u64;

    while cursor.idx < total {
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        // 暂停 = 停喂（设备也停消费，时钟冻结）；seek 仍然处理（暂停中拖
        // 进度条：恢复后必须从新位置走）
        while shared.paused.load(Ordering::Relaxed) && !shared.shutdown.load(Ordering::Relaxed) {
            if let Some(t) = shared.poll_seek(&mut seek_gen) {
                feeder_relocate(
                    &shared,
                    &mut feed,
                    &mut decoder,
                    pts.as_slice(),
                    t,
                    &mut cursor,
                )
                .await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        if let Some(t) = shared.poll_seek(&mut seek_gen) {
            feeder_relocate(
                &shared,
                &mut feed,
                &mut decoder,
                pts.as_slice(),
                t,
                &mut cursor,
            )
            .await;
            continue;
        }
        // 读样本带读取代次：seek 会抬代次，堵在旧位置上的 serve 请求尽早
        // 退场（注销读者登记，填充调度器立刻转向新位置）——干等旧数据的话
        // seek 的就绪被白白拖后一个区间。
        let my_epoch = shared.read_epoch.load(Ordering::Relaxed);
        let shared_for_read = Arc::clone(&shared);
        let sample = match source
            .read_cancellable(cursor.idx, move || {
                shared_for_read.read_epoch.load(Ordering::Relaxed) != my_epoch
            })
            .await
        {
            Ok(s) => s,
            Err(e) => {
                if shared.seek_pending_since(seek_gen) {
                    continue; // seek 引起的退场：循环顶上重定位
                }
                shared
                    .fail(format!("取音频样本 {} 失败: {e}", cursor.idx))
                    .await;
                return;
            }
        };
        let decoded = match decoder.decode(&sample) {
            Ok(d) => d,
            Err(e) => {
                // 单包解不出来不该整场静音，但也**不能跳过**：跳过等于把这段
                // 时间从音频里抹掉，音轨比画面短一截——每跳一包就多攒一份
                // 音画差。补一包等长静音，时间轴严丝合缝。
                log::warn!("[Playback] 音频样本 {} 解码失败（补静音）: {e}", cursor.idx);
                vec![0f32; 1024 * ch]
            }
        };
        // 倍速伸缩（rate 变化逐窗生效）
        let rate = shared.rate();
        let mut stretched = Vec::with_capacity(decoded.len() / 2);
        wsola.process(&decoded, rate, &mut stretched);
        diag_in += (decoded.len() / ch) as u64;
        diag_out += (stretched.len() / ch) as u64;
        if !stretched.is_empty() {
            // 冲流期环放不下就丢这包：老数据还没被回调丢完、设备又暂停着时
            //（用户暂停/水位暂停），等在 push 上会让喂送任务连同冲流收口、
            // 水位重判一起卡死——「拖进度一直缓冲中」的根因。冲流边界本来
            // 就是要丢的内容；时间轴的连续由时钟换轴负责，不靠这包撑着。
            let need_secs = stretched.len() as f64 / ch as f64 / f64::from(sample_rate);
            if cursor.flush_pending && feed.vacant_secs() < need_secs {
                diag_drop += (stretched.len() / ch) as u64;
                log::debug!("[Playback] 冲流期环满，丢一包等回调腾位");
            } else if !feed
                .push(&stretched, &shared.shutdown, &shared.paused)
                .await
            {
                return;
            } else {
                cursor.pushed += 1;
            }
        }
        // 冲流收口：预填够 + 设备确实跑过丢弃周期（设备暂停时回调不来，
        // 宁可等恢复也别让旧声音漏出去）
        if cursor.flush_pending && cursor.pushed >= AUDIO_PRIME_PACKETS && feed.end_flush_if_seen()
        {
            cursor.flush_pending = false;
        }
        // 预填够一截就放行设备（见 AUDIO_PRIME_PACKETS）
        if let Some(tx) = primed.take()
            && cursor.pushed >= AUDIO_PRIME_PACKETS
        {
            let _ = tx.send(());
        }
        // 水位状态机（冲流期间不判：环里混着待丢弃的旧数据）
        if !cursor.flush_pending {
            let ring_secs = feed.buffered_secs();
            if ring_secs <= BUFFER_LOW_SECS && feed.set_starved(true) {
                log::info!("[Playback] 缓冲低水位（{ring_secs:.2}s），暂停设备等填充");
            } else if ring_secs >= BUFFER_HIGH_SECS && feed.set_starved(false) {
                log::info!("[Playback] 缓冲回填（{ring_secs:.2}s），恢复播放");
            }
            // UI 徽标边沿只按 buffered_ui 翻转：seek 路径上 starved 被提前
            // 解除（收冲流的口），靠它的翻转沿发事件会把 Ready 永远丢掉。
            if ring_secs <= BUFFER_LOW_SECS && !shared.buffered_ui.swap(true, Ordering::Relaxed) {
                shared.emit(PlayerEvent::Buffering).await;
            } else if ring_secs >= BUFFER_HIGH_SECS
                && shared.buffered_ui.swap(false, Ordering::Relaxed)
            {
                shared.emit(PlayerEvent::Ready).await;
            }
        }
        cursor.idx += 1;
        // 喂送心跳（debug）：喂到哪、环多满、各状态位——排「喂送卡死/失聪」
        // 类问题的第一现场
        if cursor.idx.is_multiple_of(200) {
            log::debug!(
                "[Playback][feeder] idx {}/{} 环 {:.2}s paused={} flush={} gen={} 入{diag_in} 出{diag_out} 丢{diag_drop}",
                cursor.idx,
                total,
                feed.buffered_secs(),
                shared.paused.load(Ordering::Relaxed),
                cursor.flush_pending,
                seek_gen
            );
        }
    }
    // 流很短或一直没喂满：别让调用方等满超时，直接放行
    if let Some(tx) = primed.take() {
        let _ = tx.send(());
    }

    // 喂完：水位暂停就此作废——后面不会再有数据，「等填充」永远等不到，
    // 设备必须跑起来把环里最后一段放干，下面的放干等待才有尽头（否则
    // 尾段一次低水位就能把设备暂停在残量上：环放不干、挂钟接不了、
    // 时钟冻结、徽标常亮——seek 到音频轨末尾尤其必现）。
    feed.unstarve();
    // 徽标若还亮着就此收口：不会再有回填沿了
    if shared.buffered_ui.swap(false, Ordering::Relaxed) {
        shared.emit(PlayerEvent::Ready).await;
    }

    // 喂完：等设备把环形缓冲放干，再把主时钟切到挂钟——音频时钟在缓冲
    // 放干后不再推进，视频尾巴（比音轨长时）会被冻住，Ended 也到不了。
    while feed.buffered_secs() > 0.0 && !shared.shutdown.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let at = shared.now();
    let rate = shared.rate();
    let wall = Arc::new(Clock::wall());
    wall.set_rate(rate);
    wall.reset(at);
    shared.set_clock(wall);
    log::info!("[Playback] 音频喂送结束（{at:.1}s 起切挂钟收尾）");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASC 的 AOT 解析：LC 与 HE 两族都要读对（诊断日志的地基）。
    /// 位序：首 5 位 = AOT；31 时再取 6 位（实际 profile = 32 + ext）。
    #[test]
    fn asc_aot_parses_lc_and_he() {
        // AOT=2（LC）：00010 → 0b0001_0xxx
        assert_eq!(asc_aot(&[0x10, 0x00]), Some(2));
        // AOT=5（HE v1）：00101 → 0b0010_1xxx
        assert_eq!(asc_aot(&[0x29, 0x00]), Some(5));
        // AOT=29（HE v2）：11101 → 0xE8 高 5 位
        assert_eq!(asc_aot(&[0xE8, 0x00]), Some(29));
        // AOT=31 转义：11111 + 000000 → 32
        assert_eq!(asc_aot(&[0xF8, 0x00]), Some(32));
        assert_eq!(asc_aot(&[]), None);
    }

    /// media_end 在 elst 修剪下扣掉 edit 起点（priming 不该算进时长）。
    #[test]
    fn media_end_subtracts_edit_start() {
        let mut info = TrackInfo {
            media_timescale: 48_000,
            media_duration: 48_000 * 100,
            edit_media_start_secs: 2112.0 / 48_000.0,
            ..Default::default()
        };
        assert!((media_end(&info) - (100.0 - 2112.0 / 48_000.0)).abs() < 1e-9);
        info.media_timescale = 0;
        info.media_duration = 0;
        assert_eq!(media_end(&info), 0.0);
    }
}

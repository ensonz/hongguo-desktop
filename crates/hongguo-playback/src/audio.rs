//! 音频输出与主时钟。
//!
//! 时钟口径是**可闻位置**：`base + (已消费帧数 − 设备引线) × rate ÷ 采样率`。
//! 已消费是设备回调拉走的帧数；引线来自回调时间戳（`playback − callback`，
//! 即这批样本到 DAC 的提前量）——不扣的话时钟快于耳朵，画面就跟着快，
//! 这是音画不同步的系统性来源之一（蓝牙/大缓冲设备上更明显）。主流播放器
//! （Chromium/ExoPlayer）的音频时钟都取可闻位置。
//!
//! 暂停即冻结（设备不消费）；倍速直接乘在时钟上（换速时 rebase）。无音频轨
//! 或设备打不开时退化为挂钟（暂停在该模式下失效——首页场景恒有音轨）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

/// 可闻位置时钟。媒体时间域 = 样本 PTS（秒）。
pub(crate) struct Clock {
    sample_rate: u64,
    /// 音频已播帧数（每声道一个样本算一帧）。
    consumed: AtomicU64,
    /// 设备引线（帧）：这批样本从回调到 DAC 的提前量。≤ consumed。
    lead: AtomicU64,
    /// base（媒体秒）与倍速：换速时 rebase，两者必须原子地一起动。
    state: Mutex<ClockState>,
    /// 无音频模式：起点挂钟。
    wall: Mutex<Option<Instant>>,
}

struct ClockState {
    base: f64,
    rate: f64,
}

impl Clock {
    pub(crate) fn audio(sample_rate: u32) -> Self {
        Self {
            sample_rate: u64::from(sample_rate),
            consumed: AtomicU64::new(0),
            lead: AtomicU64::new(0),
            state: Mutex::new(ClockState {
                base: 0.0,
                rate: 1.0,
            }),
            wall: Mutex::new(None),
        }
    }

    pub(crate) fn wall() -> Self {
        Self {
            sample_rate: 0,
            consumed: AtomicU64::new(0),
            lead: AtomicU64::new(0),
            state: Mutex::new(ClockState {
                base: 0.0,
                rate: 1.0,
            }),
            wall: Mutex::new(None),
        }
    }

    /// 设播放起点（媒体秒）。计数清零，now() 从 base 起走。倍速保留
    /// （用户偏好不因起播/seek 重置）。
    pub(crate) fn reset(&self, base_secs: f64) {
        let mut st = self.state.lock();
        st.base = base_secs;
        drop(st);
        self.consumed.store(0, Ordering::Relaxed);
        self.lead.store(0, Ordering::Relaxed);
        *self.wall.lock() = (self.sample_rate == 0).then(Instant::now);
    }

    pub(crate) fn now(&self) -> f64 {
        let st = self.state.lock();
        if self.sample_rate == 0 {
            let start = self.wall.lock().expect("挂钟已设");
            return st.base + start.elapsed().as_secs_f64() * st.rate;
        }
        let consumed = self.consumed.load(Ordering::Relaxed);
        let lead = self.lead.load(Ordering::Relaxed).min(consumed);
        st.base + (consumed - lead) as f64 / self.sample_rate as f64 * st.rate
    }

    /// 换倍速：按旧速率算出当前位并锁内 rebase（清计数），时间轴不跳。
    pub(crate) fn set_rate(&self, rate: f64) {
        let mut st = self.state.lock();
        st.base = audio_now(&st, self.sample_rate, &self.consumed, &self.lead);
        st.rate = rate;
        drop(st);
        self.consumed.store(0, Ordering::Relaxed);
        self.lead.store(0, Ordering::Relaxed);
    }

    pub(crate) fn rate(&self) -> f64 {
        self.state.lock().rate
    }

    /// 输出回调记账：设备又消费了 `frames` 帧、当前引线 `lead` 帧。
    /// 帧数按增量累加（漏一次回调也不失真），引线按最新值覆盖（可闻位置
    /// 读数时再钳到 ≤ 已消费）。
    fn advance(&self, frames: u64, lead: u64) {
        self.consumed.fetch_add(frames, Ordering::Relaxed);
        self.lead.store(lead, Ordering::Relaxed);
    }
}

/// now() 的音频支路（set_rate 锁内复用）。
fn audio_now(st: &ClockState, sample_rate: u64, consumed: &AtomicU64, lead: &AtomicU64) -> f64 {
    if sample_rate == 0 {
        return st.base;
    }
    let c = consumed.load(Ordering::Relaxed);
    let l = lead.load(Ordering::Relaxed).min(c);
    st.base + (c - l) as f64 / sample_rate as f64 * st.rate
}

/// cpal 流的 Send 包装。
///
/// CoreAudio 的流对象是线程安全的不透明对象：数据回调跑在设备自己的 IO 线程
/// （与句柄在哪个线程无关），`pause()`/`play()`（AudioOutputUnitStart/Stop）
/// 本身线程安全。cpal 出于保守统一标了 `!Send`，跨执行器搬运句柄的约束由
/// 本类型给出。
struct SendStream(cpal::Stream);
unsafe impl Send for SendStream {}

/// 设备级播放开关的仲裁：**用户暂停**与**缓冲断流暂停**各占一位，设备只在
/// 两位都清零时运转。两个暂停源来自不同任务（控制面 vs 喂送循环），必须
/// 单点合成，否则「缓冲恢复」会把用户暂停悄悄解除。
pub(crate) struct AudioDevice {
    stream: Mutex<Option<SendStream>>,
    user_paused: AtomicBool,
    starved: AtomicBool,
}

impl AudioDevice {
    fn apply(&self) {
        let guard = self.stream.lock();
        let Some(stream) = guard.as_ref() else {
            return;
        };
        let should_play =
            !self.user_paused.load(Ordering::Relaxed) && !self.starved.load(Ordering::Relaxed);
        // pause/play 的错误类型不同，分支各自消化
        let result = if should_play {
            stream.0.play().map_err(|e| e.to_string())
        } else {
            stream.0.pause().map_err(|e| e.to_string())
        };
        if let Err(e) = result {
            log::warn!(
                "[Playback] 音频设备 {}失败: {e}",
                if should_play { "启动" } else { "暂停" }
            );
        }
    }

    pub(crate) fn set_user_paused(&self, paused: bool) {
        self.user_paused.store(paused, Ordering::Relaxed);
        self.apply();
    }

    /// 缓冲水位控制（rebuffer 状态机）。返回值 = 状态是否发生翻转
    /// （翻转沿是发 Buffering/Ready 事件的时机）。
    pub(crate) fn set_starved(&self, starved: bool) -> bool {
        if self.starved.swap(starved, Ordering::Relaxed) == starved {
            return false;
        }
        self.apply();
        true
    }

    /// 设备起跑闸（起播预填完成后调用）：两位暂停标志都清零才让设备转。
    pub(crate) fn start(&self) {
        self.apply();
    }
}

/// 音频输出与主时钟的控制块（设备 + 回调里的可变量）。
pub(crate) struct AudioOut {
    pub(crate) device: Arc<AudioDevice>,
    pub(crate) clock_src: Arc<Clock>,
    /// 音量（f32 位型存储；1.0 = 全量）。回调内乘 PCM——零延迟生效。
    volume: Arc<AtomicU32>,
    /// 静音独立于音量（对齐 Tauri 版 playback-prefs 语义）。
    muted: Arc<AtomicBool>,
    /// 冲流中：回调丢弃环形缓冲里的所有旧数据、不推进时钟（seek 用）。
    discarding: Arc<AtomicBool>,
    /// 冲流收口凭证（见 [`RingFeed::end_flush_if_seen`]）。
    discard_seen: Arc<AtomicBool>,
    /// 欠载静音累计（诊断）：持续增长 = 喂送跟不上；接近 0 = 健康。
    #[expect(dead_code, reason = "诊断字段：与 [cb] 日志配套，排障时按需读取")]
    pub(crate) silence_frames: Arc<AtomicU64>,
}

impl AudioOut {
    /// 建输出流。`channels`/`sample_rate` 是解码侧格式；音量/静音取当前值。
    /// 不在这里启动设备：调用方先预填（见 player::adopt_audio）。
    pub(crate) fn open(
        channels: u16,
        sample_rate: u32,
        volume: f32,
        muted: bool,
    ) -> Option<(Self, RingFeed)> {
        let device = cpal::default_host().default_output_device()?;
        let ring = ringbuf::HeapRb::<f32>::new(usize::from(channels) * sample_rate as usize); // 1 秒
        let (prod, mut cons) = ring.split();

        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let clock = Arc::new(Clock::audio(sample_rate));
        let cb_clock = Arc::clone(&clock);
        let cb_calls = Arc::new(AtomicU64::new(0));
        // 欠载（缓冲空、只能出静音）累计帧数。
        //
        // **静音不推进时钟**：回调里只把真实取出的帧计入 consumed。设备在
        // 解码器还没喂上来的那几十毫秒里空转、或中途追不上喂送时，都会往
        // 喇叭送静音——那些帧要是也计时，音频时钟就跑到真实声音前面去。
        let cb_silence = Arc::new(AtomicU64::new(0));
        let cb_silence_w = Arc::clone(&cb_silence);
        let cb_volume = Arc::new(AtomicU32::new(volume.to_bits()));
        let cb_volume_w = Arc::clone(&cb_volume);
        let cb_muted = Arc::new(AtomicBool::new(muted));
        let cb_muted_w = Arc::clone(&cb_muted);
        let cb_discard = Arc::new(AtomicBool::new(false));
        let cb_discard_w = Arc::clone(&cb_discard);
        let cb_discard_feed = Arc::clone(&cb_discard);
        // 冲流的收口凭证：回调真跑过至少一个丢弃周期（设备暂停时回调不来，
        // 拿不到凭证——宁可等恢复，也别让旧声音漏出去）
        let cb_seen = Arc::new(AtomicBool::new(false));
        let cb_seen_w = Arc::clone(&cb_seen);
        let cb_seen_out = Arc::clone(&cb_seen);
        let channels_cb = usize::from(channels);
        // 累计已消费帧数与引线都在时钟里，回调只管报数
        let err = move |e| log::warn!("[Playback] 音频设备回调错误: {e}");
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    if cb_discard.load(Ordering::Relaxed) {
                        // 冲流：整环丢弃（一次回调清空），出静音、不计时
                        cb_seen_w.store(true, Ordering::Relaxed);
                        let mut scratch = [0f32; 64];
                        loop {
                            if cons.pop_slice(&mut scratch) < scratch.len() {
                                break;
                            }
                        }
                        data.fill(0.0);
                        return;
                    }
                    let filled = cons.pop_slice(data); // 样本数
                    // 设备引线：这批数据到 DAC 的提前量（帧）
                    let lead = info
                        .timestamp()
                        .playback
                        .duration_since(&info.timestamp().callback)
                        .map(|d| d.as_secs_f64() * sample_rate as f64)
                        .unwrap_or(0.0);
                    let frames = filled / channels_cb;
                    if cb_muted.load(Ordering::Relaxed) {
                        // 静音不是暂停：照常推进时钟，只是听不见
                        data.fill(0.0);
                    } else {
                        let vol = f32::from_bits(cb_volume.load(Ordering::Relaxed));
                        if vol != 1.0 {
                            for s in &mut data[..filled] {
                                *s *= vol;
                            }
                        }
                        // 欠载的尾巴补静音
                        for slot in &mut data[filled..] {
                            *slot = 0.0;
                        }
                    }
                    let silent_frames = ((data.len() - filled) / channels_cb) as u64;
                    let silence = cb_silence_w.fetch_add(silent_frames, Ordering::Relaxed);
                    // 只计真实音频：时钟 =「已经送进设备的真实声音」的可闻位置
                    cb_clock.advance(frames as u64, lead as u64);
                    let n = cb_calls.fetch_add(1, Ordering::Relaxed);
                    if n.is_multiple_of(200) {
                        log::debug!(
                            "[Playback][cb] 第 {n} 次回调，clock={:.2}s，引线 {:.0} 帧，欠载 {:.1}s（累计）",
                            cb_clock.now(),
                            lead,
                            (silence + silent_frames) as f64 / f64::from(sample_rate)
                        );
                    }
                },
                err,
                None,
            )
            .ok()?;
        log::info!("[Playback] 音频设备已建（{channels}ch {sample_rate}Hz）");
        let device = Arc::new(AudioDevice {
            stream: Mutex::new(Some(SendStream(stream))),
            user_paused: AtomicBool::new(false),
            starved: AtomicBool::new(false),
        });
        Some((
            Self {
                device: Arc::clone(&device),
                clock_src: clock,
                volume: cb_volume_w,
                muted: cb_muted_w,
                discarding: cb_discard_w,
                discard_seen: cb_seen_out,
                silence_frames: cb_silence,
            },
            RingFeed {
                producer: prod,
                discarding: cb_discard_feed,
                discard_seen: cb_seen,
                device,
            },
        ))
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        self.volume.store(volume.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// 开始冲流（seek 跳点即调）：回调进入丢弃态并清零收口凭证，
    /// 收口由喂送端重新预填后经 [`RingFeed::end_flush_if_seen`] 完成。
    pub(crate) fn begin_flush(&self) {
        self.discard_seen.store(false, Ordering::Relaxed);
        self.discarding.store(true, Ordering::Relaxed);
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        // 先停设备再释放：Drop 顺序里显式 pause 一遍（语义清晰、代价为零）
        self.device.set_user_paused(true);
        *self.device.stream.lock() = None;
    }
}

/// 音频解码循环往环形缓冲投放的把手，兼带冲流协议的收口端。
pub(crate) struct RingFeed {
    producer: ringbuf::HeapProd<f32>,
    discarding: Arc<AtomicBool>,
    discard_seen: Arc<AtomicBool>,
    device: Arc<AudioDevice>,
}

impl RingFeed {
    /// 投放一段交错 PCM；缓冲满时让出任务等设备消费（消费速率由硬件决定）。
    /// 返回 false = 已收到停机信号。
    pub(crate) async fn push(&mut self, samples: &[f32], shutdown: &AtomicBool) -> bool {
        let mut done = 0usize;
        while done < samples.len() {
            if shutdown.load(Ordering::Relaxed) {
                return false;
            }
            done += self.producer.push_slice(&samples[done..]);
            if done < samples.len() {
                tokio::time::sleep(std::time::Duration::from_millis(4)).await;
            }
        }
        true
    }

    /// 开始冲流（与 [`AudioOut::begin_flush`] 同一协议，喂送端视角）。
    pub(crate) fn begin_flush(&self) {
        self.discard_seen.store(false, Ordering::Relaxed);
        self.discarding.store(true, Ordering::Relaxed);
    }

    /// 结束冲流：**仅当**回调真跑过丢弃周期才放行（设备暂停时回调不来，
    /// 凭证拿不到就等——否则旧数据会在恢复后漏出去）。返回是否已收口。
    pub(crate) fn end_flush_if_seen(&self) -> bool {
        if self.discard_seen.load(Ordering::Relaxed) {
            self.discarding.store(false, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// 环内滞留样本数（rebuffer 水位判断用）。
    pub(crate) fn buffered_frames(&self) -> usize {
        self.producer.occupied_len()
    }

    /// 缓冲断流暂停（见 AudioDevice::set_starved）。
    pub(crate) fn set_starved(&self, starved: bool) -> bool {
        self.device.set_starved(starved)
    }
}

/// 冲流协议 + 水位状态机的单元测试（不碰真设备，纯逻辑部分）。
#[cfg(test)]
mod tests {
    use super::*;

    /// 时钟数学：可闻位置 = base + (消费 − 引线) × rate ÷ 采样率。
    #[test]
    fn clock_counts_audible_position_not_pulled() {
        let clock = Clock::audio(48_000);
        clock.reset(10.0);
        // 拉了 1 秒、引线 480 帧（10ms）：可闻位置 10s + 0.99s
        clock.advance(48_000, 480);
        assert!((clock.now() - 10.99).abs() < 1e-9);
    }

    /// 引线不可能超过已消费量（起播瞬间）：钳到 0，不出负数时间。
    #[test]
    fn clock_clamps_lead_to_consumed() {
        let clock = Clock::audio(48_000);
        clock.reset(0.0);
        clock.advance(100, 9_999);
        assert!(clock.now() >= 0.0);
    }

    /// 换倍速 rebase：时间轴连续，换速后按新速率推进。
    #[test]
    fn clock_rebases_on_rate_change() {
        let clock = Clock::audio(48_000);
        clock.reset(0.0);
        clock.advance(48_000, 0); // 1s
        clock.set_rate(2.0);
        let at_switch = clock.now();
        assert!((at_switch - 1.0).abs() < 1e-9, "换速瞬间不跳");
        clock.advance(48_000, 0); // 再消费 1s 设备时间 = 媒体 2s
        assert!((clock.now() - 3.0).abs() < 1e-9);
    }

    /// 倍速对挂钟模式同样生效（无音频退化路径）。
    #[test]
    fn wall_clock_scales_with_rate() {
        let clock = Clock::wall();
        clock.set_rate(2.0);
        clock.reset(5.0);
        assert!((clock.now() - 5.0).abs() < 0.05);
    }
}

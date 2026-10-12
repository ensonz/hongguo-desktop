//! 音频输出与主时钟。
//!
//! 时钟 = 起点媒体时间 + 扬声器**已消费**的帧数 ÷ 采样率：暂停即冻结（设备
//! 不再消费），失真只可能来自设备缓冲漂移，量级远小于画面可察觉阈值。无音
//! 频轨或设备打不开时退化为挂钟（暂停在该模式下失效——首页场景恒有音轨）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use ringbuf::traits::{Consumer, Producer, Split};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 主时钟。媒体时间域 = 样本 PTS（秒）。
pub(crate) struct Clock {
    sample_rate: u64,
    /// 音频已播帧数（每声道一个样本算一帧）。
    consumed: AtomicU64,
    base: parking_lot::Mutex<f64>,
    /// 无音频模式：起点挂钟。
    wall: Mutex<Option<Instant>>,
}

impl Clock {
    pub(crate) fn audio(sample_rate: u32) -> Self {
        Self {
            sample_rate: u64::from(sample_rate),
            consumed: AtomicU64::new(0),
            base: Mutex::new(0.0),
            wall: Mutex::new(None),
        }
    }

    pub(crate) fn wall() -> Self {
        Self {
            sample_rate: 0,
            consumed: AtomicU64::new(0),
            base: Mutex::new(0.0),
            wall: Mutex::new(None),
        }
    }

    /// 设播放起点（媒体秒）。计数清零，now() 从 base 起走。
    pub(crate) fn reset(&self, base_secs: f64) {
        *self.base.lock() = base_secs;
        self.consumed.store(0, Ordering::Relaxed);
        *self.wall.lock() = (self.sample_rate == 0).then(Instant::now);
    }

    pub(crate) fn now(&self) -> f64 {
        if self.sample_rate == 0 {
            let start = self.wall.lock().expect("挂钟已设");
            return *self.base.lock() + start.elapsed().as_secs_f64();
        }
        *self.base.lock() + self.consumed.load(Ordering::Relaxed) as f64 / self.sample_rate as f64
    }

    /// 输出回调记账：设备又消费了 `frames` 帧。
    fn consumed(&self, frames: u64) {
        self.consumed.fetch_add(frames, Ordering::Relaxed);
    }
}

/// cpal 流的 Send 包装。
///
/// CoreAudio 的流对象是线程安全的不透明对象：数据回调跑在设备自己的 IO 线程
/// （与句柄在哪个线程无关），`pause()`/`play()`（AudioOutputUnitStart/Stop）
/// 本身线程安全。cpal 出于保守统一标了 `!Send`，跨执行器搬运句柄的约束由
/// 本类型给出。
struct SendStream(cpal::Stream);
unsafe impl Send for SendStream {}

/// 扬声器输出：设备流 + 解码 PCM 的投放端。
///
/// 流建不出来（无设备/格式拒绝）返回 `None`，调用方退化挂钟——有画面没
/// 声音仍可看，不该整场失败。
pub(crate) struct AudioOut {
    stream: Mutex<Option<SendStream>>,
    pub(crate) clock_src: Arc<Clock>,
}

impl AudioOut {
    /// 建输出流并起播。`channels`/`sample_rate` 是解码侧的格式。
    pub(crate) fn open(channels: u16, sample_rate: u32) -> Option<(Self, RingFeed)> {
        let device = cpal::default_host().default_output_device()?;
        let ring = ringbuf::HeapRb::<f32>::new(
            usize::from(channels) * sample_rate as usize / 4, // ¼ 秒缓冲
        );
        let (prod, mut cons) = ring.split();

        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let consumed_clock = Arc::new(Clock::audio(sample_rate));
        let cb_clock = Arc::clone(&consumed_clock);
        let cb_calls = Arc::new(AtomicU64::new(0));
        let err = move |e| log::warn!("[Playback] 音频设备回调错误: {e}");
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let filled = cons.pop_slice(data);
                    for slot in &mut data[filled..] {
                        *slot = 0.0; // 欠载静音
                    }
                    cb_clock.consumed((data.len() / usize::from(channels)) as u64);
                    let n = cb_calls.fetch_add(1, Ordering::Relaxed);
                    if n.is_multiple_of(200) {
                        log::debug!(
                            "[Playback][cb] 第 {n} 次回调，consumed={}",
                            cb_clock.now()
                        );
                    }
                },
                err,
                None,
            )
            .ok()?;
        stream.play().ok()?;
        log::info!("[Playback] 音频设备已开（{channels}ch {sample_rate}Hz）");
        Some((
            Self {
                stream: Mutex::new(Some(SendStream(stream))),
                clock_src: consumed_clock,
            },
            RingFeed { producer: prod },
        ))
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        if let Some(stream) = self.stream.lock().as_ref() {
            // pause/play 的错误类型不同（Pause/PlayStreamError），分支各自消化
            let result = if paused {
                stream.0.pause().map_err(|e| e.to_string())
            } else {
                stream.0.play().map_err(|e| e.to_string())
            };
            if let Err(e) = result {
                log::warn!("[Playback] 音频设备 {}失败: {e}", if paused { "暂停" } else { "恢复" });
            }
        }
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        // 先停设备再释放：Drop 顺序里显式 pause 一遍（语义清晰、代价为零）
        self.set_paused(true);
        *self.stream.lock() = None;
    }
}

/// 音频解码循环往环形缓冲投放的把手。
pub(crate) struct RingFeed {
    producer: ringbuf::HeapProd<f32>,
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
}

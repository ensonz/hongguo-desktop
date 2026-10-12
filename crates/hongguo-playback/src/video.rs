//! 视频轨：VT 逐帧解码 + 按主时钟上屏。
//!
//! 解码从**起点前的最近关键帧**起步（关键帧之前的样本是必需参考帧，产物
//! 帧按 PTS 过滤掉不上屏），节奏完全对主时钟：`now() >= 帧 PTS` 才把帧放进
//! 上屏槽并通知宿主。同步解码吐出的是解码序，每帧按自带 PTS 对表，不按吐出
//! 顺序排位（与转码链同一纪律）。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use hongguo_media_platform::platform::vt::HevcFrameDecoder;
use hongguo_mp4::sample_table::TrackInfo;

use crate::audio::Clock;
use crate::player::Shared;
use crate::source::SampleSource;

/// 起播样本号：起点前最近的**关键帧**（全关键帧轨直接取第一个 ≥ 起点的样本）。
pub(crate) fn start_index(info: &TrackInfo, start_secs: f64) -> usize {
    let pts = info.sample_pts();
    let eps = 1e-6;
    match &info.stss {
        None => pts.partition_point(|&p| p < start_secs - eps),
        Some(sync) => {
            let mut best = 0usize;
            for &n in sync {
                let i = n as usize - 1; // stss 是 1 基
                if i < pts.len() && pts[i] <= start_secs + eps {
                    best = best.max(i);
                }
            }
            best
        }
    }
}

/// 视频主循环。返回自然播完（中途退出由停机/事件错误控制）。
pub(crate) async fn run(
    shared: Arc<Shared>,
    clock: Arc<Clock>,
    mut source: SampleSource,
    info: TrackInfo,
    parameter_sets: Vec<Vec<u8>>,
    start_secs: f64,
) {
    let begin_at = start_index(&info, start_secs);
    let mut decoder = match HevcFrameDecoder::open(&parameter_sets, true) {
        Ok(d) => d,
        Err(e) => {
            shared.fail(format!("解码器初始化失败: {e}")).await;
            return;
        }
    };
    *shared.dims.lock() = (info.width as usize, info.height as usize);

    let mut pts = info.sample_pts();
    if pts.is_empty() {
        let fps = info.average_framerate().unwrap_or(25.0);
        pts = (0..info.samples.len()).map(|i| i as f64 / fps).collect();
    }
    let frame_dur = 1.0 / info.average_framerate().unwrap_or(25.0);
    let mut first_sent = false;
    let shown_frames = Arc::new(std::sync::atomic::AtomicU64::new(0));

    for (idx, &pt) in pts.iter().enumerate().skip(begin_at).take(source.len()) {
        // 等到这一帧的显示时刻（时钟冻结在暂停时，这里自然一起停）
        while clock.now() < pt && !shared.shutdown.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let sample = match source.read(idx).await {
            Ok(s) => s,
            Err(e) => {
                shared.fail(format!("取视频样本 {idx} 失败: {e}")).await;
                return;
            }
        };
        let frames = match decoder.decode(&sample, pt, frame_dur) {
            Ok(frames) => frames,
            Err(e) => {
                shared.fail(format!("视频样本 {idx} 解码失败: {e}")).await;
                return;
            }
        };
        // 起点前的参考帧不入槽（起点对齐由关键帧起步 + 这条过滤共同完成）
        for frame in frames {
            if frame.pts_secs < start_secs - frame_dur {
                continue;
            }
            let (w, h) = frame.dims();
            *shared.latest.lock() = Some(frame);
            if !first_sent {
                first_sent = true;
                log::info!(
                    "[Playback] 首帧 {w}x{h}（起点 {start_secs:.1}s，时长 {:.1}s）",
                    *shared.duration_secs.lock()
                );
                shared
                    .emit(crate::player::PlayerEvent::FirstFrame { width: w, height: h })
                    .await;
            }
            let shown = shown_frames.fetch_add(1, Ordering::Relaxed);
            if shown.is_multiple_of(100) {
                log::debug!(
                    "[Playback][video] 已上屏 {shown} 帧（{:.2}s），clock={:.2}",
                    pt,
                    clock.now()
                );
            }
            shared.emit(crate::player::PlayerEvent::Frame).await;
        }
    }
    match decoder.finish() {
        Ok(tail) => {
            for frame in tail {
                if frame.pts_secs < start_secs - frame_dur {
                    continue;
                }
                *shared.latest.lock() = Some(frame);
                shared.emit(crate::player::PlayerEvent::Frame).await;
            }
        }
        Err(e) => log::warn!("[Playback] 解码收尾失败: {e}"),
    }

    // 等时钟走到结尾再宣布结束（音频尾音放完）
    let end = *shared.duration_secs.lock();
    while clock.now() < end && !shared.shutdown.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    shared.emit(crate::player::PlayerEvent::Ended).await;
}

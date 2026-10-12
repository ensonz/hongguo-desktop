//! §5.3 spike：平台解码帧 → GPUI `surface()` 上屏最小闭环。
//!
//! 验证三件事：① VT 解码器请求 **'420f'** FullRange NV12 能否成立（gate：
//! gpui 的 Metal shader 断言全区间）；② 进程内 `hongguo_stream::serve()` 按
//! 样本表逐段取明文字节、逐样本喂解码器这条数据面够不够播放；③ 帧经
//! `DecodedFrame`（Send）跨线程进 GPUI 实体、`surface()` 零拷贝上屏的渲染
//! 通路。结论决定 hongguo-playback 引擎（后续目标）的契约形态——spike 本体
//! 届时被引擎取代。
//!
//! 取流编排与 Tauri 壳同源：[`PlayRuntime`]（阶段 0 去耦后 GPUI 可直接构造）
//! + `prepare`，缓存/渐进填充/读者需求全部复用。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Entity, ObjectFit, Window, div, surface};
use hongguo_common::{AppError, AppResult};
use hongguo_media_platform::platform::vt::{DecodedFrame, HevcFrameDecoder};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::play_service::online::{PlayRuntime, prepare};
use tokio::sync::mpsc;

use crate::bridge;

/// 解码线程 → GPUI 实体的消息。帧带自己的显示时间，实体只管「最新一帧
/// 上屏」；失败一次性终结。
enum SpikeMsg {
    Frame(DecodedFrame),
    Failed(String),
    Ended,
}

#[derive(Debug, Clone)]
enum Phase {
    /// 起播准备中（解析档案 → prepare → 读 moov）。
    Loading,
    Playing,
    Failed(String),
    Ended,
}

pub struct SpikePlayer {
    title: String,
    phase: Phase,
    /// 当前上屏帧。替换即释放上一帧的引用（解码侧最多领先一个通道深度）。
    frame: Option<DecodedFrame>,
}

impl SpikePlayer {
    pub fn new(
        state: Arc<AppStateInner>,
        series_id: String,
        title: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<SpikeMsg>(4);
        bridge::spawn(Self::run(state, series_id, tx));
        cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.recv().await {
                // 实体已被丢弃（用户退出播放）→ 停止消费，通道随之关闭，
                // 解码任务下一次 send 失败自然退出
                if this.update(cx, |p, cx| p.on_msg(msg, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            title,
            phase: Phase::Loading,
            frame: None,
        }
    }

    fn on_msg(&mut self, msg: SpikeMsg, cx: &mut Context<Self>) {
        match msg {
            SpikeMsg::Frame(frame) => {
                if matches!(self.phase, Phase::Loading) {
                    self.phase = Phase::Playing;
                }
                self.frame = Some(frame);
                cx.notify();
            }
            SpikeMsg::Failed(e) => {
                self.phase = Phase::Failed(e);
                cx.notify();
            }
            SpikeMsg::Ended => {
                self.phase = Phase::Ended;
                cx.notify();
            }
        }
    }

    /// 整条播放链（tokio 线程）：解析档案 → 取流 → 读样本表 → 逐样本解码 →
    /// 按源时间轴推帧。无音频（引擎目标），节奏先按视频 PTS 走。
    async fn run(state: Arc<AppStateInner>, series_id: String, tx: mpsc::Sender<SpikeMsg>) {
        if let Err(e) = Self::run_inner(&state, &series_id, &tx).await {
            let _ = tx.send(SpikeMsg::Failed(e.to_string())).await;
        }
    }

    async fn run_inner(
        state: &AppStateInner,
        series_id: &str,
        tx: &mpsc::Sender<SpikeMsg>,
    ) -> AppResult<()> {
        let runtime = PlayRuntime::new(crate::tokio().handle().clone(), None);
        let settings = state.settings();
        let env = state.api_env();

        // 剧集档案：本地命中秒回，缺失现场解析（与 Tauri 的 play_prefetch 同口径）
        let series = match state.store.series_by_id(series_id)? {
            Some(s) if !s.episodes.is_empty() => s,
            _ => {
                let s =
                    hongguo_service::series_service::resolver::resolve_series(series_id, &env)
                        .await?;
                state.store.upsert_series(&s)?;
                s
            }
        };
        // 起点对齐 Tauri：看过 >3 秒续那一集，否则第 1 集（集内 seek 是引擎目标）
        let vid_index = match state.store.series_last_position(series_id)? {
            Some((idx, p)) if p.current_time > 3.0 => idx,
            _ => 1,
        };
        let ep = series
            .episodes
            .iter()
            .find(|e| e.vid_index == vid_index)
            .or_else(|| series.episodes.iter().find(|e| e.vid_index == 1));
        let Some(ep) = ep else {
            return Err(AppError::Media("分集表里没有可播的集".into()));
        };
        if ep.vid.trim().is_empty() {
            return Err(AppError::Media("该集没有 vid".into()));
        }
        let vid = ep.vid.clone();
        let progress_key = format!("{series_id}:{}", ep.vid_index);

        // 取流编排：与 Tauri 完全同一段 service（渐进注册 + 后台填充）
        let prepared = prepare(&runtime, &vid, &progress_key, None, settings, &env).await?;
        let def = prepared.definition;

        // 样本表：逐 MB 读头部直到 moov 完整（渐进注册已保证头部就绪，
        // 常规一两个分块内完成）
        let head = read_until_moov(&vid, def).await?;
        let demuxed = hongguo_media::demux::demux_bytes(&head)?;
        let video = demuxed
            .video_track()
            .ok_or_else(|| AppError::Media("流里没有视频轨".into()))?;
        let info = &video.info;
        let (nalus, _length_size) = hongguo_media::hevc::param_nalus_from_head(&head, info)?;
        // **'420f' 验证点**：FullRange 请求建不出会话会在这里报错——那是
        // spike 最重要的输出，直接决定引擎契约（退路 = 逐帧 RGBA）
        let mut decoder = HevcFrameDecoder::open(&nalus, true)?;
        log::info!(
            "[Spike] '420f' 解码会话建立成功（参数集 {} 条，{}x{}）",
            nalus.len(),
            info.width,
            info.height
        );

        let fps = info.average_framerate().unwrap_or(25.0);
        let frame_dur = 1.0 / fps;
        let mut pts = info.sample_pts();
        if pts.is_empty() {
            pts = (0..info.samples.len()).map(|i| i as f64 / fps).collect();
        }

        // 节奏：源 PTS 展开成挂钟时间。serve 是阻塞读（未就绪在条件变量上
        // 等，读者需求会驱动填充优先供这段），放阻塞池，别占 runtime worker。
        let start_at = tokio::time::Instant::now();
        let base = pts.first().copied().unwrap_or(0.0);
        let mut frames_out = 0usize;
        for (i, &(offset, size)) in info.samples.iter().enumerate() {
            let wait = (pts[i] - base).max(0.0);
            tokio::time::sleep_until(start_at + Duration::from_secs_f64(wait)).await;
            let sample = read_range(&vid, def, offset, size).await?;
            for frame in decoder.decode(&sample, pts[i], frame_dur)? {
                frames_out += 1;
                if frames_out == 1 || frames_out.is_multiple_of(100) {
                    log::info!(
                        "[Spike] 第 {frames_out} 帧出画（{}x{}，源 pts {:.3}s）",
                        frame.dims().0,
                        frame.dims().1,
                        frame.pts_secs
                    );
                }
                if tx.send(SpikeMsg::Frame(frame)).await.is_err() {
                    return Ok(()); // 实体已丢弃
                }
            }
        }
        for frame in decoder.finish()? {
            if tx.send(SpikeMsg::Frame(frame)).await.is_err() {
                return Ok(());
            }
        }
        let _ = tx.send(SpikeMsg::Ended).await;
        Ok(())
    }
}

/// 从流里读头部直到样本表可解析（moov 完整）。每段 1MB，16MB 封顶。
async fn read_until_moov(vid: &str, def: u32) -> AppResult<Vec<u8>> {
    let mut head = Vec::new();
    let mut cursor = 0u64;
    for _ in 0..16 {
        let chunk = read_range(vid, def, cursor, cursor + 1024 * 1024 - 1).await?;
        if chunk.is_empty() {
            break;
        }
        cursor += chunk.len() as u64;
        head.extend_from_slice(&chunk);
        if hongguo_media::demux::demux_bytes(&head).is_ok() {
            return Ok(head);
        }
    }
    Err(AppError::Media("头部 16MB 内拿不到完整 moov".into()))
}

/// 按样本表的 `[offset, offset+size)` 取一段明文字节。
///
/// serve 是同步入口：数据未就绪时在条件变量上等（并把需求登记给填充调度
/// 器优先供这段），必须放阻塞池——tokio worker 上阻塞会饿死运行时。
async fn read_range(vid: &str, def: u32, offset: u64, size: u64) -> AppResult<Vec<u8>> {
    let vid = vid.to_string();
    let header = format!("bytes={offset}-{}", offset + size - 1);
    tokio::task::spawn_blocking(move || {
        let (status, _, body) = hongguo_stream::serve(&vid, def, Some(&header))
            .map_err(|e| AppError::Media(format!("取流失败: {e}")))?;
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(AppError::Media(format!(
                "取流区间 {header} 失败（HTTP {status}）"
            )))
        }
    })
    .await
    .map_err(|e| AppError::Internal(format!("阻塞池 join: {e}")))?
}

impl gpui_kit::Render for SpikePlayer {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = cx.theme();
        let stage: gpui_kit::AnyElement = match (&self.phase, &self.frame) {
            (Phase::Failed(msg), _) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(msg.clone()),
                )
                .into_any_element(),
            (Phase::Ended, _) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("播放结束"),
                )
                .into_any_element(),
            (_, Some(frame)) => div()
                .size_full()
                .child(surface(frame.buffer()).object_fit(ObjectFit::Contain))
                .into_any_element(),
            (_, None) => h_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(gpui_kit::rgb(0x000000))
            .text_color(theme.foreground)
            // spike 的标题条固定显示（沉浸流淡出层是阶段 3 的活）
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match &self.phase {
                                Phase::Loading => "准备中…".to_string(),
                                Phase::Playing => {
                                    let (w, h) = self
                                        .frame
                                        .as_ref()
                                        .map(|f| f.dims())
                                        .unwrap_or((0, 0));
                                    format!("播放中 {w}x{h}")
                                }
                                Phase::Failed(_) => "失败".to_string(),
                                Phase::Ended => "结束".to_string(),
                            }),
                    ),
            )
            .child(stage)
    }
}

/// spike 的外壳行：返回按钮 + 播放器实体。由 FeedView 装配。
pub struct SpikeHost {
    pub player: Entity<SpikePlayer>,
}

impl gpui_kit::Render for SpikeHost {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex().px_4().py_2().child(
                    Button::new("spike-back")
                        .label("返回列表")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(SpikeClosed);
                        })),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.player.clone()))
    }
}

/// 用户从 spike 播放返回的事件（FeedView 订阅）。
#[derive(Debug)]
pub struct SpikeClosed;

impl gpui_kit::EventEmitter<SpikeClosed> for SpikeHost {}

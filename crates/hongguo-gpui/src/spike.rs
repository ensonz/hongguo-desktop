//! §5.3 spike 的壳侧视图：hongguo-playback 引擎的集成验证面。
//!
//! 引擎负责解码/音频/同步；这里只做宿主的事——解析剧集 → `prepare` 取流 →
//! `open` 引擎 → `surface()` 把最新帧刷上屏 + 事件驱动状态机。引擎契约
//! （420f NV12、serve 读、事件流）已在引擎 crate 钉死，本模块不再有媒体逻辑。

use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Entity, ObjectFit, Window, div, surface};
use hongguo_common::{AppError, AppResult};
use hongguo_media_platform::platform::vt::DecodedFrame;
use hongguo_playback::{OpenOptions, PlayerEvent, PlayerHandle};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::play_service::online::{PlayRuntime, prepare};
use tokio::sync::mpsc;

use crate::bridge;

#[derive(Debug, Clone)]
enum Phase {
    /// 解析档案 → prepare → 引擎 open（首帧前）。
    Loading,
    Playing,
    Failed(String),
    Ended,
}

pub struct SpikePlayer {
    title: String,
    phase: Phase,
    /// 引擎句柄：在则播放中（Drop 即停）。
    handle: Option<PlayerHandle>,
    /// 渲染用的最新帧快照（Frame 事件时从句柄拉）。
    frame: Option<DecodedFrame>,
    render_count: u64,
}

impl SpikePlayer {
    pub fn new(
        state: Arc<AppStateInner>,
        series_id: String,
        title: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, mut events) = mpsc::channel::<PlayerEvent>(16);
        let boot = bridge::spawn(Self::bootstrap(state, series_id, tx));
        cx.spawn(async move |this, cx| {
            // 起播链：句柄就位即进入 Playing（首帧由事件再确认）
            if let Ok(Ok(handle)) = boot.await {
                let _ = this.update(cx, |p, cx| {
                    p.handle = Some(handle);
                    p.phase = Phase::Playing;
                    cx.notify();
                });
            }
            // 事件循环：实体弃了就退出（引擎随句柄 Drop 停机）
            let mut seen = 0usize;
            while let Some(event) = events.recv().await {
                seen += 1;
                if seen.is_multiple_of(100) {
                    log::debug!("[Spike] 第 {seen} 个事件到达（{event:?}）");
                }
                let cont = this.update(cx, |p, cx| {
                    match event {
                        PlayerEvent::FirstFrame { .. } => p.phase = Phase::Playing,
                        PlayerEvent::Frame => {
                            p.frame = p.handle.as_ref().and_then(|h| h.latest_frame());
                        }
                        PlayerEvent::Ended => p.phase = Phase::Ended,
                        PlayerEvent::Failed(msg) => p.phase = Phase::Failed(msg),
                    }
                    cx.notify();
                });
                if let Err(e) = &cont {
                    log::warn!("[Spike] 实体更新失败，退出事件循环: {e}");
                    break;
                }
            }
        })
        .detach();
        Self {
            title,
            phase: Phase::Loading,
            handle: None,
            frame: None,
            render_count: 0,
        }
    }

    /// 起播链（tokio 线程）：解析剧集 → prepare → 引擎 open。与 Tauri 壳
    /// 同一段服务面（PlayRuntime 阶段 0 去耦后可直接构造）。
    async fn bootstrap(
        state: Arc<AppStateInner>,
        series_id: String,
        events: mpsc::Sender<PlayerEvent>,
    ) -> AppResult<PlayerHandle> {
        let runtime = PlayRuntime::new(crate::tokio().handle().clone(), None);
        let settings = state.settings();
        let env = state.api_env();

        let series = match state.store.series_by_id(&series_id)? {
            Some(s) if !s.episodes.is_empty() => s,
            _ => {
                let s =
                    hongguo_service::series_service::resolver::resolve_series(&series_id, &env)
                        .await?;
                state.store.upsert_series(&s)?;
                s
            }
        };
        // 起点对齐 Tauri：看过 >3 秒续那一集的那个位置
        let (vid_index, start_secs) = match state.store.series_last_position(&series_id)? {
            Some((idx, p)) if p.current_time > 3.0 => (idx, p.current_time),
            _ => (1, 0.0),
        };
        let ep = series
            .episodes
            .iter()
            .find(|e| e.vid_index == vid_index)
            .or_else(|| series.episodes.iter().find(|e| e.vid_index == 1));
        let ep = ep.ok_or_else(|| AppError::Media("分集表里没有可播的集".into()))?;
        if ep.vid.trim().is_empty() {
            return Err(AppError::Media("该集没有 vid".into()));
        }
        let progress_key = format!("{series_id}:{}", ep.vid_index);
        let prepared = prepare(
            &runtime,
            &ep.vid,
            &progress_key,
            None,
            settings,
            &env,
        )
        .await?;
        hongguo_playback::open(OpenOptions {
            vid: ep.vid.clone(),
            definition: prepared.definition,
            start_secs,
            events,
        })
        .await
    }

    fn status_line(&self) -> String {
        match &self.phase {
            Phase::Loading => "准备中…".to_string(),
            Phase::Playing => match self.handle.as_ref() {
                Some(h) => {
                    let (w, hh) = h.dims();
                    format!(
                        "播放中 {w}x{hh} · {:.1}s / {:.1}s",
                        h.position_secs(),
                        h.duration_secs()
                    )
                }
                None => "播放中".to_string(),
            },
            Phase::Failed(_) => "失败".to_string(),
            Phase::Ended => "结束".to_string(),
        }
    }
}

impl gpui_kit::Render for SpikePlayer {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.render_count += 1;
        if self.render_count % 50 == 1 {
            log::debug!(
                "[Spike] render #{}（frame={}）",
                self.render_count,
                self.frame.is_some()
            );
        }
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
                            .child(self.status_line()),
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

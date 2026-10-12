//! 首页推荐流（tab 16）：封面网格 + 换一换。
//!
//! 与 Tauri 的 `recommend_feed` command 同一服务面：`state.api_env()` +
//! [`fetch_recommend_feed`]。封面与 Tauri 的 `hongguo-cover://` 同一条三级
//! 转码阶梯（协议层 [`hongguo_protocol::cover`]：WIC → ffmpeg → 纯软解，
//! 含磁盘缓存）——CDN 给的 HEIC 三家壳都渲染不了，转 JPEG 是统一前提。
//! 会话（sessionId）驱动「换一换」：带上一页会话换一批新剧（hgplayer 同款）。

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{ElementId, Entity, ObjectFit, RenderImage, SharedString, Window, div, img};
use hongguo_api::discover::FeedItem;
use hongguo_api::recommend::fetch_recommend_feed;
use hongguo_service::app_state::AppStateInner;

use crate::bridge;
use crate::spike::{SpikeClosed, SpikeHost, SpikePlayer};

/// 推荐流页的加载状态机（explicit states，coding-guides「Async work」）。
#[derive(Debug, Clone)]
enum Phase {
    /// 首次加载中（无旧数据可展示）。
    Loading,
    /// 加载失败（上次无数据）。`String` 是给人看的失败说明。
    Failed(String),
    /// 有数据（可能正在后台刷新，旧数据保留）。
    Loaded,
}

pub struct FeedView {
    state: Arc<AppStateInner>,
    items: Vec<FeedItem>,
    /// 服务端下发的推荐会话；换一换时带上以排除已见。
    session: String,
    phase: Phase,
    loading: bool,
    /// 已解码的封面位图（series_id → RGBA），加载完成即入表。
    covers: HashMap<String, Arc<RenderImage>>,
    /// 已派发封面加载的 series_id（防重复派发）。
    covers_pending: std::collections::HashSet<String>,
    /// §5.3 spike 播放宿主：点封面进播放，返回事件后清空。
    player: Option<Entity<SpikeHost>>,
}

impl FeedView {
    pub fn new(state: Arc<AppStateInner>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            state,
            items: Vec::new(),
            session: String::new(),
            phase: Phase::Loading,
            loading: false,
            covers: HashMap::new(),
            covers_pending: std::collections::HashSet::new(),
            player: None,
        };
        view.refresh(cx);
        view
    }

    /// 拉一页推荐流。加载中幂等；刷新期间保留旧列表（指南：refresh 尽量
    /// 保住可用数据）。
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        if self.items.is_empty() {
            self.phase = Phase::Loading;
        }
        cx.notify();

        let state = Arc::clone(&self.state);
        let session = self.session.clone();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_recommend_feed("16", &session, 0, &[], &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            this.update(cx, |view, cx| {
                view.loading = false;
                match result {
                    Ok(page) => {
                        log::info!(
                            "[Recommend] GPUI 壳 -> {} 条, 首条: {}",
                            page.items.len(),
                            page.items.first().map(|i| i.title.as_str()).unwrap_or("-")
                        );
                        view.session = page.session_id;
                        view.load_covers(&page.items, cx);
                        // §5.3 spike 脚手架：HONGGUO_SPIKE_AUTO=1 时自动试播
                        // 第一部（无头验证整条解码链；阶段 3 随 spike 一并拆除）
                        let auto = if std::env::var_os("HONGGUO_SPIKE_AUTO").is_some() {
                            page.items.first().map(|i| (i.series_id.clone(), i.title.clone()))
                        } else {
                            None
                        };
                        view.items = page.items;
                        view.phase = Phase::Loaded;
                        if let Some((sid, title)) = auto {
                            view.open_spike(sid, title, cx);
                        }
                    }
                    Err(e) => {
                        log::warn!("[Recommend] GPUI 壳失败: {e}");
                        if view.items.is_empty() {
                            view.phase = Phase::Failed(format!("推荐流加载失败：{e}"));
                        }
                    }
                }
                cx.notify();
            })
        })
        .detach();
    }

    /// 逐张派发封面加载：协议层三级阶梯跑在 tokio 线程（同步 HTTP + 解码
    /// 不占 GPUI 执行器），JPEG 字节回来后在本线程解码位图并入库。
    fn load_covers(&mut self, items: &[FeedItem], cx: &mut Context<Self>) {
        for item in items {
            let sid = item.series_id.clone();
            if !self.covers_pending.insert(sid.clone()) {
                continue;
            }
            let remote = item.cover.clone();
            // acquire_renderable 是同步函数且内部自建 current_thread runtime
            // （http_get），必须走阻塞池——tokio worker 线程上建 runtime 会
            // panic（"Cannot start a runtime from within a runtime"）。
            let rx = bridge::spawn(async move {
                tokio::task::spawn_blocking(move || {
                    hongguo_protocol::cover::acquire_renderable(&remote).map(|(bytes, _ct)| bytes)
                })
                .await
                .expect("阻塞池 join")
            });
            cx.spawn(async move |this, cx| {
                let bytes: Vec<u8> = match rx.await.expect("tokio 桥不会丢弃发送端") {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        this.update(cx, |view, _| {
                            log::warn!("[Cover] GPUI 壳封面加载失败: {e}");
                            view.covers_pending.remove(&sid);
                        })
                        .ok();
                        return;
                    }
                };
                let _ = this.update(cx, |view, cx| match image::load_from_memory(&bytes) {
                    Ok(decoded) => {
                        let frame = image::Frame::new(decoded.to_rgba8());
                        view.covers
                            .insert(sid, Arc::new(RenderImage::new(smallvec::smallvec![frame])));
                        cx.notify();
                    }
                    Err(e) => {
                        log::warn!("[Cover] GPUI 壳解码失败: {e}");
                        view.covers_pending.remove(&sid);
                    }
                });
            })
            .detach();
        }
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let status: SharedString = match (&self.phase, self.loading) {
            (Phase::Loading, _) => "加载中…".into(),
            (Phase::Failed(msg), _) => msg.clone().into(),
            (Phase::Loaded, true) => "刷新中…".into(),
            (Phase::Loaded, false) => format!("{} 部", self.items.len()).into(),
        };
        h_flex()
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_lg().child("首页"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    ),
            )
            .child(
                Button::new("feed-refresh")
                    .label("换一换")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .into_any_element()
    }

    fn render_grid(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        match &self.phase {
            Phase::Loading => h_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element(),
            Phase::Failed(msg) => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(msg.clone()),
                )
                .child(
                    Button::new("feed-retry")
                        .label("重试")
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                )
                .into_any_element(),
            Phase::Loaded if self.items.is_empty() => h_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("暂无内容"),
                )
                .into_any_element(),
            Phase::Loaded => div()
                .id("feed-grid")
                .flex_1()
                .overflow_y_scroll()
                .p_4()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_4()
                        .children(self.items.iter().map(|item| self.render_card(item, cx))),
                )
                .into_any_element(),
        }
    }

    /// 点封面进 spike 播放（§5.3）：建播放器实体并订阅返回事件。
    fn open_spike(&mut self, series_id: String, title: String, cx: &mut Context<Self>) {
        let player = cx.new(|pcx| {
            SpikePlayer::new(Arc::clone(&self.state), series_id.clone(), title.clone(), pcx)
        });
        let host = cx.new(|_| SpikeHost { player });
        cx.subscribe(&host, |this, _, _: &SpikeClosed, cx| {
            this.player = None;
            cx.notify();
        })
        .detach();
        self.player = Some(host);
        cx.notify();
    }

    /// 一张封面卡。点击进 §5.3 spike 播放（首页沉浸流是阶段 3 的正式形态）。
    fn render_card(&self, item: &FeedItem, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        // 封面位图未就绪时以次级底色占位（阶梯是秒级，有磁盘缓存）。
        let cover: gpui_kit::AnyElement = match self.covers.get(&item.series_id) {
            Some(bitmap) => img(gpui_kit::ImageSource::Render(Arc::clone(bitmap)))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => div().size_full().into_any_element(),
        };
        let sid = item.series_id.clone();
        let title = item.title.clone();
        v_flex()
            .w_40()
            .gap_2()
            .child(
                div()
                    .id(ElementId::from(format!("cover-{}", item.series_id)))
                    .overflow_hidden()
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .w_40()
                    .h_48()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_spike(sid.clone(), title.clone(), cx);
                    }))
                    .child(cover),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .truncate()
                    .child(item.title.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{} 集", item.episode_cnt)),
            )
            .into_any_element()
    }
}

impl gpui_kit::Render for FeedView {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = cx.theme();
        // spike 播放中：整个内容区让给播放器（返回后回到网格）
        if let Some(host) = &self.player {
            return v_flex()
                .size_full()
                .bg(theme.background)
                .text_color(theme.foreground)
                .child(host.clone())
                .into_any_element();
        }
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_toolbar(cx))
            .child(self.render_grid(cx))
            .into_any_element()
    }
}

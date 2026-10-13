//! 应用内剧集详情页（Tauri `/detail` 路由的 GPUI 对齐）。
//!
//! 播放页点剧名、互动卡片进入：档案（resolve_series，库缓存优先）+
//! 详情 meta（简介/追剧数/播放量/热度）。首版骨架：返回栏、封面 + 标题 +
//! 计数行 + 简介、播放/继续播放、分集网格（点选即播）。未上线剧（档案无
//! 分集）给「即将上线」降级视图，与 Tauri prefill 语义一致。
//!
//! 页面间不互相持有：加载与播放入口经事件交壳裁决（Shell 订阅
//! [`DetailEvent`] 切目的地/调 feed 起播）。

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::h_flex;
use gpui_kit::component::v_flex;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{App, ElementId, ImageSource, ObjectFit, SharedString, div, img};
use hongguo_common::model::Series;
use hongguo_service::app_state::AppStateInner;

use crate::bridge;
use crate::cover_img;

/// 详情页发往壳的事件（壳订阅后切目的地/起播）。
#[derive(Debug, Clone)]
pub(crate) enum DetailEvent {
    Back,
    Play {
        series_id: String,
        vid_index: u32,
        start_secs: f64,
    },
}

pub(crate) struct DetailView {
    state: Arc<AppStateInner>,
    series_id: String,
    /// 档案（resolve_series：库缓存优先，未命中拉官方接口并落库）。
    series: Option<Arc<Series>>,
    /// 详情 meta：简介、追剧数、全剧播放量、热度值（fetch_series_meta）。
    meta: Option<SeriesMetaBrief>,
    covers: std::collections::HashMap<String, Arc<gpui_kit::RenderImage>>,
    covers_pending: std::collections::HashSet<String>,
    loading: bool,
    /// 简介展开态（超 80 字可展开，Tauri 详情页同款交互）。
    intro_expanded: bool,
}

/// 详情 meta 的页面内投影（只留渲染要用的字段）。
#[derive(Debug, Clone)]
struct SeriesMetaBrief {
    intro: String,
    followed_cnt: i64,
    play_cnt: i64,
    hot_score: i64,
}

impl DetailView {
    pub(crate) fn new(state: Arc<AppStateInner>, _cx: &mut App) -> Self {
        Self {
            state,
            series_id: String::new(),
            series: None,
            meta: None,
            covers: std::collections::HashMap::new(),
            covers_pending: std::collections::HashSet::new(),
            loading: false,
            intro_expanded: false,
        }
    }

    /// 装载一部剧（同剧已装载则跳过；换剧强制重载）。
    pub(crate) fn load(&mut self, series_id: String, cx: &mut Context<Self>) {
        if self.loading || (self.series_id == series_id && self.series.is_some()) {
            return;
        }
        self.series_id = series_id.clone();
        self.series = None;
        self.meta = None;
        self.intro_expanded = false;
        self.loading = true;
        let state = Arc::clone(&self.state);
        let sid = series_id.clone();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            let series = match state.store.series_by_id(&sid)? {
                Some(s) if !s.episodes.is_empty() => s,
                _ => {
                    let s = hongguo_service::series_service::resolver::resolve_series(&sid, &env)
                        .await?;
                    state.store.upsert_series(&s)?;
                    s
                }
            };
            // 装饰性 meta 失败不拦页面（简介/计数给空，Tauri 同口径）
            let meta = hongguo_api::detail::fetch_series_meta(&sid, &env)
                .await
                .ok();
            Ok::<_, hongguo_common::AppError>((series, meta))
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.loading = false;
                match result {
                    Ok((series, meta)) => {
                        if meta.is_some() {
                            view.load_cover(&series.cover, cx);
                        }
                        view.meta = meta.map(|m| SeriesMetaBrief {
                            intro: m.intro,
                            followed_cnt: m.followed_cnt,
                            play_cnt: m.play_cnt,
                            hot_score: m.hot_score,
                        });
                        view.series = Some(Arc::new(series));
                    }
                    Err(e) => {
                        log::warn!("[Detail] {series_id} 档案装载失败: {e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 懒加载封面进本页 covers 表（管线与全屏占位同源，状态各自持有）。
    fn load_cover(&mut self, remote: &str, cx: &mut Context<Self>) {
        if remote.is_empty()
            || self.covers.contains_key(&self.series_id)
            || !self.covers_pending.insert(self.series_id.clone())
        {
            return;
        }
        let sid = self.series_id.clone();
        let rx = cover_img::fetch(remote.to_string());
        cx.spawn(async move |this, cx| {
            let bytes = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| match bytes {
                Ok(b) => match image::load_from_memory(&b) {
                    Ok(decoded) => {
                        view.covers.insert(
                            sid.clone(),
                            Arc::new(crate::image_upload::render_image(decoded.to_rgba8())),
                        );
                        cx.notify();
                    }
                    Err(e) => {
                        log::warn!("[Detail] 封面解码失败: {e}");
                        view.covers_pending.remove(&sid);
                    }
                },
                Err(e) => {
                    log::warn!("[Detail] 封面加载失败: {e}");
                    view.covers_pending.remove(&sid);
                }
            });
        })
        .detach();
    }

    // —— 渲染 ——

    fn render_page(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        if self.loading || self.series.is_none() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(theme.background)
                .text_color(theme.foreground)
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(if self.loading {
                            "加载中…"
                        } else {
                            "暂无内容"
                        }),
                )
                .into_any_element();
        }
        let series = self.series.as_ref().expect("上面已判 none");
        let meta = self.meta.clone();
        let cover = self.covers.get(&self.series_id).cloned();
        let intro = meta.as_ref().map(|m| m.intro.clone()).unwrap_or_default();
        let intro_expandable = intro.chars().count() > 80;
        let expanded = self.intro_expanded;
        // 计数行（Tauri 详情页头部同款口径）
        let mut meta_line: Vec<String> = Vec::new();
        if series.episode_count > 0 {
            meta_line.push(format!("全{}集", series.episode_count));
        }
        if let Some(m) = &meta {
            if m.followed_cnt > 0 {
                meta_line.push(format!("{}人追剧", Self::fmt_count(m.followed_cnt)));
            }
            if m.play_cnt > 0 {
                meta_line.push(format!("{}次播放", Self::fmt_play_count(m.play_cnt)));
            }
            if m.hot_score > 0 {
                meta_line.push(format!("热度{}", Self::fmt_play_count(m.hot_score)));
            }
        }
        // 继续播放的目标集与起点（本地档案 >3 秒续播，service 同一段决策）
        let resume = hongguo_service::play_service::resume::resolve_entry(
            &self.state.store,
            &self.series_id,
            &[],
        )
        .ok();
        let (resume_ep, resume_start) = resume.unwrap_or((1, 0.0));
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .id("detail-page")
            .overflow_y_scroll()
            .child(
                // 返回栏
                h_flex().items_center().gap_2().px_4().py_3().child(
                    Button::new("detail-back")
                        .icon(gpui_kit::assets::IconName::ChevronLeft)
                        .ghost()
                        .small()
                        .label("返回")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(DetailEvent::Back);
                        })),
                ),
            )
            .child(
                div()
                    .max_w(gpui_kit::px(880.0))
                    .mx_auto()
                    .w_full()
                    .px_6()
                    .pb_8()
                    .child(
                        h_flex()
                            .items_start()
                            .gap_5()
                            .child(
                                div()
                                    .h(gpui_kit::px(240.0))
                                    .w(gpui_kit::px(170.0))
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .rounded(gpui_kit::px(10.0))
                                    .bg(theme.secondary)
                                    .children(cover.map(|bitmap| {
                                        img(ImageSource::Render(bitmap))
                                            .size_full()
                                            .object_fit(ObjectFit::Cover)
                                    })),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child(SharedString::from(series.title.clone())),
                                    )
                                    .children((!meta_line.is_empty()).then(|| {
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child(SharedString::from(meta_line.join(" · ")))
                                    }))
                                    .children((!intro.is_empty()).then(|| {
                                        div()
                                            .text_sm()
                                            .line_height(gpui_kit::relative(1.7))
                                            .text_color(theme.muted_foreground)
                                            .when(!expanded, |s| {
                                                s.h(gpui_kit::px(66.0)).overflow_hidden()
                                            })
                                            .child(SharedString::from(intro.clone()))
                                    }))
                                    .children((intro_expandable).then(|| {
                                        div()
                                            .id("detail-intro-toggle")
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .hover(|s| s.text_color(theme.foreground))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.intro_expanded = !this.intro_expanded;
                                                cx.notify();
                                            }))
                                            .child(if expanded { "收起" } else { "展开" })
                                    }))
                                    .child(
                                        Button::new("detail-play")
                                            .label(if resume_start > 3.0 {
                                                format!("继续播放 第{}集", resume_ep)
                                            } else {
                                                "立即播放".to_string()
                                            })
                                            .primary()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let start = if resume_start > 3.0 {
                                                    resume_start
                                                } else {
                                                    0.0
                                                };
                                                cx.emit(DetailEvent::Play {
                                                    series_id: this.series_id.clone(),
                                                    vid_index: resume_ep,
                                                    start_secs: start,
                                                });
                                            })),
                                    ),
                            ),
                    )
                    .child(self.render_episode_grid(cx)),
            )
            .into_any_element()
    }

    /// 分集网格（选集即播；未上线剧 = 空网格 + 降级提示）。
    fn render_episode_grid(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let Some(series) = &self.series else {
            return div().into_any_element();
        };
        if series.episodes.is_empty() {
            return div()
                .mt_6()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("该剧即将上线，敬请期待")
                .into_any_element();
        }
        div()
            .mt_6()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(series.episodes.iter().map(|ep| {
                div()
                    .id(ElementId::from(format!("detail-ep-{}", ep.vid_index)))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(gpui_kit::px(56.0))
                    .h(gpui_kit::px(40.0))
                    .rounded(gpui_kit::px(6.0))
                    .text_sm()
                    .cursor_pointer()
                    .text_color(gpui_kit::rgba(0xe5e5e5ff))
                    .bg(gpui_kit::rgba(0x262626cc))
                    .hover(|s| s.bg(gpui_kit::rgba(0x404040ff)))
                    .on_click({
                        let sid = self.series_id.clone();
                        let vid_index = ep.vid_index;
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(DetailEvent::Play {
                                series_id: sid.clone(),
                                vid_index,
                                start_secs: 0.0,
                            });
                        })
                    })
                    .child(format!("{}", ep.vid_index))
            }))
            .into_any_element()
    }

    /// 计数格式化（万/亿，与互动栏播放量同源口径）。
    fn fmt_count(n: i64) -> String {
        Self::fmt_play_count(n)
    }

    fn fmt_play_count(n: i64) -> String {
        if n >= 100_000_000 {
            return format!("{:.1}亿", n as f64 / 100_000_000.0).replace(".0", "");
        }
        if n >= 10_000 {
            return format!("{:.1}万", n as f64 / 10_000.0).replace(".0", "");
        }
        format!("{n}")
    }
}

impl gpui_kit::Render for DetailView {
    fn render(
        &mut self,
        _window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        self.render_page(cx)
    }
}

impl gpui_kit::EventEmitter<DetailEvent> for DetailView {}

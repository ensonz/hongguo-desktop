//! 排行榜（热度榜，`ranklist_hot_sc`）：名次 + 封面 + 热度信息的列表。
//!
//! 与 Tauri 的 `rank_list` command 同一服务面：`state.api_env()` +
//! [`fetch_rank_ex`]（v2 先落全站热度榜单一维度，筛选维度待后续迭代）。

use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{ElementId, ObjectFit, RenderImage, SharedString, Window, div, img};
use hongguo_api::rank::fetch_rank_ex;
use hongguo_service::app_state::AppStateInner;

use crate::bridge;

/// 排行榜页的加载状态机（与 FeedView 同一套 explicit states）。
#[derive(Debug, Clone)]
enum Phase {
    Loading,
    Failed(String),
    Loaded,
}

pub struct RankView {
    state: Arc<AppStateInner>,
    items: Vec<hongguo_api::rank::RankItem>,
    phase: Phase,
    loading: bool,
    /// 是否已经拉过一次（首次进榜自动加载；失败后靠重试按钮）。
    loaded_once: bool,
    /// 已解码的封面位图（**系列 id** → 位图）。键曾经用名次，结果翻车：
    /// 榜单名次会并列（本机实测榜里有两行同为「5」），按名次做键时第二行
    /// 的封面加载被去重吞掉、渲染又取到第一行那张——那一行的封面就成了
    /// 别的剧（用户看到的「封面不对」）。行列身份一律用系列 id。
    covers: std::collections::HashMap<String, Arc<RenderImage>>,
    covers_pending: std::collections::HashSet<String>,
}

impl RankView {
    pub fn new(state: Arc<AppStateInner>) -> Self {
        Self {
            state,
            items: Vec::new(),
            phase: Phase::Loading,
            loading: false,
            loaded_once: false,
            covers: std::collections::HashMap::new(),
            covers_pending: std::collections::HashSet::new(),
        }
    }

    /// 拉热度榜。加载中幂等；刷新期间保留旧列表。
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        if !self.loaded_once {
            self.phase = Phase::Loading;
        }
        cx.notify();

        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_rank_ex("all", "ranklist_hot_sc", None, 0, "", &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            this.update(cx, |view, cx| {
                view.loading = false;
                view.loaded_once = true;
                match result {
                    Ok(page) => {
                        log::info!("[Rank] GPUI 壳 -> {} 条", page.items.len());
                        view.load_covers(&page.items, cx);
                        view.items = page.items;
                        view.phase = Phase::Loaded;
                    }
                    Err(e) => {
                        log::warn!("[Rank] GPUI 壳失败: {e}");
                        if view.items.is_empty() {
                            view.phase = Phase::Failed(format!("排行榜加载失败：{e}"));
                        }
                    }
                }
                cx.notify();
            })
        })
        .detach();
    }

    /// 逐行派发封面加载（阻塞池跑三级阶梯，与 FeedView 同一套）。
    fn load_covers(&mut self, items: &[hongguo_api::rank::RankItem], cx: &mut Context<Self>) {
        for (index, item) in items.iter().enumerate() {
            // 系列 id 缺失（理论上不该发生）时退回行下标：键必须唯一，
            // 退回名次会重现「两行同键」。
            let key = if item.series_id.is_empty() {
                format!("row{index}")
            } else {
                item.series_id.clone()
            };
            if !self.covers_pending.insert(key.clone()) {
                continue;
            }
            let remote = item.cover.clone();
            let rx = bridge::spawn(async move {
                tokio::task::spawn_blocking(move || {
                    hongguo_protocol::cover::acquire_renderable(&remote).map(|(bytes, _ct)| bytes)
                })
                .await
                .expect("阻塞池 join")
            });
            let rank = item.rank;
            cx.spawn(async move |this, cx| {
                let bytes: Vec<u8> = match rx.await.expect("tokio 桥不会丢弃发送端") {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        this.update(cx, |view, _| {
                            log::warn!("[Rank/Cover] 封面加载失败: {e}");
                            view.covers_pending.remove(&key);
                        })
                        .ok();
                        return;
                    }
                };
                let _ = this.update(cx, |view, cx| match image::load_from_memory(&bytes) {
                    Ok(decoded) => {
                        // 取证开关：HONGGUO_DUMP_COVER=<目录> 时落盘原始字节 +
                        // 解码 PNG（rank 为名，离线对拍用）
                        if let Ok(dir) = std::env::var("HONGGUO_DUMP_COVER") {
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = std::fs::write(
                                std::path::Path::new(&dir).join(format!("rank{rank}.bin")),
                                &bytes,
                            );
                            let rgba = decoded.to_rgba8();
                            let _ = rgba.save(
                                std::path::Path::new(&dir).join(format!("rank{rank}.ours.png")),
                            );
                            let head: Vec<u8> = bytes.iter().take(12).copied().collect();
                            log::info!(
                                "[Rank/Cover] 落盘 rank{rank}（{}B，首字节 {head:02x?}，{}×{}）",
                                bytes.len(),
                                rgba.width(),
                                rgba.height()
                            );
                        }
                        view.covers.insert(
                            key,
                            Arc::new(crate::image_upload::render_image(decoded.to_rgba8())),
                        );
                        cx.notify();
                    }
                    Err(e) => {
                        log::warn!("[Rank/Cover] 解码失败: {e}");
                        view.covers_pending.remove(&key);
                    }
                });
            })
            .detach();
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
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
                    .child(div().text_lg().child("排行榜"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    ),
            )
            .child(
                Button::new("rank-refresh")
                    .label("刷新")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            )
            .into_any_element()
    }

    fn render_list(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
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
                    Button::new("rank-retry")
                        .label("重试")
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
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
                .id("rank-list")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    v_flex().py_2().children(
                        self.items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| self.render_row(item, index, cx)),
                    ),
                )
                .into_any_element(),
        }
    }

    /// 一行榜单：名次 / 封面 / 标题+副题 / 热度。行身份用**系列 id**（同
    /// [`Self::load_covers`] 的口径）：名次会并列，拿名次做 id 会撞。
    fn render_row(
        &self,
        item: &hongguo_api::rank::RankItem,
        index: usize,
        cx: &Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let key = if item.series_id.is_empty() {
            format!("row{index}")
        } else {
            item.series_id.clone()
        };
        let rank_color = if item.rank <= 3 {
            theme.primary
        } else {
            theme.muted_foreground
        };
        h_flex()
            .id(ElementId::from(format!("rank-row-{key}")))
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .hover(|s| s.bg(theme.secondary))
            .child(
                div()
                    .w_8()
                    .text_center()
                    .text_base()
                    .text_color(rank_color)
                    .child(item.rank.to_string()),
            )
            .child(
                div()
                    .w_12()
                    .h_16()
                    .overflow_hidden()
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .flex_shrink_0()
                    .child(match self.covers.get(&key) {
                        Some(bitmap) => img(gpui_kit::ImageSource::Render(Arc::clone(bitmap)))
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .into_any_element(),
                        None => div().size_full().into_any_element(),
                    }),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_1()
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
                            .truncate()
                            .child(item.sub_title.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .flex_shrink_0()
                    .child(if item.rec_text.is_empty() {
                        SharedString::from(String::new())
                    } else {
                        item.rec_text.clone().into()
                    }),
            )
            .into_any_element()
    }

    /// 预览种子（screenshot feature）：灌假榜单与合成封面，跳过网络。
    /// `empty` = 空态分支（Loaded + 空列表，渲染「暂无内容」）。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_preview_seed(&mut self, empty: bool, cx: &mut Context<Self>) {
        self.phase = Phase::Loaded;
        self.loading = false;
        self.loaded_once = true;
        self.items = if empty {
            Vec::new()
        } else {
            crate::preview::fixtures::rank_items()
        };
        for (i, item) in self.items.iter().enumerate() {
            self.covers
                .insert(item.series_id.clone(), crate::preview::fixtures::cover(i));
        }
        cx.notify();
    }
}

impl gpui_kit::Render for RankView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_toolbar(cx))
            .child(self.render_list(cx))
    }
}

//! 下半区 tab 栏 + 选集网格（Tauri Tabs 三触发器同序同计数；选集格 =
//! 序号 + 第 N 集 + 右侧时长角标，续播集描边高亮）。

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::h_flex;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{ElementId, SharedString, div};

use super::{DetailTab, DetailView};

impl DetailView {
    pub(super) fn render_tabs(&self, cx: &Context<Self>) -> Option<gpui_kit::AnyElement> {
        let theme = cx.theme();
        let series = self.series.as_ref()?;
        let episode_total = series.episodes.len() as i64;
        let related_total = self
            .related
            .as_ref()
            .map(|r| r.works.len() as i64 + r.guess.len() as i64)
            .unwrap_or(0);
        let tabs = [
            (DetailTab::Episodes, "选集", episode_total),
            (DetailTab::Reviews, "剧评", self.reviews_total),
            (DetailTab::Related, "相关推荐", related_total),
        ];
        Some(
            v_flex()
                .mt_8()
                .child(
                    h_flex()
                        .gap_6()
                        .border_b_1()
                        .border_color(theme.secondary)
                        .children(tabs.map(|(tab, label, count)| {
                            let active = self.tab == tab;
                            div()
                                .id(ElementId::from(format!("detail-tab-{:?}", tab)))
                                .pb_2()
                                .text_sm()
                                .cursor_pointer()
                                .when(active, |s| {
                                    s.border_b_2()
                                        .border_color(theme.foreground)
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_color(theme.foreground)
                                })
                                .when(!active, |s| s.text_color(theme.muted_foreground))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.tab = tab;
                                    cx.notify();
                                }))
                                .child(if count > 0 {
                                    format!("{label} {count}")
                                } else {
                                    label.to_string()
                                })
                        })),
                )
                .child(match self.tab {
                    DetailTab::Episodes => self.render_episode_grid(cx),
                    DetailTab::Reviews => self.render_reviews(cx),
                    DetailTab::Related => self.render_related(cx),
                })
                .into_any_element(),
        )
    }

    /// 选集网格（Tauri 2/3/4/5 列响应式 → GPUI 固定 4 列网格；卡内构成
    /// 同款：序号 mono 居中 + 第 N 集 + 时长角标右对齐，续播集高亮描边）。
    fn render_episode_grid(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let series = self.series.as_ref().expect("tab 内容仅在就绪态渲染");
        if series.episodes.is_empty() {
            return div()
                .py_10()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("暂无分集信息")
                .into_any_element();
        }
        let (resume_ep, _) = self.resume_entry();
        div()
            .mt_4()
            .grid()
            .grid_cols(4)
            .gap_3()
            .children(series.episodes.iter().map(|ep| {
                let current = ep.vid_index == resume_ep;
                div()
                    .id(ElementId::from(format!("detail-ep-{}", ep.vid_index)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2p5()
                    .rounded(gpui_kit::px(8.0))
                    .border_1()
                    .when(current, |s| {
                        s.border_color(theme.foreground)
                            .text_color(theme.foreground)
                    })
                    .when(!current, |s| {
                        s.border_color(theme.secondary)
                            .text_color(theme.muted_foreground)
                    })
                    .hover(|s| s.border_color(theme.foreground))
                    .cursor_pointer()
                    .on_click({
                        let sid = self.series_id.clone();
                        let vid_index = ep.vid_index;
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(super::DetailEvent::Play {
                                series_id: sid.clone(),
                                vid_index,
                                start_secs: 0.0,
                            });
                        })
                    })
                    .child(
                        div()
                            .w(gpui_kit::px(32.0))
                            .flex_shrink_0()
                            .text_center()
                            .font_family("monospace")
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(format!("{}", ep.vid_index)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .child(SharedString::from(format!("第{}集", ep.vid_index))),
                    )
                    .children((ep.duration > 0).then(|| {
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .font_family("monospace")
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(DetailView::fmt_duration(ep.duration)))
                    }))
            }))
            .into_any_element()
    }
}

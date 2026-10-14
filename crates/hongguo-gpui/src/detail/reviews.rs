//! 剧评 tab：评分摘要 + 分页列表（Tauri ReviewList 的列表侧对齐；
//! 发评框未做——见模块尾注释）。数据走 fetch_series_comments_page
//! （group_type=1 剧级一条线，与播放器单集评论分属不同 group_id）。
//!
//! 未对齐项：发评框（评分 + 表情 + 文本，Tauri ReviewComposer）。GPUI
//! 壳的评论输入基建在首页播放器，跨页复用待输入组件抽共通后补。

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::h_flex;
use gpui_kit::component::v_flex;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{ImageSource, ObjectFit, SharedString, div, img};
use hongguo_api::danmaku::fetch_series_comments_page;

use super::DetailView;
use crate::bridge;

impl DetailView {
    /// 剧评首页（评分摘要取自首页 extra；换剧重置由 load 统一做）。
    pub(super) fn load_reviews_first_page(&mut self, cx: &mut Context<Self>) {
        if self.reviews_loading {
            return;
        }
        self.reviews_loading = true;
        let sid = self.series_id.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_series_comments_page(&sid, "", &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.reviews_loading = false;
                match result {
                    Ok(page) => {
                        view.reviews_total = page.page.total;
                        view.reviews_score = page.score.clone();
                        view.reviews_score_cnt = page.score_cnt;
                        view.reviews_has_more = page.page.has_more;
                        view.reviews_cursor = page.page.next_cursor.clone();
                        view.reviews = Arc::new(page.page.items.clone());
                        for url in page.page.items.iter().map(|c| c.avatar.clone()) {
                            view.load_image(url, cx);
                        }
                        // 评分摘要变了要回头刷新头部统计行
                        cx.notify();
                    }
                    Err(e) => log::warn!("[Detail] 剧评拉取失败: {e}"),
                }
            });
        })
        .detach();
    }

    /// 剧评翻页（相邻页重叠条目按 comment_id 去重，首页评论同款）。
    pub(super) fn load_reviews_next(&mut self, cx: &mut Context<Self>) {
        if self.reviews_loading || !self.reviews_has_more {
            return;
        }
        self.reviews_loading = true;
        let sid = self.series_id.clone();
        let cursor = self.reviews_cursor.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_series_comments_page(&sid, &cursor, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.reviews_loading = false;
                if let Ok(page) = result {
                    view.reviews_has_more = page.page.has_more;
                    view.reviews_cursor = page.page.next_cursor;
                    let mut items = (*view.reviews).clone();
                    for c in page.page.items {
                        if !items.iter().any(|x| x.comment_id == c.comment_id) {
                            view.load_image(c.avatar.clone(), cx);
                            items.push(c);
                        }
                    }
                    view.reviews = Arc::new(items);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_reviews(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        v_flex()
            .mt_4()
            .gap_4()
            // 评分摘要（剧均评分 + 评分人数；空 = 暂无评分整行不渲染）
            .children((!self.reviews_score.is_empty()).then(|| {
                h_flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(gpui_kit::rgba(0xfacc15ff))
                            .child(format!(
                                "{:.1}",
                                self.reviews_score.parse::<f64>().unwrap_or(0.0)
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(if self.reviews_score_cnt > 0 {
                                format!("{}人评分", DetailView::fmt_count(self.reviews_score_cnt))
                            } else {
                                "剧均评分".to_string()
                            }),
                    )
            }))
            // 剧评列表（头像 + 昵称 + 评分行 + 时间 + 正文 + 点赞数；
            // 评分行只有剧评有——单集评论 score 恒空）
            .children(self.reviews.iter().map(|c| {
                let avatar = self.images.get(&c.avatar).cloned();
                h_flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .size_9()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .rounded_full()
                            .bg(theme.secondary)
                            .children(avatar.map(|bitmap| {
                                img(ImageSource::Render(bitmap))
                                    .size_full()
                                    .object_fit(ObjectFit::Cover)
                            })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(SharedString::from(c.user_name.clone())),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .children((!c.score.is_empty()).then(|| {
                                        div()
                                            .px_1p5()
                                            .rounded(gpui_kit::px(4.0))
                                            .text_xs()
                                            .bg(gpui_kit::rgba(0xfacc1526))
                                            .text_color(gpui_kit::rgba(0xfacc15ff))
                                            .child(SharedString::from(format!("{}分", c.score)))
                                    }))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(SharedString::from(
                                                crate::home::fmt::relative_time(c.create_time),
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(gpui_kit::relative(1.6))
                                    .child(SharedString::from(c.text.clone())),
                            )
                            .children((c.digg_count > 0).then(|| {
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(format!("♥ {}", c.digg_count)))
                            })),
                    )
            }))
            .children(self.reviews.is_empty().then(|| {
                div()
                    .py_6()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if self.reviews_loading {
                        "加载中…"
                    } else {
                        "暂无剧评"
                    })
            }))
            // 加载更多（Tauri 是滚动懒加载；GPUI 详情页滚动容器裁判复杂，
            // 按钮等价且省一档哨兵）
            .children((self.reviews_has_more && !self.reviews.is_empty()).then(|| {
                Button::new("detail-reviews-more")
                    .label(if self.reviews_loading {
                        "加载中…"
                    } else {
                        "加载更多剧评"
                    })
                    .secondary()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.load_reviews_next(cx);
                    }))
            }))
            .into_any_element()
    }
}

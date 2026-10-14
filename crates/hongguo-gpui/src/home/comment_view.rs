//! 评论/回复面板的渲染（列表、回复行、删除确认浮层）：数据操作在
//! super::comments，本文件只管画。

#[cfg(feature = "screenshot")]
use std::time::{Duration, Instant};

use gpui_kit::component::Disableable;
use gpui_kit::component::Icon;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Input;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{ElementId, SharedString, div};
use hongguo_api::danmaku::CommentItem;

#[cfg(feature = "screenshot")]
use super::controls::CONTROLS_HOLD_SECS;

impl super::HomeView {
    /// 评论面板（Tauri comment-panel 对齐：右侧滑出，列表 + 发送框；
    /// 回复内联展开；点赞乐观 ±1；表情代码随表情基建对齐补）。
    pub(super) fn render_comment_panel(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let loading = self.comments_loading && self.comments.is_empty();
        let comment_input = self.comment_input.clone();
        let reply_target = self
            .reply_to
            .as_ref()
            .map(|(_, _, name)| name.clone())
            .unwrap_or_default();

        v_flex()
            .id("comment-panel")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(gpui_kit::px(380.0))
            .bg(gpui_kit::rgba(0x171717f2))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            // 滚轮拦下（Tauri comment-panel data-wheel-block 同款）：面板内
            // 滚动是列表自己在滚，不冒泡成舞台切剧
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(
                // 头部：评论 · N + 关闭
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(gpui_kit::rgb(0xffffff))
                            .child(format!("评论 · {}", self.comments_total)),
                    )
                    .child(
                        Button::new("comment-close")
                            .icon(gpui_kit::assets::IconName::X)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.comments_panel_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("comments-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .child(if loading {
                        div()
                            .py_8()
                            .text_center()
                            .text_sm()
                            .text_color(gpui_kit::rgba(0xffffff99))
                            .child("加载中…")
                            .into_any_element()
                    } else if self.comments.is_empty() {
                        div()
                            .py_8()
                            .text_center()
                            .text_sm()
                            .text_color(gpui_kit::rgba(0xffffff99))
                            .child("还没有评论，抢首条")
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_4()
                            .children(self.comments.iter().map(|c| self.render_comment_row(c, cx)))
                            .children(self.comments_has_more.then(|| {
                                div()
                                    .id("comments-more")
                                    .py_2()
                                    .text_center()
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(gpui_kit::rgba(0x60a5faff))
                                    .hover(|s| s.text_color(gpui_kit::rgb(0x93c5fd)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_comments_next(cx);
                                    }))
                                    .child(if self.comments_loading {
                                        "加载中…"
                                    } else {
                                        "加载更多"
                                    })
                            }))
                            .into_any_element()
                    }),
            )
            .child(
                // 底部发送框：回复目标指示 + 输入 + 发送
                v_flex()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(gpui_kit::rgba(0x262626ff))
                    .children((!reply_target.is_empty()).then(|| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgba(0x93c5fdff))
                                    .child(format!("回复 @{reply_target}：")),
                            )
                            .child(
                                div()
                                    .id("reply-cancel")
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(gpui_kit::rgba(0xffffff99))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.reply_to = None;
                                        cx.notify();
                                    }))
                                    .child("取消"),
                            )
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            // 表情按钮（Tauri EmojiPickerButton 同款：开时高亮；
                            // 点击不夺输入框焦点，插入落真实光标处）
                            .child(
                                div()
                                    .id("emoji-btn")
                                    .px_1()
                                    .cursor_pointer()
                                    .text_color(if self.emoji_picker_open {
                                        gpui_kit::rgb(0xffffff)
                                    } else {
                                        gpui_kit::rgba(0xffffff99)
                                    })
                                    .hover(|s| s.text_color(gpui_kit::rgb(0xffffff)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.emoji_picker_open = !this.emoji_picker_open;
                                        cx.notify();
                                    }))
                                    .child(
                                        Icon::new(gpui_kit::assets::IconName::FaceGrinning)
                                            .size_4(),
                                    ),
                            )
                            .children(comment_input.as_ref().map(|input| {
                                Input::new(input).flex_1().h(gpui_kit::px(32.0)).text_xs()
                            }))
                            .child(
                                Button::new("comment-send")
                                    .label(if self.reply_to.is_some() {
                                        "回复"
                                    } else {
                                        "发送"
                                    })
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.reply_to.is_some() {
                                            this.submit_reply(cx);
                                        } else {
                                            this.submit_comment(cx);
                                        }
                                    })),
                            ),
                    ),
            )
            // 表情选择器：点外部收起的捕获层 + 上拉网格（Tauri EmojiPicker
            // 同款：8 列、每格一张表情图、连选不关面板、选完光标处插入）
            .children(self.emoji_picker_open.then(|| {
                div()
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.emoji_picker_open = false;
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }))
            .children(self.emoji_picker_open.then(|| {
                let comment_input = comment_input.clone();
                v_flex()
                    .absolute()
                    .left_3()
                    .bottom(gpui_kit::px(56.0))
                    .w(gpui_kit::px(264.0))
                    .p_2()
                    .rounded(gpui_kit::px(12.0))
                    .bg(gpui_kit::rgba(0x000000d9))
                    .border_1()
                    .border_color(gpui_kit::rgba(0xffffff14))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .id("emoji-grid")
                            .max_h(gpui_kit::px(176.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_wrap()
                            .gap_0p5()
                            .children(crate::danmaku_emoji::EMOJI_FILES.iter().map(|(name, _)| {
                                let code = name.to_string();
                                let img = crate::danmaku_emoji::emoji_by_name(name);
                                div()
                                    .id(gpui_kit::ElementId::from(format!("emoji-{}", code)))
                                    .size_8()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(gpui_kit::px(6.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(gpui_kit::rgba(0xffffff26)))
                                    .on_click({
                                        let input = comment_input.clone();
                                        move |_, window, cx| {
                                            if let Some(input) = &input {
                                                // 光标处插入 [名字]（焦点纪律：
                                                // 点格子不夺焦，选完接着打字）
                                                input.update(cx, |state, cx| {
                                                    state.insert(code.clone(), window, cx);
                                                });
                                            }
                                        }
                                    })
                                    .children(img.map(|img| {
                                        gpui_kit::img(gpui_kit::ImageSource::Render(img))
                                            .size_6()
                                            .object_fit(gpui_kit::ObjectFit::Contain)
                                    }))
                            })),
                    )
                    .into_any_element()
            }))
            .children(self.comment_delete_target.is_some().then(|| {
                // 删除确认浮层（Tauri ConfirmDialog 同款语义：comment/del
                // service_id=4，评论与回复同款；点遮罩=取消）
                let is_reply = self
                    .comment_delete_target
                    .as_ref()
                    .and_then(|(_, p)| p.as_ref())
                    .is_some();
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::rgba(0x00000099))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.cancel_delete(cx)),
                    )
                    .child(
                        v_flex()
                            .w(gpui_kit::px(272.0))
                            .p_4()
                            .gap_2p5()
                            .rounded(gpui_kit::px(12.0))
                            .bg(gpui_kit::rgb(0x262626))
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(gpui_kit::rgb(0xffffff))
                                    .child(if is_reply {
                                        "删除回复"
                                    } else {
                                        "删除评论"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgba(0xffffff99))
                                    .child(if is_reply {
                                        "确定删除这条回复吗？删除后不可恢复。"
                                    } else {
                                        "确定删除这条评论吗？删除后不可恢复。"
                                    }),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_2()
                                    .mt_1()
                                    .child(
                                        Button::new("comment-del-cancel")
                                            .label("取消")
                                            .ghost()
                                            .small()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_delete(cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("comment-del-confirm")
                                            .label(if self.comment_deleting {
                                                "删除中…"
                                            } else {
                                                "删除"
                                            })
                                            .danger()
                                            .small()
                                            .disabled(self.comment_deleting)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.confirm_delete(cx);
                                            })),
                                    ),
                            ),
                    )
            }))
            .into_any_element()
    }

    /// 评论列表一行：头像占位（昵称首字圆形）/昵称/文本/相对时间/♡。
    fn render_comment_row(&self, c: &CommentItem, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let cid = c.comment_id.clone();
        let is_mine = !self.my_user_id().is_empty() && c.user_id == self.my_user_id();
        let digged = self
            .comment_digg_override
            .get(&cid)
            .copied()
            .unwrap_or(c.user_digg);
        let digg_count = c.digg_count;
        let replies = self.replies.get(&cid).cloned();
        let replies_open = self.replies_open.contains(&cid);
        let reply_count = c.reply_count;
        let loading = self.replies_loading.contains(&cid);
        let my_reply_target = self.reply_to.as_ref().is_some_and(|(rc, _, _)| rc == &cid);
        let _ = my_reply_target;

        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .child(self.render_avatar(&c.avatar, &c.user_name, 32.0))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgba(0xffffff99))
                                    .child(SharedString::from(if c.user_name.is_empty() {
                                        "匿名用户".to_string()
                                    } else {
                                        c.user_name.clone()
                                    })),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(gpui_kit::rgba(0xffffffe6))
                                    .child(SharedString::from(c.text.clone())),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_size(gpui_kit::px(11.0))
                                            .text_color(gpui_kit::rgba(0xffffff73))
                                            .child(Self::relative_time(c.create_time)),
                                    )
                                    .child(
                                        // 回复本条评论（打开底部回复输入框；
                                        // Tauri 评论行「回复」同款）
                                        div()
                                            .id(ElementId::from(format!("comment-reply-{cid}")))
                                            .text_size(gpui_kit::px(11.0))
                                            .cursor_pointer()
                                            .text_color(gpui_kit::rgba(0xffffff73))
                                            .hover(|s| s.text_color(gpui_kit::rgba(0xd4d4d8ff)))
                                            .on_click({
                                                let cid = cid.clone();
                                                let name = c.user_name.clone();
                                                cx.listener(move |this, _, _, cx| {
                                                    // 只展开底部回复输入框（Tauri
                                                    // setReplyTarget 同款，不拉回复区）
                                                    this.reply_to =
                                                        Some((cid.clone(), None, name.clone()));
                                                    cx.notify();
                                                })
                                            })
                                            .child("回复"),
                                    )
                                    .children(is_mine.then(|| {
                                        div()
                                            .id(ElementId::from(format!("comment-del-{cid}")))
                                            .text_size(gpui_kit::px(11.0))
                                            .cursor_pointer()
                                            .text_color(gpui_kit::rgba(0xffffff73))
                                            .hover(|s| s.text_color(gpui_kit::rgb(0xf87171)))
                                            .on_click({
                                                let cid = cid.clone();
                                                cx.listener(move |this, _, _, cx| {
                                                    this.request_delete(cid.clone(), None, cx);
                                                })
                                            })
                                            .child("删除")
                                    }))
                                    .children((reply_count > 0).then({
                                        let cid = cid.clone();
                                        move || {
                                            div()
                                                .id(ElementId::from(format!("reply-toggle-{cid}")))
                                                .text_size(gpui_kit::px(11.0))
                                                .cursor_pointer()
                                                .text_color(gpui_kit::rgba(0x60a5faff))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_replies(
                                                        cid.clone(),
                                                        reply_count,
                                                        cx,
                                                    );
                                                }))
                                                .child(if replies_open {
                                                    "收起回复".to_string()
                                                } else {
                                                    format!("展开 {reply_count} 条回复")
                                                })
                                        }
                                    })),
                            ),
                    )
                    .child(
                        // ♡ 点赞（乐观 ±1）
                        div()
                            .id(ElementId::from(format!("comment-digg-{cid}")))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_0p5()
                            .cursor_pointer()
                            .text_color(if digged {
                                gpui_kit::rgb(0xef4444)
                            } else {
                                gpui_kit::rgba(0xffffff99)
                            })
                            .on_click({
                                let cid = cid.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_comment_digg(cid.clone(), cx);
                                })
                            })
                            .child(Icon::new(gpui_kit::assets::IconName::Heart).size_4())
                            .children((digg_count > 0).then(|| {
                                div()
                                    .text_size(gpui_kit::px(10.0))
                                    .child(format!("{digg_count}"))
                            })),
                    ),
            )
            .children(replies_open.then(|| {
                let items = replies.map(|v| (*v).clone()).unwrap_or_default();
                v_flex()
                    .pl_10()
                    .gap_2()
                    .children((loading && items.is_empty()).then(|| {
                        div()
                            .text_xs()
                            .text_color(gpui_kit::rgba(0xffffff73))
                            .child("加载中…")
                    }))
                    .children(items.iter().map(|r| {
                        let digged = self
                            .comment_digg_override
                            .get(&r.reply_id)
                            .copied()
                            .unwrap_or(r.user_digg);
                        let digg_count = r.digg_count;
                        let rid = r.reply_id.clone();
                        let rcid = cid.clone();
                        let rname = r.user_name.clone();
                        let reply_mine =
                            !self.my_user_id().is_empty() && r.user_id == self.my_user_id();
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(self.render_avatar(&r.avatar, &r.user_name, 24.0))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(gpui_kit::rgba(0xffffff99))
                                            .child(SharedString::from(if r.user_name.is_empty() {
                                                "匿名用户".to_string()
                                            } else {
                                                r.user_name.clone()
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(gpui_kit::rgba(0xffffffd9))
                                            .child(SharedString::from(format!(
                                                "回复 @{}：{}",
                                                if r.reply_to_name.is_empty() {
                                                    "原评论".to_string()
                                                } else {
                                                    r.reply_to_name.clone()
                                                },
                                                r.text
                                            ))),
                                    )
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .text_size(gpui_kit::px(10.0))
                                                    .text_color(gpui_kit::rgba(0xffffff73))
                                                    .child(Self::relative_time(r.create_time)),
                                            )
                                            .child(
                                                div()
                                                    .id(ElementId::from(format!("reply-btn-{rid}")))
                                                    .text_size(gpui_kit::px(10.0))
                                                    .cursor_pointer()
                                                    .text_color(gpui_kit::rgba(0xffffff99))
                                                    .hover(|s| {
                                                        s.text_color(gpui_kit::rgb(0xffffff))
                                                    })
                                                    .on_click({
                                                        let rcid = rcid.clone();
                                                        let rid = rid.clone();
                                                        let rname = rname.clone();
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.reply_to = Some((
                                                                rcid.clone(),
                                                                Some(rid.clone()),
                                                                rname.clone(),
                                                            ));
                                                            cx.notify();
                                                        })
                                                    })
                                                    .child("回复"),
                                            )
                                            .children(reply_mine.then(|| {
                                                div()
                                                    .id(ElementId::from(format!("reply-del-{rid}")))
                                                    .text_size(gpui_kit::px(10.0))
                                                    .cursor_pointer()
                                                    .text_color(gpui_kit::rgba(0xffffff73))
                                                    .hover(|s| {
                                                        s.text_color(gpui_kit::rgb(0xf87171))
                                                    })
                                                    .on_click({
                                                        let rid = rid.clone();
                                                        let rcid = rcid.clone();
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.request_delete(
                                                                rid.clone(),
                                                                Some(rcid.clone()),
                                                                cx,
                                                            );
                                                        })
                                                    })
                                                    .child("删除")
                                            }))
                                            .child(
                                                // ♡ 回复点赞（乐观 ±1）
                                                div()
                                                    .id(ElementId::from(format!(
                                                        "reply-digg-{rid}"
                                                    )))
                                                    .flex()
                                                    .items_center()
                                                    .gap_0p5()
                                                    .cursor_pointer()
                                                    .text_color(if digged {
                                                        gpui_kit::rgb(0xef4444)
                                                    } else {
                                                        gpui_kit::rgba(0xffffff99)
                                                    })
                                                    .on_click({
                                                        let rid = rid.clone();
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.toggle_reply_digg(rid.clone(), cx);
                                                        })
                                                    })
                                                    .child(
                                                        Icon::new(
                                                            gpui_kit::assets::IconName::Heart,
                                                        )
                                                        .size_3(),
                                                    )
                                                    .children((digg_count > 0).then(|| {
                                                        div().child(format!("{digg_count}"))
                                                    })),
                                            ),
                                    ),
                            )
                    }))
            }))
            .into_any_element()
    }

    /// 取证辅助（screenshot feature 的 HONGGUO_SHOT_PANEL=comments）：
    /// 打开评论面板与表情选择器（无人值守抓网格 UI）。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_comments(&mut self, cx: &mut Context<Self>) {
        self.comments_panel_open = true;
        self.emoji_picker_open = true;
        self.controls_until = Some(Instant::now() + Duration::from_secs(CONTROLS_HOLD_SECS));
        self.load_comments_first_page(cx);
        cx.notify();
    }
}

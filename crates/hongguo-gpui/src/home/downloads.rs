//! 下载面板：开关与任务快照刷新、整剧入队、任务行渲染（暂停/重试）。

use std::sync::Arc;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::h_flex;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{ElementId, div};
use hongguo_common::model::DownloadTask;

use crate::bridge;

impl super::HomeView {
    /// 开关下载面板；打开时刷新任务快照与完成角标。
    pub(super) fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
        self.downloads_open = !self.downloads_open;
        self.wake_controls(cx);
        self.refresh_downloads(cx);
        cx.notify();
        // 面板开着期间 1s 一刷：暂停时没有 Frame 事件驱动重渲染，
        // 下载进度/完成态会停在旧值（后台下载仍在跑，进度照常变化）
        if self.downloads_open {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    let ok = this
                        .update(cx, |view, cx| {
                            if !view.downloads_open {
                                return;
                            }
                            view.refresh_downloads(cx);
                        })
                        .is_ok();
                    if !ok {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// 刷新下载任务快照与选集浮层的完成角标。
    fn refresh_downloads(&mut self, cx: &mut Context<Self>) {
        let state = Arc::clone(&self.state);
        let sid = self
            .now
            .as_ref()
            .map(|n| n.series_id.clone())
            .unwrap_or_default();
        let rx = bridge::spawn(async move {
            (
                crate::downloads::tasks_of(&state, &sid),
                crate::downloads::completed_vids(&state, &sid),
            )
        });
        cx.spawn(async move |this, cx| {
            let (tasks, done) = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| {
                view.downloads = Arc::new(tasks);
                view.done_vids = Arc::new(done);
            });
        })
        .detach();
    }

    /// 入队当前剧的全部集（已在队列/已完成的自动跳过）。
    fn download_all(&mut self, cx: &mut Context<Self>) {
        let Some(now) = self.now.clone() else {
            return;
        };
        let vids: Vec<u32> = (1..=now.episode_count as u32).collect();
        let added = crate::downloads::enqueue_batch(&self.state, &now.series_id, &vids);
        log::info!("[Home] 下载入队 {added} 集");
        self.refresh_downloads(cx);
        cx.notify();
    }

    /// 下载面板（右侧滑出）：任务列表（集号/状态/进度）+ 全量入队。
    /// 任务快照每次渲染从队列直读——Frame 事件驱动的重渲染顺带刷新进度。
    pub(super) fn render_downloads_panel(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        v_flex()
            .id("downloads-panel")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(gpui_kit::px(360.0))
            .bg(gpui_kit::rgba(0x171717f2))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .child(
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
                            .child("下载任务"),
                    )
                    .child(
                        Button::new("downloads-close")
                            .icon(gpui_kit::assets::IconName::X)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.downloads_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().px_4().pb_2().child(
                    Button::new("downloads-all")
                        .label(format!(
                            "下载本剧全部 {} 集",
                            self.now.as_ref().map(|n| n.episode_count).unwrap_or(0)
                        ))
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.download_all(cx))),
                ),
            )
            .child(
                div()
                    .id("downloads-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .child(if self.downloads.is_empty() {
                        div()
                            .py_8()
                            .text_center()
                            .text_sm()
                            .text_color(gpui_kit::rgba(0xffffff99))
                            .child("暂无下载任务")
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_2()
                            .children(
                                self.downloads
                                    .iter()
                                    .map(|t| self.render_download_row(t, cx)),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// 下载任务一行：集号 + 状态文本 + 进度条 + 行内操作（running → 暂停；
    /// stopped/failed → 重试。Tauri task-row 同款操作面）。
    fn render_download_row(&self, t: &DownloadTask, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let id = t.id.clone();
        let (status_text, percent) = match &t.status {
            hongguo_common::model::TaskStatus::Pending => ("排队中".to_string(), 0.0),
            hongguo_common::model::TaskStatus::Running => {
                (format!("{:.0}%", t.percent()), t.percent())
            }
            hongguo_common::model::TaskStatus::Completed => ("已完成".to_string(), 100.0),
            hongguo_common::model::TaskStatus::Failed => (format!("失败：{}", t.error), 0.0),
            hongguo_common::model::TaskStatus::Stopped => ("已停止".to_string(), 0.0),
        };
        let status_color = match &t.status {
            hongguo_common::model::TaskStatus::Completed => gpui_kit::rgba(0x4ade80ff),
            hongguo_common::model::TaskStatus::Failed => gpui_kit::rgba(0xf87171ff),
            _ => gpui_kit::rgba(0xffffffb3),
        };
        let action: Option<(&str, bool)> = match &t.status {
            hongguo_common::model::TaskStatus::Running => Some(("暂停", false)),
            hongguo_common::model::TaskStatus::Stopped
            | hongguo_common::model::TaskStatus::Failed => Some(("重试", true)),
            _ => None,
        };
        v_flex()
            .gap_1()
            .p_2()
            .rounded(gpui_kit::px(8.0))
            .bg(gpui_kit::rgba(0x26262680))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(gpui_kit::rgba(0xffffffe6))
                            .child(format!("第 {} 集", t.vid_index)),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .children(action.map(|(label, is_retry)| {
                                div()
                                    .id(ElementId::from(format!("download-action-{id}")))
                                    .text_xs()
                                    .cursor_pointer()
                                    .text_color(gpui_kit::rgba(0x60a5faff))
                                    .hover(|s| s.text_color(gpui_kit::rgb(0x93c5fd)))
                                    .on_click({
                                        let id = id.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            if is_retry {
                                                crate::downloads::retry_task(&this.state, &id);
                                            } else {
                                                crate::downloads::stop_task(&this.state, &id);
                                            }
                                            // 立即刷新快照（进度靠 Frame 事件，
                                            // 状态翻转不等下一次渲染）
                                            this.downloads = Arc::new(crate::downloads::tasks_of(
                                                &this.state,
                                                &this
                                                    .now
                                                    .as_ref()
                                                    .map(|n| n.series_id.clone())
                                                    .unwrap_or_default(),
                                            ));
                                            cx.notify();
                                        })
                                    })
                                    .child(label)
                            }))
                            .child(div().text_xs().text_color(status_color).child(status_text)),
                    ),
            )
            .child(
                div()
                    .h_1()
                    .rounded_full()
                    .bg(gpui_kit::rgba(0xffffff26))
                    .child(
                        div()
                            .h_1()
                            .rounded_full()
                            .bg(gpui_kit::rgb(0xef4444))
                            .w(gpui_kit::relative((percent / 100.0) as f32)),
                    ),
            )
            .into_any_element()
    }

    /// 取证辅助（screenshot feature 的 HONGGUO_SHOT_PANEL=downloads）：
    /// 打开下载面板（无人值守抓任务行 UI）。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_downloads(&mut self, cx: &mut Context<Self>) {
        self.downloads_open = true;
        self.downloads = Arc::new(crate::downloads::tasks_of(
            &self.state,
            &self
                .now
                .as_ref()
                .map(|n| n.series_id.clone())
                .unwrap_or_default(),
        ));
        cx.notify();
    }
}

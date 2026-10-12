//! 壳窗口：持久侧栏导航（首页 / 排行）+ 内容区视图切换。
//!
//! 侧栏目的地对齐 Tauri 客户端的信息架构；每个目的地是一个自治视图实体
//! （各自拥有加载状态与数据），壳只管「当前在哪」与「把服务面递下去」。

use std::sync::Arc;

use gpui_kit::component::sidebar::{Sidebar, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Window};
use hongguo_service::app_state::AppStateInner;

use crate::home::HomeView;
use crate::rank::RankView;

/// 侧栏目的地（Tauri 客户端的信息架构；找剧 / 书架 / 播放待后续迭代）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    Feed,
    Rank,
}

/// 壳窗口实体：持有两个视图，切换当前目的地。服务面由视图各自持有。
pub struct Shell {
    dest: Dest,
    feed: Entity<HomeView>,
    rank: Entity<RankView>,
}

impl Shell {
    pub fn new(state: Arc<AppStateInner>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let feed = cx.new(|cx| HomeView::new(state.clone(), window, cx));
        let rank = cx.new(|_cx| RankView::new(state.clone()));
        // 取证开关：HONGGUO_RANK=1 直接开在排行榜页（无头验证封面用）
        let dest = if std::env::var("HONGGUO_RANK").is_ok() {
            rank.update(cx, RankView::load);
            Dest::Rank
        } else {
            Dest::Feed
        };
        Self { dest, feed, rank }
    }

    /// 给 main 的启动回调：把窗口提到前台。
    pub fn activate(&self, cx: &mut App) {
        cx.activate(true);
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
        Sidebar::new("shell-nav").children(vec![
            SidebarMenuItem::new("首页")
                .icon(gpui_kit::assets::IconName::House)
                .active(self.dest == Dest::Feed)
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.dest != Dest::Feed {
                        this.dest = Dest::Feed;
                        cx.notify();
                    }
                })),
            SidebarMenuItem::new("排行榜")
                .icon(gpui_kit::assets::IconName::Trophy)
                .active(self.dest == Dest::Rank)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dest = Dest::Rank;
                    this.rank.update(cx, RankView::load);
                    cx.notify();
                })),
        ])
    }

    fn render_content(&self) -> gpui_kit::AnyElement {
        match self.dest {
            Dest::Feed => self.feed.clone().into_any_element(),
            Dest::Rank => self.rank.clone().into_any_element(),
        }
    }
}

impl gpui_kit::Render for Shell {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("shell")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .items_stretch()
            .child(self.render_sidebar(cx))
            .child(
                // 内容区：吃掉剩余宽度；过载时只允许它收缩（coding-guides
                // 「flex_1 仍可能拒绝收缩」），滚动归视图自己。
                v_flex().flex_1().min_w_0().child(self.render_content()),
            )
    }
}

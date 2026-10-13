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
    state: Arc<AppStateInner>,
    feed: Entity<HomeView>,
    rank: Entity<RankView>,
}

impl Shell {
    pub fn new(state: Arc<AppStateInner>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let feed = cx.new(|cx| HomeView::new(state.clone(), window, cx));
        let rank = cx.new(|_cx| RankView::new(state.clone()));
        // 取证开关：HONGGUO_RANK=1 直接开在排行榜页（无头验证封面用）；
        // 预览进程有自己的目的地种子，不吃这个环境变量（可能残留）
        let dest = if !crate::preview::is_active() && std::env::var("HONGGUO_RANK").is_ok() {
            rank.update(cx, RankView::load);
            Dest::Rank
        } else {
            Dest::Feed
        };
        Self {
            dest,
            state,
            feed,
            rank,
        }
    }

    /// 给 main 的启动回调：把窗口提到前台。
    pub fn activate(&self, cx: &mut App) {
        cx.activate(true);
    }

    /// 取证辅助（screenshot feature）：暂停播放 + 拉长控制条可见窗，让
    /// 无人值守抓帧能拍到控制条/互动栏。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_pause_player(&self, cx: &mut App) {
        self.feed.update(cx, |home, cx| home.debug_pause_player(cx));
    }

    /// 取证辅助（screenshot feature）：打开下载面板。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_downloads(&self, cx: &mut App) {
        self.feed
            .update(cx, |home, cx| home.debug_open_downloads(cx));
    }

    /// 取证辅助（screenshot feature）：打开选集浮层系列剧 tab。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_picker(&self, cx: &mut App) {
        self.feed.update(cx, |home, cx| home.debug_open_picker(cx));
    }

    /// 取证辅助（screenshot feature）：打开评论面板与表情选择器。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_comments(&self, cx: &mut App) {
        self.feed
            .update(cx, |home, cx| home.debug_open_comments(cx));
    }

    /// 取证辅助（screenshot feature）：打开登录面板。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_login(&self, cx: &mut App) {
        self.feed.update(cx, |home, cx| home.debug_open_login(cx));
    }

    /// 预览种子分发（screenshot feature）：rank 场景切到排行榜并灌假榜单，
    /// 其余全走首页。未知场景键的错误向上传（矩阵写错键名启动即报，
    /// 开发工具宁可显性失败）。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_preview_seed(
        &mut self,
        scene: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if scene.starts_with("rank") {
            self.dest = Dest::Rank;
            self.rank.update(cx, |rank, cx| {
                rank.debug_preview_seed(scene == "rank-empty", cx)
            });
        } else {
            self.dest = Dest::Feed;
            self.feed
                .update(cx, |home, cx| home.debug_preview_seed(scene, window, cx))?;
        }
        cx.notify();
        Ok(())
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
        let logged_in = self.state.settings().account.is_some();
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
            // 登录入口（未登录打开登录面板，已登录直接退出；评论/删除/
            // 预约/点赞全靠登录态）
            SidebarMenuItem::new(if logged_in { "退出登录" } else { "登录" })
                .icon(gpui_kit::assets::IconName::User)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if logged_in {
                        this.feed.update(cx, |home, cx| home.login_logout(cx));
                    } else {
                        this.feed.update(cx, |home, cx| home.toggle_login_panel(cx));
                    }
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
            // 小屏播放时侧栏藏掉（480×270 装不下，Tauri use-mini-window
            // 「侧栏隐藏」同款；状态见 crate::mini）
            .children((!crate::mini::is_active()).then(|| self.render_sidebar(cx)))
            .child(
                // 内容区：吃掉剩余宽度；过载时只允许它收缩（coding-guides
                // 「flex_1 仍可能拒绝收缩」），滚动归视图自己。
                v_flex().flex_1().min_w_0().child(self.render_content()),
            )
    }
}

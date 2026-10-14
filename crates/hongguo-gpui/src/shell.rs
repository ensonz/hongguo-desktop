//! 壳窗口：持久侧栏导航 + 顶栏 + 内容区视图切换（Tauri `app-shell` /
//! `app-sidebar` 的 GPUI 对齐形态）。
//!
//! 侧栏目的地对齐 Tauri 客户端的信息架构（12 项）：已有页（首页沉浸流 /
//! 排行榜）接真数据，未实现页给「建设中」空态——形态先行，页面逐个补。
//! 可折叠：展开 224px 图标+文字，折叠 56px 纯图标（tooltip 出标签，
//! Tauri collapsed 同款）。贴底账户区（登录面板复用首页的）+ 折叠钮。
//!
//! 顶栏用组件库 TitleBar 拉到 44px：应用名居左，**分类 tab 居中**（首页
//! 三个 tab，Tauri 的 portal 插槽同位），置顶 + 窗口钮（组件自动附）居右。

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::v_flex;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{App, ElementId, Entity, Window, div};
use hongguo_service::app_state::AppStateInner;

use crate::detail::{DetailEvent, DetailView};
use crate::home::HomeEvent;
use crate::home::HomeView;
use crate::pages::merge::MergeView;
use crate::pages::liked::{LikedEvent, LikedView};
use crate::pages::collections::{CollectionEvent, CollectionView};
use crate::pages::rank::RankView;
use crate::pages::storage::StorageView;

/// 侧栏导航目的地（与 Tauri `app-sidebar.tsx` 的 NAV_ITEMS 逐项同序）。
/// 播放不在列：它由各页「播放/继续播放」进入，首页本身就是沉浸流。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dest {
    Home,
    Browse,
    Rank,
    New,
    History,
    Collections,
    Liked,
    Reservations,
    Tasks,
    Merge,
    Storage,
    Settings,
    /// 应用内详情页（不在侧栏：播放页点剧名 / 播放入口进入）。
    Detail,
}

impl Dest {
    const ALL: [Dest; 12] = [
        Dest::Home,
        Dest::Browse,
        Dest::Rank,
        Dest::New,
        Dest::History,
        Dest::Collections,
        Dest::Liked,
        Dest::Reservations,
        Dest::Tasks,
        Dest::Merge,
        Dest::Storage,
        Dest::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Dest::Home => "首页",
            Dest::Browse => "找剧",
            Dest::Rank => "排行榜",
            Dest::New => "新剧",
            Dest::History => "历史",
            Dest::Collections => "收藏",
            Dest::Liked => "点赞",
            Dest::Reservations => "预约",
            Dest::Tasks => "下载管理",
            Dest::Merge => "一键合并",
            Dest::Storage => "磁盘清理",
            Dest::Settings => "设置",
            // 不在侧栏（播放页进入），仅兜匹配
            Dest::Detail => "详情",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Dest::Home => IconName::Flame,
            Dest::Browse => IconName::Compass,
            Dest::Rank => IconName::Trophy,
            Dest::New => IconName::Sparkles,
            Dest::History => IconName::ClockArrowDown,
            Dest::Collections => IconName::Star,
            Dest::Liked => IconName::Heart,
            Dest::Reservations => IconName::BellRing,
            Dest::Tasks => IconName::ListChecks,
            Dest::Merge => IconName::Combine,
            Dest::Storage => IconName::HardDrive,
            Dest::Settings => IconName::Settings,
            Dest::Detail => IconName::Info,
        }
    }

    /// 已有真数据视图的目的地（其余给建设中空态）。
    fn view(self) -> Option<View> {
        match self {
            Dest::Home => Some(View::Home),
            Dest::Rank => Some(View::Rank),
            Dest::Liked => Some(View::Liked),
            Dest::Collections => Some(View::Collections),
            Dest::Storage => Some(View::Storage),
            Dest::Merge => Some(View::Merge),
            Dest::Detail => Some(View::Detail),
            _ => None,
        }
    }
}

/// 壳内视图（Dest::view 的落点；Detail 不在侧栏，由播放页点剧名进入）。
enum View {
    Home,
    Rank,
    Liked,
    Collections,
    Storage,
    Merge,
    Detail,
}

/// 壳窗口实体：持有各视图实体，切换当前目的地。服务面由视图各自持有。
pub struct Shell {
    dest: Dest,
    state: Arc<AppStateInner>,
    sidebar_collapsed: bool,
    feed: Entity<HomeView>,
    rank: Entity<RankView>,
    liked: Entity<LikedView>,
    collections: Entity<CollectionView>,
    storage: Entity<StorageView>,
    merge: Entity<MergeView>,
    detail: Option<Entity<DetailView>>,
    /// 详情页当前加载的剧（同剧重复点击不重载）。
    detail_series: Option<String>,
}

impl Shell {
    pub fn new(state: Arc<AppStateInner>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let feed = cx.new(|cx| HomeView::new(state.clone(), window, cx));
        let rank = cx.new(|_cx| RankView::new(state.clone()));
        let liked = cx.new(|cx| LikedView::new(state.clone(), window, cx));
        let collections = cx.new(|cx| CollectionView::new(state.clone(), window, cx));
        let storage = cx.new(|cx| StorageView::new(state.clone(), window, cx));
        let merge = cx.new(|cx| MergeView::new(state.clone(), window, cx));
        // 播放页 → 详情页：HomeView 发事件，壳切目的地（页面间不互相持有）
        cx.subscribe(&feed, |this, _, event: &HomeEvent, cx| match event {
            HomeEvent::OpenDetail(series_id) => this.open_detail(series_id.clone(), cx),
        })
        .detach();
        // 点赞/收藏卡「继续播放」：壳切回首页沉浸流起播（与详情页同通道）
        cx.subscribe(&liked, |this, _, event: &LikedEvent, cx| {
            let LikedEvent::Play {
                series_id,
                vid_index,
                start_secs,
            } = event.clone();
            this.start_play(series_id, vid_index, start_secs, cx);
        })
        .detach();
        cx.subscribe(&collections, |this, _, event: &CollectionEvent, cx| {
            let CollectionEvent::Play {
                series_id,
                vid_index,
                start_secs,
            } = event.clone();
            this.start_play(series_id, vid_index, start_secs, cx);
        })
        .detach();
        // 取证开关：HONGGUO_RANK=1 直接开在排行榜页（无头验证封面用）；
        // HONGGUO_TAB=36/39 直接开在漫剧/真人源（tab 数据链取证用）。
        // 预览进程有自己的目的地种子，不吃这些环境变量（可能残留）
        let tab_source = (!crate::preview::is_active())
            .then(|| std::env::var("HONGGUO_TAB").ok())
            .flatten()
            .and_then(|v| match v.as_str() {
                "36" => Some(crate::home::Source::Comic),
                "39" => Some(crate::home::Source::Human),
                _ => None,
            });
        let dest = if !crate::preview::is_active() && std::env::var("HONGGUO_RANK").is_ok() {
            rank.update(cx, RankView::load);
            Dest::Rank
        } else {
            if let Some(source) = tab_source {
                feed.update(cx, |home, cx| home.switch_source(source, cx));
            }
            Dest::Home
        };
        Self {
            dest,
            state,
            sidebar_collapsed: false,
            feed,
            rank,
            liked,
            collections,
            storage,
            merge,
            detail: None,
            detail_series: None,
        }
    }

    /// 页面「继续播放」的统一入口：切回首页沉浸流起播（详情页播放同通道，
    /// 页面间不互相持有）。
    fn start_play(
        &mut self,
        series_id: String,
        vid_index: u32,
        start_secs: f64,
        cx: &mut Context<Self>,
    ) {
        self.dest = Dest::Home;
        self.feed.update(cx, |home, cx| {
            home.play_from_detail(series_id, vid_index, start_secs, cx)
        });
        cx.notify();
    }

    /// 打开应用内详情页（播放页点剧名）：同剧重复点击不重载。
    fn open_detail(&mut self, series_id: String, cx: &mut Context<Self>) {
        if self.dest == Dest::Detail && self.detail_series.as_deref() == Some(&series_id) {
            return;
        }
        let view = match &self.detail {
            Some(v) => v.clone(),
            None => {
                let v = cx.new(|cx| DetailView::new(self.state.clone(), cx));
                // 详情页事件（返回/播放）→ 壳裁决目的地与起播
                cx.subscribe(&v, |this, _, event: &DetailEvent, cx| {
                    this.on_detail_event(event.clone(), cx);
                })
                .detach();
                self.detail = Some(v.clone());
                v
            }
        };
        view.update(cx, |detail, cx| detail.load(series_id.clone(), cx));
        self.detail_series = Some(series_id);
        self.dest = Dest::Detail;
        cx.notify();
    }

    /// 首页当前 tab 源（预览交互探针读，断言切源生效）。
    #[cfg(feature = "screenshot")]
    pub(crate) fn active_source(&self, cx: &App) -> crate::home::Source {
        self.feed.read(cx).active_source()
    }

    /// 详情页的返回/播放入口（DetailView 发事件，壳裁决目的地）。
    fn on_detail_event(&mut self, event: DetailEvent, cx: &mut Context<Self>) {
        match event {
            DetailEvent::Back => {
                self.dest = Dest::Home;
                cx.notify();
            }
            DetailEvent::Play {
                series_id,
                vid_index,
                start_secs,
            } => {
                self.dest = Dest::Home;
                self.feed.update(cx, |home, cx| {
                    home.play_from_detail(series_id, vid_index, start_secs, cx)
                });
                cx.notify();
            }
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
        } else if scene.starts_with("storage") {
            self.dest = Dest::Storage;
            self.storage
                .update(cx, |view, cx| view.debug_preview_seed(scene, cx));
        } else if scene.starts_with("liked") {
            self.dest = Dest::Liked;
            self.liked
                .update(cx, |view, cx| view.debug_preview_seed(scene, cx));
        } else if scene.starts_with("collections") {
            self.dest = Dest::Collections;
            self.collections
                .update(cx, |view, cx| view.debug_preview_seed(scene, cx));
        } else if scene.starts_with("merge") {
            self.dest = Dest::Merge;
            self.merge
                .update(cx, |view, cx| view.debug_preview_seed(scene, window, cx));
        } else if scene.starts_with("detail") {
            // 详情页场景：确保实体在（不走网络 load，直接灌种子）
            self.dest = Dest::Detail;
            if self.detail.is_none() {
                let v = cx.new(|cx| DetailView::new(self.state.clone(), cx));
                cx.subscribe(&v, |this, _, event: &DetailEvent, cx| {
                    this.on_detail_event(event.clone(), cx);
                })
                .detach();
                self.detail = Some(v);
            }
            if let Some(detail) = &self.detail {
                detail.update(cx, |view, cx| view.debug_preview_seed(scene, cx));
            }
            self.detail_series = Some("7100000".into());
        } else {
            self.dest = Dest::Home;
            self.feed
                .update(cx, |home, cx| home.debug_preview_seed(scene, window, cx))?;
        }
        cx.notify();
        Ok(())
    }

    // —— 渲染 ——

    /// 顶栏（组件库 TitleBar）：应用名居左，窗口钮（组件在非 macOS 自动
    /// 附上并标 WindowControlArea）居右。**不放任何自绘可点控件**——组件
    /// 把整条 bar 标成 Drag 区，落进去的点击都会被原生命中测试判成拖拽
    /// 吞掉（置顶钮上一版就栽在这）；它需要的入口都放在舞台/控制栏里。
    fn render_title_bar(&self) -> gpui_kit::AnyElement {
        gpui_kit::component::TitleBar::new()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("红果"),
            )
            .into_any_element()
    }

    /// 侧栏（Tauri app-sidebar 对齐）：12 目的地，展开 224px 图标+文字、
    /// 折叠 56px 纯图标（tooltip 出标签）；贴底账户区 + 折叠钮。
    fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let collapsed = self.sidebar_collapsed;
        let logged_in = self.state.settings().account.is_some();
        let item = |dest: Dest| {
            let active = self.dest == dest;
            let row = div()
                .id(ElementId::from(format!("nav-{}", dest.label())))
                .flex()
                .items_center()
                .gap_3()
                .rounded_md()
                .px_3()
                .py(gpui_kit::px(7.0))
                .text_sm()
                .cursor_pointer()
                .when(collapsed, |s| s.justify_center().px_0())
                .text_color(if active {
                    theme.foreground
                } else {
                    theme.muted_foreground
                })
                .when(active, |s| s.bg(theme.secondary))
                .hover(|s| s.bg(theme.secondary_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.dest == dest {
                        return;
                    }
                    this.dest = dest;
                    match dest {
                        Dest::Rank => this.rank.update(cx, RankView::load),
                        Dest::Liked => this.liked.update(cx, LikedView::load),
                        Dest::Collections => {
                            this.collections.update(cx, CollectionView::load)
                        }
                        Dest::Storage => this.storage.update(cx, StorageView::load),
                        Dest::Merge => this.merge.update(cx, MergeView::load),
                        _ => {}
                    }
                    cx.notify();
                }))
                .child(
                    gpui_kit::component::Icon::new(dest.icon())
                        .small()
                        .text_color(if active {
                            theme.foreground
                        } else {
                            theme.muted_foreground
                        }),
                )
                .when(!collapsed, |s| s.child(dest.label()));
            if collapsed {
                row.tooltip(move |window, cx| Tooltip::new(dest.label()).build(window, cx))
            } else {
                row
            }
        };
        v_flex()
            .w(gpui_kit::px(if collapsed { 56.0 } else { 224.0 }))
            .h_full()
            .flex_shrink_0()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .p_2()
            .gap_1()
            .id("shell-sidebar")
            .overflow_y_scroll()
            .children(Dest::ALL.map(item))
            // 贴底账户区（Tauri AccountButton 同位）：未登录开登录面板，
            // 已登录直接退出（GPUI 无设置页，退出入口沿用在侧）
            .child(
                div()
                    .border_t_1()
                    .border_color(theme.border)
                    .pt_2()
                    .mt_auto()
                    .child(
                        div()
                            .id("nav-account")
                            .flex()
                            .items_center()
                            .gap_3()
                            .rounded_md()
                            .px_3()
                            .py(gpui_kit::px(7.0))
                            .text_sm()
                            .cursor_pointer()
                            .text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.secondary_hover))
                            .when(collapsed, |s| s.justify_center().px_0())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.feed.update(cx, |home, cx| {
                                    if logged_in {
                                        home.login_logout(cx);
                                    } else {
                                        home.toggle_login_panel(cx);
                                    }
                                });
                            }))
                            .child(
                                gpui_kit::component::Icon::new(IconName::UserRound)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .when(!collapsed, |s| {
                                s.child(if logged_in { "退出登录" } else { "登录" })
                            }),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(theme.border)
                    .pt_2()
                    .child(if collapsed {
                        Button::new("sidebar-expand")
                            .icon(IconName::PanelLeft)
                            .ghost()
                            .small()
                            .tooltip("展开")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_collapsed = false;
                                cx.notify();
                            }))
                            .into_any_element()
                    } else {
                        Button::new("sidebar-collapse")
                            .icon(IconName::PanelLeftClose)
                            .ghost()
                            .small()
                            .label("收起")
                            .tooltip("收起")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_collapsed = true;
                                cx.notify();
                            }))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// 未实现目的地的空态（本轮决策：全列出 + 建设中空态，页面逐个补）。
    fn render_stub(&self, dest: Dest, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                gpui_kit::component::Icon::new(dest.icon())
                    .with_size(gpui_kit::px(40.0))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!("「{}」页面建设中", dest.label())),
            )
            .into_any_element()
    }

    fn render_content(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        match self.dest.view() {
            Some(View::Home) => self.feed.clone().into_any_element(),
            Some(View::Rank) => self.rank.clone().into_any_element(),
            Some(View::Liked) => self.liked.clone().into_any_element(),
            Some(View::Collections) => self.collections.clone().into_any_element(),
            Some(View::Storage) => self.storage.clone().into_any_element(),
            Some(View::Merge) => self.merge.clone().into_any_element(),
            Some(View::Detail) => self
                .detail
                .as_ref()
                .expect("Detail 目的地必有实体")
                .clone()
                .into_any_element(),
            None => self.render_stub(self.dest, cx),
        }
    }
}

impl gpui_kit::Render for Shell {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        // chrome 全藏的两种形态：小屏播放（480×270 纯播放器，Tauri 小屏
        // 同款，拖窗走播放页拖拽条）与播放器沉浸全屏（播放器全屏按钮/F
        // 进入；顶栏最大化钮是另一档「应用全屏」，chrome 保留，对齐 Tauri）
        let in_mini = crate::mini::is_active();
        let immersive = self.feed.read(cx).immersive_fullscreen();
        let chrome_hidden = in_mini || immersive;
        // 先建好所有吃 cx 的闭包/子渲染，再取 theme（theme 借 cx，两者不能
        // 交叠——闭包里要 &mut Context）
        let title_bar = (!chrome_hidden).then(|| self.render_title_bar());
        let sidebar = (!chrome_hidden).then(|| self.render_sidebar(cx));
        let content = self.render_content(cx);
        let theme = cx.theme();
        v_flex()
            .id("shell")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .children(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    // 小屏/沉浸全屏时侧栏藏掉（视频区吃满，播放跟随窗口）
                    .children(sidebar)
                    .child(
                        // 内容区：吃掉剩余宽度；过载时只允许它收缩（coding-guides
                        // 「flex_1 仍可能拒绝收缩」），滚动归视图自己。
                        v_flex().flex_1().min_w_0().child(content),
                    ),
            )
    }
}

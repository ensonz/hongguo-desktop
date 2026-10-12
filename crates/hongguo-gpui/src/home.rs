//! 首页：沉浸式播放流（Tauri `home-page.tsx` 的 GPUI 对齐实现）。
//!
//! 视频铺满整页：滚轮在当前 tab 源内切上一部/下一部剧，剧名/集数/热度叠加
//! 在画面左下，切剧间隙封面占位。三个 tab（推荐 16 / 漫剧 36 / 真人 39）
//! 来自书城 cell 换一换的同一接口，**重复点当前 tab = 换一批**（重开会话、
//! 游标归零）；推荐 tab 把「你的内容」（云端历史 + 书架）种子排前，同一
//! 个号打开就是你的剧（对齐第三方与 Tauri 端）。
//!
//! 播放经 [`hongguo_playback`] 引擎：起点决策（本地档案 >3 秒续播，云端
//! 历史兜底）、5 秒节流落盘、首帧出画预取下一部的流、自动连播（末集接
//! 流内下一部——Tauri 的剧终接力跨季/换批语义后续对齐）。取流编排与
//! Tauri 同一段 service（[`PlayRuntime`]）。
//!
//! 对齐取舍：↑↓ 切剧与控制栏随控制层目标落地；「回首页 5 分钟换一批」
//! 的过期重开未做（换一批入口本身已对齐）。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    ElementId, ImageSource, ObjectFit, RenderImage, SharedString, Window, div, img, surface,
};
use hongguo_api::discover::FeedItem;
use hongguo_api::history::{WatchHistoryItem, fetch_watch_history};
use hongguo_api::interact::fetch_bookshelf;
use hongguo_api::recommend::fetch_recommend_feed;
use hongguo_common::AppResult;
use hongguo_media_platform::platform::vt::DecodedFrame;
use hongguo_playback::{OpenOptions, PlayerEvent, PlayerHandle};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::play_service::online::{PlayRuntime, prefetch_stream, prepare};
use hongguo_service::play_service::position;
use tokio::sync::mpsc;

use crate::bridge;

// ---------------------------------------------------------------- 数据面

/// 沉浸流的三个 tab（tab_type 即书城 cell 换一换的契约，见 docs 抓包文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Feed,
    Comic,
    Human,
}

impl Source {
    const ALL: [Source; 3] = [Source::Feed, Source::Comic, Source::Human];

    fn tab_type(self) -> &'static str {
        match self {
            Source::Feed => "16",
            Source::Comic => "36",
            Source::Human => "39",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Source::Feed => "推荐",
            Source::Comic => "漫剧",
            Source::Human => "真人",
        }
    }
}

/// 沉浸流只消费这几个身份/展示字段（feed 条目、种子、在播钉住三源合一）。
#[derive(Debug, Clone, Default)]
struct StreamItem {
    series_id: String,
    title: String,
    cover: String,
    /// 横版封面优先做全屏占位（竖版拉满横屏会糊，Tauri 同口径）。
    horiz_cover: String,
    heat_text: String,
    season_tag: String,
    badge: String,
}

impl From<&FeedItem> for StreamItem {
    fn from(i: &FeedItem) -> Self {
        Self {
            series_id: i.series_id.clone(),
            title: i.title.clone(),
            cover: i.cover.clone(),
            horiz_cover: i.horiz_cover.clone(),
            heat_text: i.heat_text.clone(),
            season_tag: i.season_tag.clone(),
            badge: i.badge.clone(),
        }
    }
}

/// 单个 tab 源的数据（各自持有会话与游标，切 tab 回来还在原位）。
#[derive(Default)]
struct SourceFeed {
    items: Vec<StreamItem>,
    session: String,
    offset: i64,
    has_more: bool,
    /// 首批/换一批在拉（此刻画面还无剧可播）。
    loading: bool,
    fetching_more: bool,
    failed: Option<String>,
}

/// 当前在播的剧（overlay 与进度落盘的身份）。
///
/// 展示标记（热度/季角标/运营角标）在**起播时**从流条目上抄一份：档案接口
/// 没有这几个字段，而叠加层必须跟着「在播的那部」走——跟游标走就会在换批/
/// 钉住条目位移时错位到相邻剧（Tauri 的 overlayMeta 也是随目标传下去的）。
#[derive(Debug, Clone)]
struct NowPlaying {
    series_id: String,
    title: String,
    vid_index: u32,
    episode_count: usize,
    heat_text: String,
    season_tag: String,
    badge: String,
}

// ---------------------------------------------------------------- 视图

pub struct HomeView {
    state: Arc<AppStateInner>,
    active: Source,
    feeds: HashMap<Source, SourceFeed>,
    indexes: HashMap<Source, usize>,

    /// 已解码封面（series_id → RGBA）：切剧间隙的全屏占位。
    covers: HashMap<String, Arc<RenderImage>>,
    covers_pending: HashSet<String>,

    // —— 播放态 ——
    /// 引擎出生纪元：每次切剧 +1，陈旧事件/任务按它丢弃。
    epoch: u64,
    player: Option<PlayerHandle>,
    now: Option<NowPlaying>,
    switching: bool,
    /// 首帧出画（收封面占位）。
    frame_ready: bool,
    frame: Option<DecodedFrame>,
    /// 切换目标（switching 时 overlay 显示它的封面与标题）。
    pending_title: Option<String>,

    wheel_at: Option<Instant>,
    last_save: Instant,
    /// 已流预取（每部一次，失败不算数允许重试）。
    stream_prefetched: HashSet<String>,
    /// 已预取过分集档案的剧。
    archives_prefetched: HashSet<String>,
    /// 云端历史：推荐 tab 种子 + 起点决策兜底（启动拉一次）。
    history: Option<Vec<WatchHistoryItem>>,
    history_pending: bool,
    /// 「你的内容」种子（云端历史 + 书架）：独立于流缓存持有，换一批/翻页
    /// 都不动它（Tauri 的 seedItems memo 同款）。混进流数组会被后续
    /// `load_feed` 整体覆盖、或在头部插入后与流内同条目重影。
    seeds: Vec<StreamItem>,

    /// 舞台两层（封面占位层 / 视频槽）上次报上来的尺寸：图形路径没有视觉
    /// 测试，这行自检日志是唯一的哨兵——视频槽必须与封面层同为整块舞台，
    /// 不一致（或掉成 0×0）就是布局回归，不能只靠肉眼再发现一次。
    /// 存 `[封面w, 封面h, 视频w, 视频h]`。
    stage_rects: Rc<Cell<Option<[f32; 4]>>>,
}

impl HomeView {
    pub fn new(state: Arc<AppStateInner>, cx: &mut Context<Self>) -> Self {
        let mut feeds = HashMap::new();
        let mut indexes = HashMap::new();
        for s in Source::ALL {
            feeds.insert(s, SourceFeed::default());
            indexes.insert(s, 0usize);
        }
        let mut view = Self {
            state: Arc::clone(&state),
            active: Source::Feed,
            feeds,
            indexes,
            covers: HashMap::new(),
            covers_pending: HashSet::new(),
            epoch: 0,
            player: None,
            now: None,
            switching: false,
            frame_ready: false,
            frame: None,
            pending_title: None,
            wheel_at: None,
            last_save: Instant::now(),
            stream_prefetched: HashSet::new(),
            archives_prefetched: HashSet::new(),
            history: None,
            history_pending: false,
            seeds: Vec::new(),
            stage_rects: Rc::new(Cell::new(None)),
        };
        view.load_feed(Source::Feed, false, cx);
        view.load_history(cx);
        view
    }

    // —— 数据：推荐流 ——

    /// 拉一个 tab 的首批（`restart` = 换一批：重开会话、游标归零；在播剧
    /// 由渲染期的钉住逻辑保持在画面上）。
    fn load_feed(&mut self, source: Source, restart: bool, cx: &mut Context<Self>) {
        {
            let feed = self.feeds.get_mut(&source).expect("源已初始化");
            if feed.loading || feed.fetching_more {
                return;
            }
            if restart {
                feed.items.clear();
                feed.session.clear();
                feed.offset = 0;
                feed.has_more = false;
                feed.failed = None;
                *self.indexes.get_mut(&source).expect("游标已初始化") = 0;
            } else if !feed.items.is_empty() {
                return;
            }
            feed.loading = true;
        }
        cx.notify();

        let state = Arc::clone(&self.state);
        let session = self
            .feeds
            .get(&source)
            .map(|f| f.session.clone())
            .unwrap_or_default();
        let tab = source.tab_type().to_string();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_recommend_feed(&tab, &session, 0, &[], &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                let feed = view.feeds.get_mut(&source).expect("源已初始化");
                feed.loading = false;
                match result {
                    Ok(page) => {
                        feed.session = page.session_id;
                        feed.offset = page.next_offset;
                        feed.has_more = page.has_more;
                        feed.items = page.items.iter().map(StreamItem::from).collect();
                        feed.failed = None;
                    }
                    Err(e) => {
                        feed.failed = Some(format!("推荐流加载失败：{e}"));
                    }
                }
                view.after_feed_loaded(source, cx);
            });
        })
        .detach();
    }

    /// 同会话续拉下一页（游标临近尾部时调用）。
    fn load_more(&mut self, source: Source, cx: &mut Context<Self>) {
        let (session, offset) = {
            let feed = self.feeds.get(&source).expect("源已初始化");
            if feed.fetching_more || feed.loading || !feed.has_more {
                return;
            }
            (feed.session.clone(), feed.offset)
        };
        {
            let feed = self.feeds.get_mut(&source).expect("源已初始化");
            feed.fetching_more = true;
        }
        let state = Arc::clone(&self.state);
        let tab = source.tab_type().to_string();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_recommend_feed(&tab, &session, offset, &[], &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                let feed = view.feeds.get_mut(&source).expect("源已初始化");
                feed.fetching_more = false;
                if let Ok(page) = result {
                    feed.session = page.session_id;
                    feed.offset = page.next_offset;
                    feed.has_more = page.has_more;
                    feed.items.extend(page.items.iter().map(StreamItem::from));
                    view.after_feed_loaded(source, cx);
                }
            });
        })
        .detach();
    }

    /// 首批/续页就位后的钩子：冷启动（引擎未起）自动播当前源第一条——对齐
    /// Tauri「起点决策 effect」的自动起播；已在播则只做游标跟随（种子到位、
    /// 换批、翻页都会改列表，游标指到的剧变了就该切过去）。
    fn after_feed_loaded(&mut self, source: Source, cx: &mut Context<Self>) {
        if self.player.is_none() && !self.switching && source == self.active {
            if let Some(item) = self.display_items().first().cloned() {
                self.play_target(item, None, cx);
            }
        } else if source == self.active {
            self.follow_cursor(cx);
        }
        cx.notify();
    }

    /// 列表结构变了（种子到位/翻页/换批）后跟上游标：游标指的剧与在播的
    /// 不一致就切过去——Tauri 的起点决策 effect 同语义（currentSeries 变了
    /// 就 setTarget，无闩锁）；切换途中不追，起播落定后各有出口。
    fn follow_cursor(&mut self, cx: &mut Context<Self>) {
        if self.switching || self.now.is_none() {
            return;
        }
        let Some(item) = self.display_items().get(self.cursor()).cloned() else {
            return;
        };
        if self
            .now
            .as_ref()
            .is_some_and(|n| n.series_id == item.series_id)
        {
            return;
        }
        self.play_target(item, None, cx);
    }

    /// 云端历史（推荐 tab 种子 + 起点兜底）。登录才有数据，匿名回空表。
    fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history_pending {
            return;
        }
        self.history_pending = true;
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_watch_history(0, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            if let Ok(page) = result {
                let _ = this.update(cx, |view, cx| {
                    view.history = Some(page.items);
                    // 「你的内容」种子到位（书架随后续上）
                    view.merge_seeds(cx);
                    view.load_seeds_meta(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 「你的内容」排前：云端历史种子（书架随后由 [`Self::load_seeds_meta`]
    /// 补进同一段）。写进独立数组，不动流缓存（Tauri seedItems 同款）。
    fn merge_seeds(&mut self, cx: &mut Context<Self>) {
        let Some(history) = &self.history else {
            return;
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut seeds = Vec::new();
        for h in history {
            if h.series_id.is_empty() || !seen.insert(h.series_id.clone()) {
                continue;
            }
            seeds.push(StreamItem {
                series_id: h.series_id.clone(),
                title: h.title.clone(),
                cover: h.cover.clone(),
                horiz_cover: String::new(),
                ..Default::default()
            });
        }
        self.seeds = seeds;
        self.follow_cursor(cx);
    }

    /// 书架种子（历史之后）：标题/封面缺，起播后由档案解析自然补上
    /// （Tauri 同口径：书架条目只带 series_id）。种子段内部去重。
    fn load_seeds_meta(&mut self, cx: &mut Context<Self>) {
        let state = Arc::clone(&self.state);
        let account = state.settings().account.filter(|a| !a.user_id.is_empty());
        let Some(account) = account else {
            return;
        };
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_bookshelf(&account.user_id, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            if let Ok(entries) = result {
                let _ = this.update(cx, |view, cx| {
                    let mut seen: HashSet<String> =
                        view.seeds.iter().map(|i| i.series_id.clone()).collect();
                    for e in entries {
                        let series_id = e.series_id.clone();
                        if series_id.is_empty() || !seen.insert(series_id.clone()) {
                            continue;
                        }
                        view.seeds.push(StreamItem {
                            series_id,
                            ..Default::default()
                        });
                    }
                    view.follow_cursor(cx);
                });
            }
        })
        .detach();
    }

    // —— 展示列表：钉住在播 + 种子已并入 ——

    /// 渲染/交互共用的展示列表：种子（仅推荐 tab）+ 流条目，**按 series_id
    /// 去重**——会话轮换后跨页偶发同条目（Tauri 渲染期同款去重），不去重
    /// 就是滚轮里同一部剧连出好几遍。在播剧若已被换出列表（换一批后的新
    /// 批次通常没有它），钉在最前——「在播不动」。
    fn display_items(&self) -> Vec<StreamItem> {
        let feed = self.feeds.get(&self.active).expect("源已初始化");
        let mut seen: HashSet<String> = HashSet::new();
        let mut items: Vec<StreamItem> = Vec::with_capacity(self.seeds.len() + feed.items.len());
        if self.active == Source::Feed {
            for s in &self.seeds {
                if !s.series_id.is_empty() && seen.insert(s.series_id.clone()) {
                    items.push(s.clone());
                }
            }
        }
        for i in &feed.items {
            if !i.series_id.is_empty() && seen.insert(i.series_id.clone()) {
                items.push(i.clone());
            }
        }
        if let Some(now) = &self.now
            && !items.iter().any(|i| i.series_id == now.series_id)
        {
            items.insert(
                0,
                StreamItem {
                    series_id: now.series_id.clone(),
                    title: now.title.clone(),
                    ..Default::default()
                },
            );
        }
        items
    }

    /// 游标位置（展示列表下标，夹到已加载范围）。语义**对齐 Tauri**：游标是
    /// 位置不是身份，列表头部插了种子就落在种子上（冷启动正是这样切到
    /// 「你的内容」第一条，见 `player-page/start` 的起点决策 effect）。
    fn cursor(&self) -> usize {
        let max = self.display_items().len().saturating_sub(1);
        (*self.indexes.get(&self.active).expect("游标已初始化")).min(max)
    }

    /// 把游标移到展示列表的第 `idx` 项。
    fn set_cursor(&mut self, idx: usize) {
        *self.indexes.get_mut(&self.active).expect("游标已初始化") = idx;
    }

    /// 滚轮切剧（Tauri 同参：400ms 冷却 + 小位移忽略）。
    fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .wheel_at
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400))
        {
            return;
        }
        let len = self.display_items().len();
        if len == 0 {
            return;
        }
        let cur = self.cursor();
        let next = (cur as i64 + dir as i64).clamp(0, len as i64 - 1) as usize;
        if next == cur {
            return;
        }
        self.wheel_at = Some(now);
        self.set_cursor(next);
        // 快滑到尾部：预取下一页
        let feed = self.feeds.get(&self.active).expect("源已初始化");
        if feed.has_more && !feed.fetching_more && len - next <= 3 {
            self.load_more(self.active, cx);
        }
        if let Some(item) = self.display_items().get(next).cloned() {
            self.play_target(item, None, cx);
        }
        cx.notify();
    }

    // —— 播放编排 ——

    /// 切到目标剧。旧的引擎继续播到新引擎就位（对齐 Tauri「store 目标
    /// 变了但画面还在上一部」的切换语义），就位即整体换装。
    fn play_target(
        &mut self,
        item: StreamItem,
        forced_next_ep: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        if self
            .now
            .as_ref()
            .is_some_and(|n| n.series_id == item.series_id)
            && forced_next_ep.is_none()
        {
            return;
        }
        // 起播目标即身份：出问题（画面错位/内容重复）时这行是唯一能对齐
        // 「屏幕上那部剧」与「引擎在放哪部」的证据。
        log::info!(
            "[Home] 起播目标 {}（{}）epoch={} 标题={}",
            item.series_id,
            match forced_next_ep {
                Some(ep) => format!("连播第 {ep} 集"),
                None => "起点决策".to_string(),
            },
            self.epoch + 1,
            if item.title.is_empty() {
                "<无标题>"
            } else {
                item.title.as_str()
            }
        );
        self.switching = true;
        self.frame_ready = false;
        self.pending_title = Some(if item.title.is_empty() {
            "正在准备…".to_string()
        } else {
            item.title.clone()
        });
        self.epoch += 1;
        self.load_cover_of(&item, cx);
        cx.notify();

        let state = Arc::clone(&self.state);
        let history = self.history.clone().unwrap_or_default();
        let (events, rx) = mpsc::channel::<PlayerEvent>(64);
        let epoch = self.epoch;
        // 事件循环先于引擎起：Channel 关闭（引擎丢弃 → 发送端清零）即自然退出
        self.spawn_event_loop(epoch, rx, cx);
        let done = bridge::spawn(Self::play_task(
            state,
            item,
            forced_next_ep,
            history,
            events,
        ));
        cx.spawn(async move |this, cx| {
            let result = done.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                // 陈旧任务（期间用户又切了剧）：不接管，引擎随 result 丢弃自停
                if view.epoch != epoch {
                    return;
                }
                match result {
                    Ok((handle, now)) => {
                        // 新引擎就位：旧的连同它的帧一起退场
                        view.player = Some(handle);
                        view.frame = None;
                        view.now = Some(now);
                        view.switching = false;
                        view.frame_ready = false;
                    }
                    Err(e) => {
                        log::warn!("[Home] 起播失败: {e}");
                        view.switching = false;
                        view.pending_title = Some(format!("起播失败：{e}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 起播链（tokio 线程）：解析档案 → 起点决策 → prepare → 引擎 open。
    /// 与 Tauri 同一段服务面；起点 >3 秒续播，本地档案优先、云端历史兜底。
    async fn play_task(
        state: Arc<AppStateInner>,
        item: StreamItem,
        forced_next_ep: Option<u32>,
        history: Vec<WatchHistoryItem>,
        events: mpsc::Sender<PlayerEvent>,
    ) -> AppResult<(PlayerHandle, NowPlaying)> {
        let runtime = PlayRuntime::new(crate::tokio().handle().clone(), None);
        let settings = state.settings();
        let env = state.api_env();

        let series = match state.store.series_by_id(&item.series_id)? {
            Some(s) if !s.episodes.is_empty() => s,
            _ => {
                let s = hongguo_service::series_service::resolver::resolve_series(
                    &item.series_id,
                    &env,
                )
                .await?;
                state.store.upsert_series(&s)?;
                s
            }
        };
        let episode_count = series.episodes.len();
        let (vid_index, start_secs) = match forced_next_ep {
            Some(idx) => (idx, 0.0),
            None => {
                let local = state
                    .store
                    .series_last_position(&item.series_id)?
                    .filter(|(_, p)| p.current_time > 3.0);
                if let Some((idx, p)) = local {
                    (idx, p.current_time)
                } else {
                    match history
                        .iter()
                        .find(|h| h.series_id == item.series_id && h.position_ms > 3_000)
                    {
                        Some(h) => (h.vid_index.max(1) as u32, h.position_ms as f64 / 1000.0),
                        None => (1, 0.0),
                    }
                }
            }
        };
        let ep = series
            .episodes
            .iter()
            .find(|e| e.vid_index == vid_index)
            .or_else(|| series.episodes.iter().find(|e| e.vid_index == 1))
            .ok_or_else(|| hongguo_common::AppError::Media("分集表里没有可播的集".into()))?;
        if ep.vid.trim().is_empty() {
            return Err(hongguo_common::AppError::Media("该集没有 vid".into()));
        }
        let progress_key = format!("{}:{}", item.series_id, ep.vid_index);
        let prepared = prepare(&runtime, &ep.vid, &progress_key, None, settings, &env).await?;
        let handle = hongguo_playback::open(OpenOptions {
            vid: ep.vid.clone(),
            definition: prepared.definition,
            start_secs,
            events,
        })
        .await?;
        Ok((
            handle,
            NowPlaying {
                series_id: item.series_id,
                title: if item.title.is_empty() {
                    series.title.clone()
                } else {
                    item.title.clone()
                },
                vid_index: ep.vid_index,
                episode_count,
                heat_text: item.heat_text,
                season_tag: item.season_tag,
                badge: item.badge,
            },
        ))
    }

    /// 引擎事件循环（每个引擎一个，epoch 封口：换剧后陈旧事件自然断流）。
    fn spawn_event_loop(
        &mut self,
        epoch: u64,
        mut events: mpsc::Receiver<PlayerEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                let ok = this
                    .update(cx, |view, cx| {
                        if view.epoch != epoch {
                            return;
                        }
                        view.on_player_event(event, cx);
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_player_event(&mut self, event: PlayerEvent, cx: &mut Context<Self>) {
        match event {
            PlayerEvent::FirstFrame { .. } => {
                self.frame_ready = true;
                self.frame = self.player.as_ref().and_then(|h| h.latest_frame());
                self.prefetch_next_stream(cx);
            }
            PlayerEvent::Frame => {
                self.frame = self.player.as_ref().and_then(|h| h.latest_frame());
                self.save_progress_maybe();
                // 相邻剧分集档案预取（换剧秒回，Tauri 同节奏）
                self.prefetch_neighbor_archives();
            }
            PlayerEvent::Ended => self.advance_after_end(cx),
            PlayerEvent::Failed(msg) => {
                self.pending_title = Some(format!("播放失败：{msg}"));
            }
        }
        cx.notify();
    }

    /// 5 秒节流落盘（播放档案是真值：续播、详情页「继续看」都吃它）。
    fn save_progress_maybe(&mut self) {
        if self.last_save.elapsed() < Duration::from_secs(5) {
            return;
        }
        self.last_save = Instant::now();
        let Some((handle, now)) = self.player.as_ref().zip(self.now.clone()) else {
            return;
        };
        let (pos, dur) = (handle.position_secs(), handle.duration_secs());
        let store = self.state.store.clone();
        bridge::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                position::save_store(&store, &now.series_id, now.vid_index, pos, dur)
            })
            .await;
        });
    }

    /// 连播：下一集；末集接流内下一部（剧终接力的跨季/换批语义后续对齐）。
    fn advance_after_end(&mut self, cx: &mut Context<Self>) {
        self.frame = None;
        self.frame_ready = false;
        if let Some(now) = self.now.clone()
            && (now.vid_index as usize) < now.episode_count
        {
            let next_idx = now.vid_index + 1;
            if let Some(item) = self
                .display_items()
                .iter()
                .find(|i| i.series_id == now.series_id)
                .cloned()
            {
                self.play_target(item, Some(next_idx), cx);
                return;
            }
        }
        self.step(1, cx);
    }

    /// 首帧出画：预取下一部剧续播集的流（每部一次；Tauri 首帧发令同款）。
    fn prefetch_next_stream(&mut self, cx: &mut Context<Self>) {
        let items = self.display_items();
        let cursor = self.cursor();
        let Some(next) = items.get(cursor + 1).cloned() else {
            return;
        };
        if !self.stream_prefetched.insert(next.series_id.clone()) {
            return;
        }
        let state = Arc::clone(&self.state);
        let history = self.history.clone().unwrap_or_default();
        // 整条取流链跑在 tokio 桥里（reqwest/定时器要 reactor）；GPUI 侧只接结果
        let done = bridge::spawn(Self::prefetch_stream_task(
            state,
            next.series_id.clone(),
            history,
        ));
        cx.spawn(async move |this, cx| {
            if let Err(e) = done.await.expect("tokio 桥不会丢弃发送端") {
                // 失败不算数：下次再成为「下一部」时允许重试
                let _ = this.update(cx, |view, _| {
                    view.stream_prefetched.remove(&next.series_id);
                });
                log::info!("[Home] 预取下一部流失败（忽略）: {e}");
            }
        })
        .detach();
    }

    async fn prefetch_stream_task(
        state: Arc<AppStateInner>,
        series_id: String,
        history: Vec<WatchHistoryItem>,
    ) -> AppResult<()> {
        let runtime = PlayRuntime::new(crate::tokio().handle().clone(), None);
        let settings = state.settings();
        let env = state.api_env();
        let series = match state.store.series_by_id(&series_id)? {
            Some(s) if !s.episodes.is_empty() => s,
            _ => {
                let s = hongguo_service::series_service::resolver::resolve_series(&series_id, &env)
                    .await?;
                state.store.upsert_series(&s)?;
                s
            }
        };
        let vid_index = match state.store.series_last_position(&series_id)? {
            Some((idx, p)) if p.current_time > 3.0 => idx,
            _ => match history
                .iter()
                .find(|h| h.series_id == series_id && h.position_ms > 3_000)
            {
                Some(h) => h.vid_index.max(1) as u32,
                None => 1,
            },
        };
        let ep = series
            .episodes
            .iter()
            .find(|e| e.vid_index == vid_index)
            .or_else(|| series.episodes.iter().find(|e| e.vid_index == 1));
        let Some(ep) = ep else {
            return Ok(());
        };
        if ep.vid.trim().is_empty() {
            return Ok(());
        }
        let progress_key = format!("{series_id}:{}", ep.vid_index);
        prefetch_stream(&runtime, &ep.vid, &progress_key, &settings, &env).await
    }

    /// 相邻剧分集档案预取（换剧时 resolve 秒回）。
    fn prefetch_neighbor_archives(&mut self) {
        let items = self.display_items();
        let cursor = self.cursor();
        for idx in [cursor + 1, cursor.saturating_sub(1)] {
            let Some(item) = items.get(idx) else {
                continue;
            };
            let sid = item.series_id.clone();
            if sid.is_empty() || !self.archives_prefetched.insert(sid.clone()) {
                continue;
            }
            let state = Arc::clone(&self.state);
            bridge::spawn(async move {
                if state.store.series_by_id(&sid).ok().flatten().is_none() {
                    let env = state.api_env();
                    if let Ok(s) =
                        hongguo_service::series_service::resolver::resolve_series(&sid, &env).await
                    {
                        let _ = state.store.upsert_series(&s);
                    }
                }
            });
        }
    }

    // —— 封面 ——

    /// 懒加载当前/目标剧的封面（全屏占位用）。
    fn load_cover_of(&mut self, item: &StreamItem, cx: &mut Context<Self>) {
        let remote = if item.horiz_cover.is_empty() {
            item.cover.clone()
        } else {
            item.horiz_cover.clone()
        };
        let sid = item.series_id.clone();
        if remote.is_empty()
            || self.covers.contains_key(&sid)
            || !self.covers_pending.insert(sid.clone())
        {
            return;
        }
        // acquire_renderable 是同步函数（内部自建 runtime），必须走阻塞池
        let rx = bridge::spawn(async move {
            tokio::task::spawn_blocking(move || {
                hongguo_protocol::cover::acquire_renderable(&remote).map(|(bytes, _)| bytes)
            })
            .await
            .expect("阻塞池 join")
        });
        cx.spawn(async move |this, cx| {
            let bytes: Vec<u8> = match rx.await.expect("tokio 桥不会丢弃发送端") {
                Ok(b) => b,
                Err(e) => {
                    let _ = this.update(cx, |view, _| {
                        log::warn!("[Home] 封面加载失败: {e}");
                        view.covers_pending.remove(&sid);
                    });
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
                    log::warn!("[Home] 封面解码失败: {e}");
                    view.covers_pending.remove(&sid);
                }
            });
        })
        .detach();
    }

    // —— 渲染 ——

    fn render_tabs(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        h_flex()
            .justify_center()
            .gap_2()
            .pt_2()
            .children(Source::ALL.map(|s| {
                let active = s == self.active;
                let button = Button::new(ElementId::from(format!("tab-{:?}", s)))
                    .label(s.label())
                    .small();
                let button = if active {
                    button.secondary()
                } else {
                    button.ghost()
                };
                button.on_click(cx.listener(move |this, _, _, cx| {
                    if this.active == s {
                        // 重复点当前 tab = 换一批（重开会话、游标归零）
                        this.load_feed(s, true, cx);
                    } else {
                        this.active = s;
                        let empty = this.feeds.get(&s).is_none_or(|f| f.items.is_empty());
                        if empty {
                            this.load_feed(s, false, cx);
                        } else {
                            // 切 tab 后当前游标的剧不在播：切过去（Tauri 的
                            // 起点决策 effect 同语义：current 变了就换目标）
                            let cursor_item = this
                                .display_items()
                                .get(this.cursor())
                                .map(|i| i.series_id.clone());
                            let target_same = this
                                .now
                                .as_ref()
                                .is_some_and(|n| cursor_item.as_deref() == Some(&n.series_id));
                            if !target_same
                                && let Some(item) = this.display_items().get(this.cursor()).cloned()
                            {
                                this.play_target(item, None, cx);
                            }
                        }
                    }
                    cx.notify();
                }))
            }))
            .into_any_element()
    }

    /// 沉浸舞台：封面占位层 + 视频层 + 信息层三层**全部绝对定位**叠在同一个
    /// 盒子里，铺满舞台。
    ///
    /// 三层都必须 absolute：舞台是 v_flex（块级流），封面层一旦是普通流内
    /// 子元素就会按图片固有高度占位（1080 宽 → 1544 高），把后面的视频层
    /// 整个顶到舞台外面压出去——看到的就是「封面在上一半、视频在下半截」。
    /// Tauri 侧同款写法：`absolute inset-0`（那里不存在流式占位这一说）。
    ///
    /// 层序对齐 Tauri `player-page.tsx`：封面在视频**之上**（z-5），出画前与
    /// 切剧期间盖着（取流/解码间隙看到的是这部剧的封面而不是黑屏），首帧
    /// 出画且不在切换中就整层收掉。
    ///
    /// 返回 `Div`（不是 AnyElement）：调用方要挂滚轮监听与 `.id`。
    fn render_stage(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme();
        // 占位封面取**目标剧**（游标位）：切剧瞬间游标已指向下一部，先出
        // 它的封面；无封面条目（书架种子）退回首帧方向已在播的那部。
        let cover_sid = self
            .display_items()
            .get(self.cursor())
            .map(|i| i.series_id.clone())
            .or_else(|| self.now.as_ref().map(|n| n.series_id.clone()));
        let cover_shown = !self.frame_ready || self.switching;
        // 封面层：图 + 压暗（Tauri 的 `brightness-[0.55]`；GPUI 没有亮度滤镜，
        // 用等价的 45% 黑罩）。
        let cover: gpui_kit::AnyElement = match cover_sid
            .as_ref()
            .filter(|_| cover_shown)
            .and_then(|s| self.covers.get(s))
        {
            Some(bitmap) => div()
                .absolute()
                .size_full()
                .child(
                    img(ImageSource::Render(Arc::clone(bitmap)))
                        .absolute()
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .child(div().absolute().size_full().bg(gpui_kit::rgba(0x00000073)))
                .into_any_element(),
            None => div().absolute().size_full().into_any_element(),
        };
        // 层序对齐 Tauri `player-page.tsx`：绘画序 = 树序，视频在下、封面在
        // **上**（z-5）——取流/解码的空档与切剧期间盖住的是新一部剧的封面，
        // 而不是上一部剧的残帧或黑屏；首帧出画且不在切换中整层让位。
        let video: gpui_kit::AnyElement = match &self.frame {
            Some(frame) => surface(frame.buffer())
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => h_flex()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
        };
        // 信息叠加层（左下）：剧名 + 集数 + 热度/角标
        let overlay = self.render_overlay(cx);
        let rects = Rc::clone(&self.stage_rects);
        v_flex()
            .flex_1()
            .min_h_0()
            .bg(gpui_kit::rgb(0x000000))
            .relative()
            .overflow_hidden()
            .child(video)
            .child(cover)
            .child(overlay)
            // 布局哨兵：子层矩形只在这里可见，两次准备绘制之间比对（见字段注释）。
            // 绘画序是「视频、封面、叠加层」，前两个就是这两层。
            .on_children_prepainted(move |bounds, _window, _cx| {
                let Some(video) = bounds.first() else { return };
                let size = |b: &gpui_kit::Bounds<gpui_kit::Pixels>| {
                    [b.size.width.to_f64() as f32, b.size.height.to_f64() as f32]
                };
                let (v, c) = (size(video), bounds.get(1).map(size).unwrap_or([0.0; 2]));
                let sample = [c[0], c[1], v[0], v[1]];
                if rects.get() != Some(sample) {
                    rects.set(Some(sample));
                    log::info!(
                        "[Home][stage] 封面层 {:.0}×{:.0}，视频层 {:.0}×{:.0}（应同为舞台整块）",
                        c[0],
                        c[1],
                        v[0],
                        v[1]
                    );
                }
            })
            .when(self.switching, |el| {
                el.child(
                    div()
                        .absolute()
                        .top_2()
                        .left_2()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .bg(gpui_kit::rgba(0x000000aa))
                        .child(
                            div().text_xs().text_color(theme.muted_foreground).child(
                                self.pending_title
                                    .clone()
                                    .unwrap_or_else(|| "正在切换…".into()),
                            ),
                        ),
                )
            })
    }

    fn render_overlay(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme();
        let Some(now) = &self.now else {
            return div().into_any_element();
        };
        // 展示标记跟着**在播的剧**（起播时抄存），不跟游标：换一批/钉住条目
        // 位移时游标可能落在别的条目上，跟游标会把隔壁剧的热度/角标贴错。
        let badge = (!now.badge.is_empty()).then(|| now.badge.clone());
        let season = (!now.season_tag.is_empty()).then(|| now.season_tag.clone());
        let heat = (!now.heat_text.is_empty()).then(|| now.heat_text.clone());
        v_flex()
            .absolute()
            .bottom_4()
            .left_4()
            .max_w_96()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .children(season.map(|s| {
                        div()
                            .px_1()
                            .rounded(theme.radius)
                            .bg(gpui_kit::rgba(0xffffff26))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgb(0xffffff))
                                    .child(SharedString::from(s)),
                            )
                    }))
                    .children((!now.title.is_empty()).then(|| {
                        div()
                            .text_lg()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(gpui_kit::rgb(0xffffff))
                            .truncate()
                            .child(SharedString::from(now.title.clone()))
                    }))
                    .children(badge.map(|b| {
                        div()
                            .px_1()
                            .rounded(theme.radius)
                            .bg(gpui_kit::rgba(0xef4444cc))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui_kit::rgb(0xffffff))
                                    .child(SharedString::from(b)),
                            )
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(gpui_kit::rgba(0xffffffcc))
                            .child(format!(
                                "第 {} 集 / 共 {} 集",
                                now.vid_index, now.episode_count
                            )),
                    )
                    .children(heat.map(|h| {
                        div()
                            .text_sm()
                            .text_color(gpui_kit::rgba(0xffffff99))
                            .child(SharedString::from(format!("🔥 {h}")))
                    })),
            )
            .into_any_element()
    }
}

impl gpui_kit::Render for HomeView {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        // render_tabs 要 &mut cx，先取（theme 借的是 cx 的不可变引用）
        let tabs = self.render_tabs(cx);
        let theme = cx.theme();
        let feed = self.feeds.get(&self.active).expect("源已初始化");
        let notice: Option<String> = if let Some(err) = &feed.failed {
            Some(err.clone())
        } else if feed.loading && self.now.is_none() {
            Some("加载中…".into())
        } else if !feed.loading && feed.items.is_empty() && self.now.is_none() {
            Some("暂无内容".into())
        } else {
            None
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            // 沉浸流锁死在视口内：任何 1px 超高都会冒出页面滚动条并吃掉滚轮
            .overflow_hidden()
            .child(tabs)
            .child(match notice {
                Some(msg) => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(msg),
                    )
                    .child(
                        Button::new("home-retry")
                            .label("重试")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_feed(this.active, false, cx);
                            })),
                    )
                    .into_any_element(),
                None => self
                    .render_stage(cx)
                    .id("home-stage")
                    .on_scroll_wheel(cx.listener(
                        |this, ev: &gpui_kit::ScrollWheelEvent, window, cx| {
                            // Tauri 同参：|Δy|<15 忽略，400ms 冷却在 step 里
                            let y = ev.delta.pixel_delta(window.line_height()).y.to_f64();
                            if y.abs() < 15.0 {
                                return;
                            }
                            this.step(if y > 0.0 { 1 } else { -1 }, cx);
                        },
                    ))
                    .into_any_element(),
            })
    }
}

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
//! 控制区（对齐 Tauri 版控制栏的核心件）：暂停/继续（按钮 + 点击画面）、
//! 进度条拖动 seek、倍速菜单（0.75–3x，音频变速不变调）、音量/静音；偏好
//! 经 [`crate::playback_prefs`] 跨集跨启动保留。键盘快捷键（空格/方向键）
//! 与 ↑↓ 切剧随控制层下一批目标落地；「回首页 5 分钟换一批」的过期重开
//! 未做（换一批入口本身已对齐）。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
#[cfg(target_os = "macos")]
use gpui_kit::surface;
use gpui_kit::{
    ElementId, Entity, ImageSource, ObjectFit, RenderImage, SharedString, Window, div, img,
};
use hongguo_api::danmaku::{
    CommentItem, Danmaku, ReplyItem, fetch_comment_replies, fetch_comments_page, fetch_danmaku_all,
};
use hongguo_api::discover::FeedItem;
use hongguo_api::history::{WatchHistoryItem, fetch_watch_history};
use hongguo_api::interact::{
    InteractionItem, collect_series, digg_comment, digg_video, fetch_bookshelf,
    fetch_interaction_state, send_comment, send_danmaku, send_reply,
};
use hongguo_api::recommend::fetch_recommend_feed;
use hongguo_common::AppResult;
use hongguo_common::model::{DownloadTask, VideoDefinition};
use hongguo_media_platform::platform::video::{FrameBuffer, VideoFrame};
use hongguo_playback::{OpenOptions, PlayerEvent, PlayerHandle};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::play_service::online::{PlayRuntime, prefetch_stream, prepare};
use hongguo_service::play_service::position;
use tokio::sync::mpsc;

use crate::bridge;

// ---------------------------------------------------------------- 数据面

/// 进度拖动的拖拽载荷（`on_drag`/`on_drag_move` 按类型配对）。
struct ScrubDrag;

/// 拖动期间跟指针的幽灵视图：空 div（拖动不需要可视残影，预览走进度条本身）。
struct ScrubGhost;

impl gpui_kit::Render for ScrubGhost {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        div()
    }
}

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
    /// 本集提供的全部档位（清晰度菜单数据源；本地集为空 = 菜单置灰）
    definitions: Vec<VideoDefinition>,
    /// 实际在播的档位（0 = 尚未取到）
    definition: u32,
    /// 本集的 vid（弹幕 group_id）
    vid: String,
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
    /// 当前可上屏的帧；`None` = 还没出画（画封面占位）。
    ///
    /// 出画与否**只认这一处**：曾经另立过 `frame_ready` 标志，和「引擎就位」
    /// 更新互相抢时序——首帧先到置 true、就位后到又清 false，封面从此永远
    /// 收不掉（画面上封面与视频叠着）。帧直接来自事件，没有第二处状态。
    frame: Option<VideoFrame>,
    /// Windows 的渲染产物：NV12 → RGBA（BT.709 limited）在事件任务里转换
    /// （不占 UI 线程），渲染按普通图像上传——gpui 的视频 surface 只有
    /// Apple 目标（CVPixelBuffer），Windows 走这条等价路径。
    #[cfg(target_os = "windows")]
    frame_image: Option<Arc<RenderImage>>,
    /// 已下台、等 render 里 `window.drop_image` 回收的图像。gpui 的纹理按
    /// `RenderImage::id` 常驻 sprite atlas（`paint_image` 只增不删），视频
    /// 路径每帧一个新 id，不主动回收就是无上限的显存/内存泄漏。
    #[cfg(target_os = "windows")]
    retired_images: Vec<Arc<RenderImage>>,
    /// 视频转换目标尺寸（宽,高；CSS px）。render_stage 按舞台显示区刷新，
    /// 事件循环的 NV12→RGBA 按它缩放输出——769px 的视口出 1080p 纹理纯
    /// 浪费：转换 CPU、图集纹理、回收压力都按面积走（8.3MB → ~1.3MB）。
    #[cfg(target_os = "windows")]
    video_target: Rc<Cell<(usize, usize)>>,
    /// 舞台周期状态日志的上一跳（排障取证：frame/switching/image 状态）。
    stage_diag_at: Cell<Instant>,
    /// 切换目标（switching 时 overlay 显示它的封面与标题）。
    pending_title: Option<String>,

    // —— 控制区（对齐 Tauri 版控制栏核心件） ——
    /// 倍速/音量/静音的现值（引擎侧同值；换集后重放给新引擎，跨启动走
    /// [`crate::playback_prefs`]）。
    rate: f64,
    volume: f32,
    muted: bool,
    /// 音量滑杆（0–1；纯用户驱动，无需程序化回写）。
    volume_slider: Entity<SliderState>,
    /// 进度拖动中的目标秒（None = 跟随播放位置显示）。
    scrubbing: Option<f64>,
    /// 上一次真正下发给引擎的 scrub seek 时刻：拖动中的连续 seek 按它节流
    ///（Tauri 的 `<video>` 每次赋值 currentTime 浏览器会自行合并，引擎这侧
    /// 每发一次就是冲流 + 解码器重建，必须限频；松手的那次不受限）。
    last_scrub_seek: Cell<Instant>,
    /// 连播锁定（Tauri bingeSeriesId 同款）：信息流里主动选了某集/点了画面
    /// = 要追这部，锁定后滚轮/↑↓ 切集而不是跟随换剧；Esc/点返回解除。
    binge: Option<String>,
    /// 选集浮层开着（贴控制栏向上弹，见 [`Self::render_picker`]）。
    picker_open: bool,
    /// 手动翻到的范围段（每段 30 集）：记设置时的当前集——当前集变了
    /// （连播/点选）覆盖作废，自动跟回当前集所在段（Tauri groupOverride）。
    picker_group_override: Option<(usize, u32)>,
    /// 用户显式选择的清晰度（None = 自动/沿用）：跨集保留（Tauri 的
    /// definition 会话态同语义），传给后续每次起播的 prepare。
    preferred_definition: Option<u32>,
    /// 剧集简介（video_detail 的 series_intro，Tauri useSeriesDetailMeta
    /// 同源同语义）：按剧缓存（装饰性信息，失败静默降级）。
    intros: HashMap<String, String>,
    /// 简介拉取中的剧（去重）。
    intros_pending: HashSet<String>,
    /// 简介展开态（Tauri introExpanded：超过 40 字可展开/收起）。
    intro_expanded: bool,
    /// 弹幕总开关（Tauri danmakuOn，随偏好落盘）。
    danmaku_on: bool,
    /// 弹幕文字不透明度 0.1–1。
    dm_opacity: f32,
    /// 弹幕字号缩放 0.5–2。
    dm_font_scale: f32,
    /// 弹幕密度 0–1（入场按比例随机丢弃）。
    dm_density: f32,
    /// 弹幕显示区域 0.25–1（占舞台顶部高度比例）。
    dm_area: f32,
    /// 弹幕设置面板开着。
    dm_panel_open: bool,
    /// 当前集的弹幕（offset_ms 升序；每次进集重拉，Tauri useDanmaku 同款）。
    danmaku_items: Arc<Vec<Danmaku>>,
    /// 已拉取弹幕的集（vid）——同集不重拉，换集必重拉。
    danmaku_vid: String,
    danmaku_fetching: bool,
    /// 已入场游标（items 下标；seek 时二分重置）。
    danmaku_cursor: usize,
    /// 在飞的弹幕（泳道 + 入场墙钟）。
    danmaku_flights: Vec<DmFlight>,
    /// 每条泳道「最早可再入场」的媒体秒（hgplayer laneFreeAt 同款）。
    danmaku_lane_free: Vec<f64>,
    /// 上一帧的播放位置（seek 跳变检测用）。
    danmaku_last_pos: f64,
    /// tick 的上次时刻（推进在飞弹幕的 dt 源）。
    danmaku_last_tick: Option<std::time::Instant>,
    /// 密度抽样的 LCG 状态。
    dm_rng: u64,
    /// 弹幕发送框输入（需要 window 创建，见 HomeView::new）。
    danmaku_input: Option<Entity<InputState>>,
    danmaku_sending: bool,
    /// 发送成功后清空输入框（set_value 需要 window，渲染期执行）。
    danmaku_clear_input: bool,
    /// 弹幕设置滑杆（透明度/字号/密度/面积）。
    dm_opacity_slider: Entity<SliderState>,
    dm_font_slider: Entity<SliderState>,
    dm_density_slider: Entity<SliderState>,
    dm_area_slider: Entity<SliderState>,
    // —— 互动（点赞/收藏/评论，Tauri interaction-rail + comment-panel） ——
    /// mget 最近互动列表（当前集/剧命中才有「已赞/已追」红标）。
    interact_items: Arc<Vec<InteractionItem>>,
    interact_loaded: bool,
    /// 集级公开计数（剧集档案 detail 下发，匿名可见；起播后从档案读）。
    ep_comment_count: i64,
    ep_digg_count: i64,
    /// 全剧收藏数（video_detail followed_cnt，load_series_meta 顺带）。
    follow_count: i64,
    digg_pending: bool,
    collect_pending: bool,
    /// 评论面板开着。
    comments_panel_open: bool,
    /// 当前集评论（已拍平去重；每次进集重拉——社交态不缓存复用）。
    comments: Arc<Vec<CommentItem>>,
    comments_total: i64,
    comments_cursor: String,
    comments_has_more: bool,
    comments_loading: bool,
    /// 已加载评论的集（vid）。
    comments_vid: String,
    /// 评论点赞的本地覆盖（乐观 ±1 与红心；key=comment_id）。
    comment_digg_override: HashMap<String, bool>,
    /// 展开回复的评论 id → 已拉回复（首页）与翻页游标。
    replies_open: HashSet<String>,
    replies: HashMap<String, Arc<Vec<ReplyItem>>>,
    replies_cursor: HashMap<String, String>,
    replies_loading: HashSet<String>,
    /// 回复目标：Some((comment_id, reply_to_reply_id, 被回复人昵称))。
    reply_to: Option<(String, Option<String>, String)>,
    /// 评论输入框（与弹幕发送框同款 InputState）。
    comment_input: Option<Entity<InputState>>,
    comment_sending: bool,
    comment_clear_input: bool,
    // —— 下载 ——
    /// 下载面板开着。
    downloads_open: bool,
    /// 隐身模式（Tauri incognito 简化对齐）：一键暂停 + 整窗近透明遮蔽，
    /// 再点恢复显示（播放由用户自行续）。
    incognito: bool,
    /// 全屏中（Render 时从 window.is_fullscreen 读，切图标用）。
    fullscreen: bool,
    /// 下载任务快照（每次渲染从队列读，进度靠 Frame 事件驱动的重渲染刷新）。
    downloads: Arc<Vec<DownloadTask>>,
    /// 选集浮层的下载完成集号（渲染时刷新）。
    done_vids: Arc<Vec<u32>>,
    /// 音量悬停弹层开着（Tauri VolumePopup 同款，随 wrapper hover 开合）。
    volume_popup_open: bool,
    /// 引擎 rebuffer 状态（Buffering/Ready 事件驱动，中间出 loading）。
    buffering: bool,
    /// 控制条可见截止时刻；拖动中/暂停时常显。
    controls_until: Option<Instant>,
    /// 进度轨道上帧矩形（拖动/点击的指针位置 → 比例换算用；同
    /// `stage_rects` 的哨兵存取模式）。
    progress_bounds: Rc<Cell<Option<gpui_kit::Bounds<gpui_kit::Pixels>>>>,

    wheel_at: Option<Instant>,
    last_save: Instant,
    /// 播放快捷键的焦点锚：键盘事件只派发给焦点子树，舞台持有一个
    /// FocusHandle（见 render_stage），首次渲染播种、点舞台带回。
    focus: gpui_kit::FocusHandle,
    focus_seeded: bool,
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

/// 在飞的弹幕：泳道 + 入场墙钟 + 估宽。位置每渲染帧按「播放中才走的
/// 墙钟」推进——暂停即冻结（无 Frame 事件），倍速不改变滚动速度
///（hgplayer 同款：它家倍速下弹幕也不变速）。progress 以「屏宽」为单位的
/// 滚动进度：x = stage_w − progress × (stage_w + 估宽)，progress ≥ 1 出屏。
struct DmFlight {
    text: String,
    lane: usize,
    width: f32,
    progress: f64,
}

impl DmFlight {
    /// 相对舞台左缘的横向偏移（像素，可为负 = 已滚出左沿）。
    fn x_offset(&self, stage_w: f64) -> f64 {
        stage_w - self.progress * (stage_w + self.width as f64)
    }
}

/// 弹幕估宽：CJK 记一字宽，其余记 0.55 字宽（无预渲染测量，误差对
/// 出屏回收无感）。
fn dm_text_width(text: &str, font_px: f32) -> f32 {
    let units: f32 = text
        .chars()
        .map(|c| if is_wide_char(c) { 1.0 } else { 0.55 })
        .sum();
    units * font_px
}

/// 宽字符判定（CJK/谚文/全形；估宽够用，不引 unicode-width 依赖——
/// gpui 依赖树里它与 unicode-properties 的版本钉子有过冲突史）。
fn is_wide_char(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x2FFFD
    )
}

/// 滚过一屏的时长（hgplayer durationMs 默认 9000）。
const DM_CROSS_MS: f64 = 9000.0;
/// 同一泳道相邻两条的入场间隔（视频秒）。
const DM_LANE_BUSY_S: f64 = 1.4;
/// 泳道高 = 字号 + 10（hgplayer top 计算同款）。
const DM_LANE_GAP: f32 = 10.0;
/// 泳道数下限。
const DM_MIN_LANES: usize = 4;
/// 基准字号范围（hgplayer 默认上限 22px）。
const DM_FONT_MIN: f32 = 15.0;
const DM_FONT_MAX: f32 = 22.0;

impl HomeView {
    pub fn new(state: Arc<AppStateInner>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut feeds = HashMap::new();
        let mut indexes = HashMap::new();
        for s in Source::ALL {
            feeds.insert(s, SourceFeed::default());
            indexes.insert(s, 0usize);
        }
        // 偏好先于引擎与滑杆：滑杆初值、起播后重放都吃它
        let prefs = crate::playback_prefs::load_from(
            &hongguo_store::paths::data_dir().join(crate::playback_prefs::FILE_NAME),
        );
        let volume_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .default_value(prefs.volume)
        });
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
            frame: None,
            #[cfg(target_os = "windows")]
            frame_image: None,
            #[cfg(target_os = "windows")]
            retired_images: Vec::new(),
            #[cfg(target_os = "windows")]
            video_target: Rc::new(Cell::new((1280, 720))),
            stage_diag_at: Cell::new(Instant::now()),
            pending_title: None,
            rate: prefs.rate,
            volume: prefs.volume,
            muted: prefs.muted,
            volume_slider,
            scrubbing: None,
            last_scrub_seek: Cell::new(Instant::now() - Duration::from_secs(10)),
            binge: None,
            picker_open: false,
            picker_group_override: None,
            preferred_definition: None,
            intros: HashMap::new(),
            intros_pending: HashSet::new(),
            intro_expanded: false,
            danmaku_on: prefs.danmaku_on,
            dm_opacity: prefs.dm_opacity,
            dm_font_scale: prefs.dm_font_scale,
            dm_density: prefs.dm_density,
            dm_area: prefs.dm_area,
            dm_panel_open: false,
            danmaku_items: Arc::new(Vec::new()),
            danmaku_vid: String::new(),
            danmaku_fetching: false,
            danmaku_cursor: 0,
            danmaku_flights: Vec::new(),
            danmaku_lane_free: Vec::new(),
            danmaku_last_pos: 0.0,
            danmaku_last_tick: None,
            dm_rng: 0x853c49e6748fea9b,
            danmaku_input: Some(cx.new(|cx| InputState::new(window, cx).placeholder("弹幕内容"))),
            danmaku_sending: false,
            danmaku_clear_input: false,
            dm_opacity_slider: cx.new(|_| {
                SliderState::new()
                    .min(0.1)
                    .max(1.0)
                    .default_value(prefs.dm_opacity)
            }),
            dm_font_slider: cx.new(|_| {
                SliderState::new()
                    .min(0.5)
                    .max(2.0)
                    .default_value(prefs.dm_font_scale)
            }),
            dm_density_slider: cx.new(|_| {
                SliderState::new()
                    .min(0.0)
                    .max(1.0)
                    .default_value(prefs.dm_density)
            }),
            dm_area_slider: cx.new(|_| {
                SliderState::new()
                    .min(0.25)
                    .max(1.0)
                    .default_value(prefs.dm_area)
            }),
            interact_items: Arc::new(Vec::new()),
            interact_loaded: false,
            ep_comment_count: 0,
            ep_digg_count: 0,
            follow_count: 0,
            digg_pending: false,
            collect_pending: false,
            comments_panel_open: false,
            comments: Arc::new(Vec::new()),
            comments_total: 0,
            comments_cursor: String::new(),
            comments_has_more: false,
            comments_loading: false,
            comments_vid: String::new(),
            comment_digg_override: HashMap::new(),
            replies_open: HashSet::new(),
            replies: HashMap::new(),
            replies_cursor: HashMap::new(),
            replies_loading: HashSet::new(),
            reply_to: None,
            comment_input: Some(cx.new(|cx| InputState::new(window, cx).placeholder("说点什么…"))),
            comment_sending: false,
            comment_clear_input: false,
            downloads_open: false,
            incognito: false,
            fullscreen: false,
            downloads: Arc::new(Vec::new()),
            done_vids: Arc::new(Vec::new()),
            volume_popup_open: false,
            buffering: false,
            controls_until: None,
            progress_bounds: Rc::new(Cell::new(None)),
            wheel_at: None,
            last_save: Instant::now(),
            focus: cx.focus_handle(),
            focus_seeded: false,
            stream_prefetched: HashSet::new(),
            archives_prefetched: HashSet::new(),
            history: None,
            history_pending: false,
            seeds: Vec::new(),
            stage_rects: Rc::new(Cell::new(None)),
        };
        view.subscribe_controls(cx);
        // 下载队列恢复：running 视为中断改回 pending（Tauri bootstrap 同款），
        // 然后触发一轮调度把中断的任务续跑起来
        {
            let tasks = view.state.store.tasks().unwrap_or_default();
            let limit = view.state.settings().max_concurrency as usize;
            view.state.replace_queue(
                hongguo_service::download_service::queue::DownloadQueue::restore(tasks, limit),
            );
            crate::downloads::kick(Arc::clone(&view.state));
        }
        // 取证心跳（HONGGUO_DIAG=1 时启用）：2s 一跳。这行停了 = 主线程/
        // 前台执行器堵死（update 派发不到主线程），render 停跳的根因直接
        // 归位。平时不开，不污染日志。
        if std::env::var("HONGGUO_DIAG").is_ok() {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    let ok = this
                        .update(cx, |_, cx| {
                            log::info!("[Home] 主线程心跳");
                            cx.notify();
                        })
                        .is_ok();
                    if !ok {
                        return;
                    }
                }
            })
            .detach();
        }
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
                        // 取证：dump 开关时把整页封面都预载落盘（离线对拍用）
                        if std::env::var("HONGGUO_DUMP_COVER").is_ok() {
                            let items: Vec<StreamItem> = feed.items.clone();
                            for item in items.iter().take(12) {
                                view.load_cover_of(item, cx);
                            }
                        }
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
                self.play_target(item, None, None, cx);
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
        self.play_target(item, None, None, cx);
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

    /// 连播锁定中（binge 命中当前剧）。
    fn in_binge(&self) -> bool {
        let Some(sid) = &self.binge else {
            return false;
        };
        self.now.as_ref().is_some_and(|n| &n.series_id == sid)
    }

    /// 连播锁定内的切集（滚轮/↑↓，Tauri useBingeRelay 的 stepEpisode 同款）：
    /// 越过第 1 集原地不动；滚过末集 = 剧终，跟随宿主刷下一部（本实现无
    /// 下一季接力，与信息流回落路径一致）。
    fn step_episode(&mut self, dir: i32, cx: &mut Context<Self>) {
        let Some(now) = self.now.clone() else {
            return;
        };
        let next = now.vid_index as i64 + dir as i64;
        if next < 1 {
            return;
        }
        if next as usize > now.episode_count {
            self.step(1, cx);
            return;
        }
        if let Some(item) = self
            .display_items()
            .iter()
            .find(|i| i.series_id == now.series_id)
            .cloned()
        {
            self.play_target(item, Some(next as u32), None, cx);
        }
    }

    /// 滚轮切剧（Tauri 同参：400ms 冷却 + 小位移忽略）。连播锁定时语义
    /// 变为切集，不跟随换剧。
    fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        if self.in_binge() {
            // Tauri 同款：滚轮也是「用户在场」，切集时唤醒悬浮层
            self.wake_controls(cx);
            self.step_episode(dir, cx);
            return;
        }
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
            self.play_target(item, None, None, cx);
        }
        cx.notify();
    }

    // —— 播放编排 ——

    /// 切到目标剧。旧的引擎继续播到新引擎就位（对齐 Tauri「store 目标
    /// 变了但画面还在上一部」的切换语义），就位即整体换装。
    /// `start_override` 给切清晰度保进度用（None = 走起点决策）。
    fn play_target(
        &mut self,
        item: StreamItem,
        forced_next_ep: Option<u32>,
        start_override: Option<f64>,
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
        // 旧的帧立即退场：切换期间画面上是新目标的封面，不是上一部的残帧
        self.frame = None;
        #[cfg(target_os = "windows")]
        self.clear_frame_image();
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
        #[cfg(target_os = "windows")]
        let video_target = Rc::clone(&self.video_target);
        #[cfg(target_os = "windows")]
        self.spawn_event_loop(epoch, rx, video_target, cx);
        #[cfg(not(target_os = "windows"))]
        self.spawn_event_loop(epoch, rx, cx);
        let done = bridge::spawn(Self::play_task(
            state,
            item,
            forced_next_ep,
            history,
            self.preferred_definition,
            start_override,
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
                        // 新引擎就位（帧随后由它的 Frame 事件送到，这里不清帧：
                        // 起了才清的写法会把先到的首帧抹掉，封面从此收不掉）。
                        // 切换途中旧帧已清，到这里的画面本来就是封面占位。
                        // 偏好重放：换集重建引擎（对齐 Tauri `<video>` 卸载重挂）后
                        // 把倍速/音量/静音按存值发给新引擎。
                        handle.set_rate(view.rate);
                        handle.set_volume(view.volume);
                        handle.set_muted(view.muted);
                        view.player = Some(handle);
                        // 连播锁定跟着目标走（Tauri setTarget 同款）：切到
                        // 别的剧即解除，同剧连播保持
                        if view.binge.as_deref() != Some(now.series_id.as_str()) {
                            view.binge = None;
                        }
                        view.load_intro(now.series_id.clone(), cx);
                        // 弹幕按集重拉（vid 变了才拉；社交态不缓存复用）
                        view.load_danmaku(now.vid.clone(), cx);
                        // 互动数据：集级公开计数（档案）+ mget 最近互动
                        view.load_episode_counts(&now, cx);
                        view.load_interact_state(cx);
                        view.now = Some(now);
                        view.switching = false;
                        view.buffering = false;
                        log::info!("[Home] 引擎就位（epoch={epoch}），switching 解除");
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
    /// `preferred_definition` 是用户显式选的清晰度（None = 自动/沿用）；
    /// `start_override` 是切档保进度的位置覆盖。
    async fn play_task(
        state: Arc<AppStateInner>,
        item: StreamItem,
        forced_next_ep: Option<u32>,
        history: Vec<WatchHistoryItem>,
        preferred_definition: Option<u32>,
        start_override: Option<f64>,
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
        // 切档保进度：覆盖值优先于起点决策（覆盖必然伴随显式集号——
        // 切清晰度 = 同集同位重播）
        let (vid_index, start_secs) = match (start_override, forced_next_ep) {
            (Some(pos), forced) => (forced.unwrap_or(1), pos),
            (None, Some(idx)) => (idx, 0.0),
            (None, None) => {
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
        // 找集号：连播是「上一集接着往下」——目标集不存在时必须报错，绝不能
        // 回落到第 1 集（那是「又从头播一遍」的来源，用户看到的正是重复）。
        // 只有起点决策（forced_next_ep 为空）才允许回落到第 1 集。
        let ep = match series.episodes.iter().find(|e| e.vid_index == vid_index) {
            Some(ep) => ep,
            None if forced_next_ep.is_some() => {
                return Err(hongguo_common::AppError::Media(format!(
                    "第 {vid_index} 集不在分集表里（表里 {} 集），不回落重播",
                    series.episodes.len()
                )));
            }
            None => series
                .episodes
                .iter()
                .find(|e| e.vid_index == 1)
                .ok_or_else(|| hongguo_common::AppError::Media("分集表里没有可播的集".into()))?,
        };
        if ep.vid.trim().is_empty() {
            return Err(hongguo_common::AppError::Media("该集没有 vid".into()));
        }
        let progress_key = format!("{}:{}", item.series_id, ep.vid_index);
        // 实际要播的 vid：比对「同一集被播了两遍」与「恰好落在第一集」的
        // 唯一凭证（重复内容投诉时先看这行，再看起播目标那行）
        log::info!(
            "[Home] 起播 {} 第 {} 集（vid={}）起点 {:.1}s",
            item.series_id,
            ep.vid_index,
            ep.vid,
            start_secs
        );
        let prepared = prepare(
            &runtime,
            &ep.vid,
            &progress_key,
            preferred_definition,
            settings,
            &env,
        )
        .await?;
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
                definitions: prepared.definitions,
                definition: prepared.definition,
                vid: ep.vid.clone(),
            },
        ))
    }

    /// 拉当前剧的简介（video_detail 的 series_intro，Tauri
    /// useSeriesDetailMeta 同源）：装饰性信息，失败静默降级、允许重试；
    /// 每剧只拉一次（Tauri staleTime 10 分钟同语义的会话内缓存）。
    fn load_intro(&mut self, series_id: String, cx: &mut Context<Self>) {
        if self.intros.contains_key(&series_id) || !self.intros_pending.insert(series_id.clone()) {
            return;
        }
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            let sid = series_id.clone();
            hongguo_api::detail::fetch_series_meta(&sid, &env)
                .await
                .map(|meta| (sid.clone(), meta.intro))
                .map_err(|_| sid)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| match result {
                Ok((sid, intro)) => {
                    view.intros_pending.remove(&sid);
                    if !intro.is_empty() {
                        view.intros.insert(sid, intro);
                    }
                }
                Err(sid) => {
                    // 失败不算数：允许下次起播重试
                    view.intros_pending.remove(&sid);
                }
            });
        })
        .detach();
    }

    /// 拉当前集的全部弹幕（group_id = vid，book_id = series_id；社交态
    /// 不缓存，每次进集都重拉——Tauri useDanmaku 同款）。拉完游标归零，
    /// 由 tick 按「当前播放位置之前的不补放」自然对齐。
    fn load_danmaku(&mut self, vid: String, cx: &mut Context<Self>) {
        if self.danmaku_fetching || self.danmaku_vid == vid {
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        self.danmaku_fetching = true;
        self.danmaku_vid = vid.clone();
        // 新集清空在飞与游标（当前播放位置之前的不会补放）
        self.danmaku_cursor = 0;
        self.danmaku_flights.clear();
        self.danmaku_lane_free.clear();
        let state = Arc::clone(&self.state);
        let sid = now.series_id.clone();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_danmaku_all(&vid, &sid, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.danmaku_fetching = false;
                match result {
                    Ok(items) => {
                        log::debug!(
                            "[Home] 弹幕拉取 {} 条（{}）offsets={:?}",
                            items.len(),
                            view.danmaku_vid,
                            items.iter().map(|d| d.offset_ms).collect::<Vec<_>>()
                        );
                        view.danmaku_items = Arc::new(items);
                        view.danmaku_cursor = 0;
                    }
                    Err(e) => {
                        // 拉取失败留空列表（装饰性数据，不影响主功能），
                        // 同集内不自动重试
                        log::warn!("[Home] 弹幕拉取失败: {e}");
                        view.danmaku_items = Arc::new(Vec::new());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 弹幕 tick（每个 Frame 事件驱动，播放中 ~30fps）：seek 跳变检测 →
    /// 入场（offset 落在 (上一帧, 当前] 的按密度抽样发射）→ 在飞推进与
    /// 出屏回收。暂停时没有 Frame 事件，位置自然冻结。
    fn danmaku_tick(&mut self, cx: &mut Context<Self>) {
        if !self.danmaku_on || self.danmaku_items.is_empty() {
            return;
        }
        let Some(player) = &self.player else {
            return;
        };
        let pos = player.position_secs();
        let paused = player.is_paused();
        // seek 跳变（倒跳/大幅前跳）：游标二分重置 + 清空在飞
        if pos < self.danmaku_last_pos - 0.05 || pos > self.danmaku_last_pos + 1.5 {
            self.danmaku_cursor = self
                .danmaku_items
                .partition_point(|d| (d.offset_ms as f64) / 1000.0 <= pos);
            self.danmaku_flights.clear();
            self.danmaku_lane_free.clear();
        }
        self.danmaku_last_pos = pos;

        let now = std::time::Instant::now();
        let dt = self
            .danmaku_last_tick
            .replace(now)
            .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        let stage_w = self
            .stage_rects
            .get()
            .and_then(|r| r.first().copied())
            .unwrap_or(1024.0) as f64;
        // 在飞推进（暂停不推进 = 冻结）与出屏回收（progress ≥ 1 滚完一屏）
        if !paused && !self.danmaku_flights.is_empty() {
            let step = dt / DM_CROSS_MS;
            for f in &mut self.danmaku_flights {
                f.progress += step;
            }
        }
        self.danmaku_flights.retain(|f| f.progress < 1.0);

        // 入场：offset 落在 (上一帧, 当前] 的按密度抽样发射（随机均匀：
        // 丢哪条无所谓，要的是同屏条数按比例变稀）
        let ms = (pos * 1000.0) as u64;
        let font_px = self.dm_font_px(stage_w as f32);
        let lanes = self.dm_lane_count(stage_w as f32);
        if self.danmaku_lane_free.len() != lanes {
            self.danmaku_lane_free = vec![0.0; lanes];
        }
        let mut spawned = 0usize;
        while self.danmaku_cursor < self.danmaku_items.len() {
            let d = self.danmaku_items[self.danmaku_cursor].clone();
            if d.offset_ms > ms {
                break;
            }
            self.danmaku_cursor += 1;
            if d.text.is_empty() || self.dm_rand() > self.dm_density {
                continue;
            }
            // 泳道：挑最早空出来的那条并占位（叠满时自然排队）
            let mut pick = 0usize;
            for (i, free) in self.danmaku_lane_free.iter().enumerate() {
                if *free < self.danmaku_lane_free[pick] {
                    pick = i;
                }
            }
            if let Some(free) = self.danmaku_lane_free.get_mut(pick) {
                *free = pos + DM_LANE_BUSY_S;
            }
            self.danmaku_flights.push(DmFlight {
                text: d.text.clone(),
                lane: pick,
                width: dm_text_width(&d.text, font_px),
                progress: 0.0,
            });
            spawned += 1;
        }
        if spawned > 0 {
            log::debug!("[Home] 弹幕入场 {} 条（pos={pos:.1}s）", spawned);
            cx.notify();
        }
    }

    /// 密度抽样的廉价伪随机（LCG；只用于「丢不丢这一条」，无需质量）。
    fn dm_rand(&mut self) -> f32 {
        self.dm_rng = self
            .dm_rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.dm_rng >> 33) as f32) / (u32::MAX as f32)
    }

    /// 基准字号（hgplayer：height/30，钳 15–22）× 用户缩放。
    fn dm_font_px(&self, stage_h: f32) -> f32 {
        let base = (stage_h / 30.0).clamp(DM_FONT_MIN, DM_FONT_MAX);
        (base * self.dm_font_scale).max(10.0).round()
    }

    /// 泳道数：显示区域高度 ÷（字号 + 10），下限 4。
    fn dm_lane_count(&self, stage_h: f32) -> usize {
        let font_px = self.dm_font_px(stage_h);
        let lanes = ((stage_h * self.dm_area) / (font_px + DM_LANE_GAP)).floor() as usize;
        lanes.max(DM_MIN_LANES)
    }

    /// 集级公开计数（剧集档案 detail 下发）：从本地档案读当前集的
    /// 评论数/点赞数（匿名可见的装饰性计数，阻塞池读 store）。
    fn load_episode_counts(&mut self, now: &NowPlaying, cx: &mut Context<Self>) {
        let state = Arc::clone(&self.state);
        let (sid, vid_index) = (now.series_id.clone(), now.vid_index);
        self.ep_comment_count = 0;
        self.ep_digg_count = 0;
        let rx = bridge::spawn(async move {
            tokio::task::spawn_blocking(move || {
                state
                    .store
                    .series_by_id(&sid)
                    .ok()
                    .flatten()
                    .and_then(|s| s.episodes.into_iter().find(|e| e.vid_index == vid_index))
                    .map(|e| (e.comment_count, e.digg_count))
            })
            .await
            .expect("阻塞池 join")
        });
        cx.spawn(async move |this, cx| {
            let counts = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| {
                if let Some((comment_count, digg_count)) = counts {
                    view.ep_comment_count = comment_count;
                    view.ep_digg_count = digg_count;
                }
            });
        })
        .detach();
    }

    /// mget 最近互动（登录后才有「已赞/已追」红标；匿名/失败静默空表）。
    fn load_interact_state(&mut self, cx: &mut Context<Self>) {
        if self.interact_loaded {
            return;
        }
        self.interact_loaded = true;
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_interaction_state(&env)
                .await
                .map(|s| s.items)
                .unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let items = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| {
                view.interact_items = Arc::new(items);
            });
        })
        .detach();
    }

    /// 登录态（settings 账号卡片的 user_id 非空）。
    fn logged_in(&self) -> bool {
        self.state
            .settings()
            .account
            .as_ref()
            .is_some_and(|a| !a.user_id.is_empty())
    }

    /// 点赞当前集（裸 vid 为点赞对象）：乐观翻转 + 计数 ±1，失败回滚。
    fn toggle_digg(&mut self, cx: &mut Context<Self>) {
        if !self.logged_in() {
            log::info!("[Home] 点赞需要登录（settings 里没有账号），已忽略");
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        let bare_vid = now.vid.clone();
        if bare_vid.is_empty() || self.digg_pending {
            return;
        }
        // 当前态：mget 命中优先，否则按未赞
        let digged = self
            .interact_items
            .iter()
            .find(|i| i.vid == bare_vid)
            .map(|i| i.user_digg)
            .unwrap_or(false);
        let next = !digged;
        self.digg_pending = true;
        // 乐观：互动列表覆盖 + 计数 ±1
        let mut items = (*self.interact_items).clone();
        if let Some(hit) = items.iter_mut().find(|i| i.vid == bare_vid) {
            hit.user_digg = next;
            hit.digged_count += if next { 1 } else { -1 };
        } else {
            items.push(InteractionItem {
                vid: bare_vid.clone(),
                series_id: now.series_id.clone(),
                user_digg: next,
                digged_count: if next { 1 } else { 0 },
                followed: false,
                followed_cnt: 0,
                ..Default::default()
            });
        }
        self.interact_items = Arc::new(items);
        self.ep_digg_count += if next { 1 } else { -1 };
        let sid = now.series_id.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            digg_video(&bare_vid, &sid, next, &env).await.map(|_| next)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.digg_pending = false;
                if let Err(e) = result {
                    // 失败回滚（下次重试）
                    log::warn!("[Home] 点赞失败: {e}");
                    view.load_interact_state(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 收藏/取消收藏当前剧（乐观翻转 + 收藏计数 ±1，失败回滚）。
    fn toggle_collect(&mut self, cx: &mut Context<Self>) {
        if !self.logged_in() {
            log::info!("[Home] 收藏需要登录（settings 里没有账号），已忽略");
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        if self.collect_pending {
            return;
        }
        let collected = self
            .interact_items
            .iter()
            .find(|i| i.series_id == now.series_id)
            .map(|i| i.followed)
            .unwrap_or(false);
        let next = !collected;
        self.collect_pending = true;
        let mut items = (*self.interact_items).clone();
        if let Some(hit) = items.iter_mut().find(|i| i.series_id == now.series_id) {
            hit.followed = next;
            hit.followed_cnt += if next { 1 } else { -1 };
        } else {
            items.push(InteractionItem {
                series_id: now.series_id.clone(),
                followed: next,
                followed_cnt: if next { 1 } else { 0 },
                ..Default::default()
            });
        }
        self.interact_items = Arc::new(items);
        self.follow_count += if next { 1 } else { -1 };
        let sid = now.series_id.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            collect_series(&sid, next, &env).await.map(|_| next)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.collect_pending = false;
                if let Err(e) = result {
                    log::warn!("[Home] 收藏失败: {e}");
                    view.load_interact_state(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 分享：剧名 + 详情链接复制进剪贴板。
    fn share_series(&mut self, cx: &mut Context<Self>) {
        let Some(now) = &self.now else {
            return;
        };
        let text = format!(
            "{} https://hongguoduanju.com/detail?series_id={}",
            now.title, now.series_id
        );
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
    }

    /// 开关下载面板；打开时刷新任务快照与完成角标。
    fn toggle_downloads(&mut self, cx: &mut Context<Self>) {
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

    /// 当前集的评论键就绪（vid 非空才可拉）。
    fn comments_ready(&self) -> bool {
        self.now.as_ref().is_some_and(|n| !n.vid.is_empty())
    }

    /// 评论面板数据：拉首页（社交态不缓存复用；换集重置）。
    fn load_comments_first_page(&mut self, cx: &mut Context<Self>) {
        let Some(now) = self.now.clone() else {
            return;
        };
        if self.comments_loading || !self.comments_ready() {
            return;
        }
        // 同集且已有数据：本会话内保留（面板重开不重拉）
        if self.comments_vid == now.vid && !self.comments.is_empty() {
            return;
        }
        self.comments_loading = true;
        self.comments_vid = now.vid.clone();
        self.comments_cursor.clear();
        self.comments_has_more = false;
        let sid = now.series_id.clone();
        let vid = now.vid.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_comments_page(&vid, &sid, "", &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.comments_loading = false;
                match result {
                    Ok(page) => {
                        view.comments_total = page.total;
                        view.comments_has_more = page.has_more;
                        view.comments_cursor = page.next_cursor;
                        view.comments = Arc::new(page.items);
                    }
                    Err(e) => log::warn!("[Home] 评论拉取失败: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 评论翻页（滚到尾部加载下一页；相邻页重叠条目按 comment_id 去重）。
    fn load_comments_next(&mut self, cx: &mut Context<Self>) {
        if self.comments_loading || !self.comments_has_more || !self.comments_ready() {
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        self.comments_loading = true;
        let sid = now.series_id.clone();
        let vid = now.vid.clone();
        let cursor = self.comments_cursor.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_comments_page(&vid, &sid, &cursor, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.comments_loading = false;
                if let Ok(page) = result {
                    view.comments_has_more = page.has_more;
                    view.comments_cursor = page.next_cursor;
                    let mut items = (*view.comments).clone();
                    for c in page.items {
                        if !items.iter().any(|x| x.comment_id == c.comment_id) {
                            items.push(c);
                        }
                    }
                    view.comments = Arc::new(items);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 发评论（reply/add 与 comment/add 不同端点；**服务端完整回显对象**
    /// 直接插列表顶部——抓包三点一线：头像/昵称/uid/时间齐全，不自拼）。
    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.comment_input else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() || self.comment_sending || !self.comments_ready() {
            return;
        }
        if !self.logged_in() {
            log::info!("[Home] 发评论需要登录（settings 里没有账号），已忽略");
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        self.comment_sending = true;
        let state = Arc::clone(&self.state);
        let (vid, sid) = (now.vid.clone(), now.series_id.clone());
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            send_comment(&vid, &sid, &text, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.comment_sending = false;
                match result {
                    Ok(item) => {
                        let mut items = (*view.comments).clone();
                        items.insert(0, item);
                        view.comments = Arc::new(items);
                        view.comments_total += 1;
                        view.ep_comment_count += 1;
                        view.comment_clear_input = true;
                    }
                    Err(e) => log::warn!("[Home] 评论发送失败: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 评论点赞（乐观 ±1 + 红心翻转，失败回滚到服务端态）。
    fn toggle_comment_digg(&mut self, comment_id: String, cx: &mut Context<Self>) {
        if !self.logged_in() {
            log::info!("[Home] 评论点赞需要登录，已忽略");
            return;
        }
        let Some(item) = self
            .comments
            .iter()
            .find(|c| c.comment_id == comment_id)
            .cloned()
        else {
            return;
        };
        let current = self
            .comment_digg_override
            .get(&comment_id)
            .copied()
            .unwrap_or(item.user_digg);
        let next = !current;
        let delta = if next { 1 } else { -1 };
        self.comment_digg_override.insert(comment_id.clone(), next);
        let mut items = (*self.comments).clone();
        if let Some(hit) = items.iter_mut().find(|c| c.comment_id == comment_id) {
            hit.digg_count += delta;
        }
        self.comments = Arc::new(items);
        let state = Arc::clone(&self.state);
        let cid = comment_id.clone();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            digg_comment(&cid, next, &env).await.map(|_| next)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| {
                if let Err(e) = result {
                    log::warn!("[Home] 评论点赞失败: {e}");
                    view.comment_digg_override.remove(&comment_id);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// 展开/收起回复（展开才拉首页；发送回复成功后强制展开让本地追加
    /// 的那条立刻可见——Tauri 同款）。
    fn toggle_replies(&mut self, comment_id: String, reply_count: i64, cx: &mut Context<Self>) {
        if self.replies_open.remove(&comment_id) {
            cx.notify();
            return;
        }
        self.replies_open.insert(comment_id.clone());
        cx.notify();
        if reply_count <= 0 || self.replies.contains_key(&comment_id) {
            return;
        }
        self.fetch_replies(comment_id, String::new(), cx);
    }

    /// 拉一页回复（首页或翻页）。
    fn fetch_replies(&mut self, comment_id: String, cursor: String, cx: &mut Context<Self>) {
        if self.replies_loading.contains(&comment_id) {
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        self.replies_loading.insert(comment_id.clone());
        let sid = now.series_id.clone();
        let vid = now.vid.clone();
        let cid = comment_id.clone();
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            fetch_comment_replies(&vid, &sid, &cid, &cursor, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| {
                view.replies_loading.remove(&comment_id);
                match result {
                    Ok(page) => {
                        view.replies_cursor
                            .insert(comment_id.clone(), page.next_cursor);
                        let mut items = view
                            .replies
                            .get(&comment_id)
                            .map(|v| (**v).clone())
                            .unwrap_or_default();
                        for r in page.items {
                            if !items.iter().any(|x| x.reply_id == r.reply_id) {
                                items.push(r);
                            }
                        }
                        view.replies.insert(comment_id, Arc::new(items));
                    }
                    Err(e) => log::warn!("[Home] 回复拉取失败: {e}"),
                }
            });
        })
        .detach();
    }

    /// 发回复（本地追加一份——服务端列表有延迟；追加后强制展开）。
    fn submit_reply(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.comment_input else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        let Some((comment_id, reply_to_reply_id, _name)) = self.reply_to.clone() else {
            return;
        };
        if text.is_empty() || self.comment_sending {
            return;
        }
        if !self.logged_in() {
            log::info!("[Home] 回复需要登录（settings 里没有账号），已忽略");
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        self.comment_sending = true;
        let state = Arc::clone(&self.state);
        let (vid, sid) = (now.vid.clone(), now.series_id.clone());
        let cid = comment_id.clone();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            send_reply(&vid, &sid, &cid, reply_to_reply_id.as_deref(), &text, &env).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.comment_sending = false;
                match result {
                    Ok(reply) => {
                        let mut items = view
                            .replies
                            .get(&comment_id)
                            .map(|v| (**v).clone())
                            .unwrap_or_default();
                        items.push(reply);
                        view.replies.insert(comment_id.clone(), Arc::new(items));
                        view.replies_open.insert(comment_id);
                        view.reply_to = None;
                        view.comment_clear_input = true;
                    }
                    Err(e) => log::warn!("[Home] 回复发送失败: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 引擎事件循环（每个引擎一个，epoch 封口：换剧后陈旧事件自然断流）。
    fn spawn_event_loop(
        &mut self,
        epoch: u64,
        mut events: mpsc::Receiver<PlayerEvent>,
        #[cfg(target_os = "windows")] video_target: Rc<Cell<(usize, usize)>>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                // 合并积压：Frame 只保最新一批里的最后一帧（1080p 转换 ~50ms、
                // 上屏只认最新，旧帧转换纯属浪费），其余事件一个不丢、按序处理。
                let mut batch = Vec::with_capacity(4);
                batch.push(first);
                while let Ok(next) = events.try_recv() {
                    batch.push(next);
                }
                let last_frame = batch.iter().rposition(|e| matches!(e, PlayerEvent::Frame(_)));
                'batch: for (i, event) in batch.into_iter().enumerate() {
                    if matches!(event, PlayerEvent::Frame(_)) && Some(i) != last_frame {
                        continue;
                    }
                    // Windows：NV12→RGBA 在事件任务里转换（每帧几十毫秒，不占
                    // UI 线程）；macOS 是 CVPixelBuffer 零拷贝，无需转换
                    #[cfg(target_os = "windows")]
                    let converted = match &event {
                        PlayerEvent::Frame(frame) => {
                            Self::render_image_of(frame, video_target.get())
                        }
                        _ => None,
                    };
                    let ok = this
                        .update(cx, |view, cx| {
                            if view.epoch != epoch {
                                log::warn!(
                                    "[Home] 事件被纪元弃置（loop={epoch}，view={}）——事件到得了 view 却进不去",
                                    view.epoch
                                );
                                return;
                            }
                            #[cfg(target_os = "windows")]
                            if let Some(image) = converted {
                                view.set_frame_image(image);
                            }
                            view.on_player_event(event, cx);
                        })
                        .is_ok();
                    if !ok {
                        break 'batch;
                    }
                }
            }
        })
        .detach();
    }

    /// 换上新渲染产物，旧图送进回收队列（见 `retired_images` 头注释：
    /// gpui 纹理只增不删，必须显式 `drop_image`）。
    #[cfg(target_os = "windows")]
    fn set_frame_image(&mut self, image: Arc<RenderImage>) {
        // 兜底上限：render 排不上队（后台/卡顿）时，回收队列也不能无界涨
        //（被挤掉的旧图其纹理可能滞留图集，代价有界：≤16 × 输出尺寸）
        const RETIRED_KEEP: usize = 16;
        while self.retired_images.len() >= RETIRED_KEEP {
            self.retired_images.remove(0);
        }
        if let Some(old) = self.frame_image.replace(image) {
            self.retired_images.push(old);
        }
    }

    /// 渲染产物退场（切剧/连播），旧图同样进回收队列。
    #[cfg(target_os = "windows")]
    fn clear_frame_image(&mut self) {
        if let Some(old) = self.frame_image.take() {
            self.retired_images.push(old);
        }
    }

    /// contain 适配：源 (w,h) 缩进目标盒（不放大、保宽高比、至少 2px）。
    #[cfg(target_os = "windows")]
    fn contain_fit(w: usize, h: usize, box_w: usize, box_h: usize) -> (usize, usize) {
        let scale = (box_w as f64 / w as f64)
            .min(box_h as f64 / h as f64)
            .min(1.0);
        (
            (w as f64 * scale).round().max(2.0) as usize,
            (h as f64 * scale).round().max(2.0) as usize,
        )
    }

    /// Windows 渲染转换：NV12（BT.709 limited）→ RGBA 图像，**按舞台显示
    /// 尺寸缩放输出**（见 `video_target` 头注释）。转换跑在本事件任务线程。
    #[cfg(target_os = "windows")]
    fn render_image_of(frame: &VideoFrame, target: (usize, usize)) -> Option<Arc<RenderImage>> {
        // Windows 目标上 FrameBuffer 只有 Nv12 一个变体（可驳尽，同 mac 侧
        // CVPixelBuffer 的口径）
        static CONVERTED: AtomicU32 = AtomicU32::new(0);
        let t0 = Instant::now();
        let FrameBuffer::Nv12(nv12) = &frame.buffer;
        let (tw, th) = Self::contain_fit(nv12.width, nv12.height, target.0, target.1);
        let rgba = hongguo_media::nv12_rgba::nv12_to_rgba_bt709_scaled(
            &nv12.data,
            nv12.stride,
            nv12.width,
            nv12.height,
            tw,
            th,
        );
        let buf = image::RgbaImage::from_raw(tw as u32, th as u32, rgba)?;
        // 取证开关：HONGGUO_DUMP_NV12=<前缀> 时第 0/200/600 帧落盘 NV12 +
        // 我们的转换 PNG（与 ffmpeg 参照对拍用；多时刻采样，抓"某些帧才
        // 错"类问题——首帧对不代表全程对）
        static DUMPED: AtomicU32 = AtomicU32::new(0);
        let n_dump = DUMPED.fetch_add(1, Ordering::Relaxed);
        if n_dump.is_multiple_of(500)
            && let Ok(prefix) = std::env::var("HONGGUO_DUMP_NV12")
        {
            let tag = if n_dump == 0 {
                "f0".to_string()
            } else {
                format!("f{n_dump}")
            };
            let _ = std::fs::write(
                std::path::PathBuf::from(&prefix).with_extension(format!("{tag}.nv12")),
                nv12.data.as_slice(),
            );
            let _ = buf
                .save(std::path::PathBuf::from(&prefix).with_extension(format!("{tag}.ours.png")));
            log::info!(
                "[Home] NV12 取证 #{n_dump} 已落盘：{prefix}.{tag}.nv12/.ours.png（{}×{} stride {}，转换目标 {tw}×{th}）",
                nv12.width,
                nv12.height,
                nv12.stride
            );
        }
        let image = Arc::new(crate::image_upload::render_image(buf));
        let n = CONVERTED.fetch_add(1, Ordering::Relaxed);
        if n < 3 || n.is_multiple_of(200) {
            log::info!(
                "[Home] NV12→RGBA 转换 #{n} 耗时 {:.1}ms（{tw}×{th}，源 {}×{}）",
                t0.elapsed().as_secs_f64() * 1000.0,
                nv12.width,
                nv12.height
            );
        }
        Some(image)
    }

    fn on_player_event(&mut self, event: PlayerEvent, cx: &mut Context<Self>) {
        match event {
            PlayerEvent::FirstFrame { .. } => {
                // 帧由紧随其后的 Frame 事件带上；这里只做「出画了」的副作用
                self.prefetch_next_stream(cx);
            }
            PlayerEvent::Frame(frame) => {
                let first = self.frame.is_none();
                if first {
                    log::info!("[Home] 首帧进入 view——video_ready 即为 true");
                }
                self.frame = Some(frame);
                self.save_progress_maybe();
                // 弹幕 tick：入场 + 在飞推进（Frame 事件即渲染节拍）
                self.danmaku_tick(cx);
                // 相邻剧分集档案预取（换剧秒回，Tauri 同节奏）
                self.prefetch_neighbor_archives();
            }
            // 引擎缓冲状态机：水位低暂停设备、回填恢复（详见 playback::player）
            PlayerEvent::Buffering => self.buffering = true,
            PlayerEvent::Ready => self.buffering = false,
            PlayerEvent::Ended => {
                self.buffering = false;
                self.advance_after_end(cx);
            }
            PlayerEvent::Failed(msg) => {
                self.buffering = false;
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

    // —— 控制区（对齐 Tauri 版控制栏核心件） ——

    /// 倍速档位（与 Tauri 版 `player-controls.tsx` 的 RATES 完全一致）。
    const RATES: [f64; 6] = [0.75, 1.0, 1.25, 1.5, 2.0, 3.0];
    /// 控制条可见时长：鼠标静止/无操作后自动隐藏。
    const CONTROLS_VISIBLE: Duration = Duration::from_secs(3);
    /// 拖动中实时 seek 的最小间隔：引擎一次 seek = 冲流 + 解码器重建，
    /// 250ms 限频跟手又不拖垮供给（Tauri 侧浏览器自行合并，这里显式限）。
    const SCRUB_SEEK_MIN_INTERVAL: Duration = Duration::from_millis(250);

    fn subscribe_controls(&mut self, cx: &mut Context<Self>) {
        // subscribe 的处理器签名与 listener 不同（实体 + 事件，无 window），
        // 直接给闭包再转发到方法
        cx.subscribe(
            &self.volume_slider,
            |this: &mut Self, _: Entity<SliderState>, event: &SliderEvent, cx| {
                this.on_volume_event(event, cx);
            },
        )
        .detach();
        // 弹幕设置滑杆：Change 即生效，Release 落盘（音量滑杆同节奏）
        let dm_sliders = [
            (&self.dm_opacity_slider, 0u8),
            (&self.dm_font_slider, 1),
            (&self.dm_density_slider, 2),
            (&self.dm_area_slider, 3),
        ];
        for (slider, which) in dm_sliders {
            cx.subscribe(
                slider,
                move |this: &mut Self, _: Entity<SliderState>, event: &SliderEvent, cx| {
                    this.on_dm_slider_event(which, event, cx);
                },
            )
            .detach();
        }
        // 弹幕发送框：Enter 提交
        if let Some(input) = &self.danmaku_input {
            cx.subscribe(
                input,
                |this: &mut Self, _: Entity<InputState>, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_danmaku(cx);
                    }
                },
            )
            .detach();
        }
        // 评论输入框：Enter 提交
        if let Some(input) = &self.comment_input {
            cx.subscribe(
                input,
                |this: &mut Self, _: Entity<InputState>, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.submit_comment(cx);
                    }
                },
            )
            .detach();
        }
    }

    /// 弹幕设置滑杆：更新对应显示参数；Release 落盘。
    fn on_dm_slider_event(&mut self, which: u8, event: &SliderEvent, cx: &mut Context<Self>) {
        let SliderValue::Single(v) = (match event {
            SliderEvent::Change(v) | SliderEvent::Release(v) => v,
        }) else {
            return;
        };
        match which {
            0 => self.dm_opacity = *v,
            1 => self.dm_font_scale = *v,
            2 => self.dm_density = *v,
            3 => self.dm_area = *v,
            _ => {}
        }
        if matches!(event, SliderEvent::Release(_)) {
            self.persist_prefs();
        }
        cx.notify();
    }

    /// 提交弹幕（发送框 Enter 或发送按钮）：未登录给日志指路（壳无 toast
    /// 基建），成功后本地乐观插入（ hgplayer 同款：发送的弹幕立刻上屏）。
    fn submit_danmaku(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.danmaku_input else {
            return;
        };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() || self.danmaku_sending {
            return;
        }
        let Some(now) = self.now.clone() else {
            return;
        };
        if now.vid.is_empty() {
            return;
        }
        let logged_in = self
            .state
            .settings()
            .account
            .as_ref()
            .is_some_and(|a| !a.user_id.is_empty());
        if !logged_in {
            log::info!("[Home] 弹幕发送需要登录（settings 里没有账号），已忽略");
            return;
        }
        let Some(player) = &self.player else {
            return;
        };
        let offset_ms = (player.position_secs() * 1000.0) as u64;
        self.danmaku_sending = true;
        let state = Arc::clone(&self.state);
        let (vid, sid) = (now.vid.clone(), now.series_id.clone());
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            send_danmaku(&vid, &sid, &text, offset_ms, &env)
                .await
                .map(|cid| (text, offset_ms, cid))
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.danmaku_sending = false;
                match result {
                    Ok((text, offset_ms, cid)) => {
                        log::info!("[Home] 弹幕已发送 cid={cid}");
                        // 本地乐观插入：排序进 items（cursor 会自然捡起上屏）。
                        // 拉取任务还握着 Arc 时插不进去就算了——飞行已直接上屏
                        let item = Danmaku {
                            comment_id: cid,
                            text: text.clone(),
                            offset_ms,
                            digg_count: 0,
                        };
                        if let Some(items) = Arc::get_mut(&mut view.danmaku_items) {
                            let at = items.partition_point(|d| d.offset_ms <= offset_ms);
                            items.insert(at, item);
                        }
                        // 输入框清空走渲染期（set_value 需要 window），这里只立标志
                        view.danmaku_clear_input = true;
                    }
                    Err(e) => {
                        log::warn!("[Home] 弹幕发送失败: {e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 音量滑杆：拖动即生效（引擎在设备回调里乘 PCM，零延迟），松手落盘。
    fn on_volume_event(&mut self, event: &SliderEvent, cx: &mut Context<Self>) {
        let value = match event {
            SliderEvent::Change(v) | SliderEvent::Release(v) => v,
        };
        let SliderValue::Single(v) = *value else {
            return;
        };
        self.volume = v;
        // Tauri setVolumeValue 同款：拖到 0 = 静音，拖离 0 = 解除静音
        self.muted = v == 0.0;
        if let Some(player) = &self.player {
            player.set_volume(v);
            player.set_muted(self.muted);
        }
        if matches!(event, SliderEvent::Release(_)) {
            self.persist_prefs();
        }
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let Some(player) = &self.player else {
            return;
        };
        if player.is_paused() {
            player.resume();
        } else {
            player.pause();
        }
        self.wake_controls(cx);
        cx.notify();
    }

    fn set_rate(&mut self, rate: f64, cx: &mut Context<Self>) {
        self.rate = rate;
        if let Some(player) = &self.player {
            player.set_rate(rate);
        }
        self.persist_prefs();
        self.wake_controls(cx);
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        if let Some(player) = &self.player {
            player.set_muted(self.muted);
        }
        self.persist_prefs();
        self.wake_controls(cx);
        cx.notify();
    }

    /// 偏好落盘（数据目录键值文件，见 [`crate::playback_prefs`]）。
    fn persist_prefs(&self) {
        let prefs = crate::playback_prefs::PlaybackPrefs {
            rate: self.rate,
            volume: self.volume,
            muted: self.muted,
            danmaku_on: self.danmaku_on,
            dm_opacity: self.dm_opacity,
            dm_font_scale: self.dm_font_scale,
            dm_density: self.dm_density,
            dm_area: self.dm_area,
        };
        bridge::spawn(async move {
            let path = hongguo_store::paths::data_dir().join(crate::playback_prefs::FILE_NAME);
            let _ =
                tokio::task::spawn_blocking(move || crate::playback_prefs::save_to(&path, &prefs))
                    .await;
        });
    }

    /// 鼠标活动/操作刷新控制条可见窗；隐藏→可见的沿上排一次性定时器，
    /// 静止到期后靠它（或播放中的帧事件流）触发重算隐藏。
    fn wake_controls(&mut self, cx: &mut Context<Self>) {
        let was_hidden = !self.controls_visible();
        self.controls_until = Some(Instant::now() + Self::CONTROLS_VISIBLE);
        if was_hidden {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Self::CONTROLS_VISIBLE).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
            cx.notify();
        }
    }

    fn controls_visible(&self) -> bool {
        self.scrubbing.is_some()
            || self.picker_open
            || self.player.as_ref().is_some_and(|p| p.is_paused())
            || self.controls_until.is_some_and(|t| Instant::now() < t)
    }

    /// 开关评论面板：打开时拉当前集评论首页。
    fn toggle_comments_panel(&mut self, cx: &mut Context<Self>) {
        self.comments_panel_open = !self.comments_panel_open;
        self.wake_controls(cx);
        if self.comments_panel_open {
            self.load_comments_first_page(cx);
        }
        cx.notify();
    }

    /// 开关选集浮层（连选连跳：选中不关，关闭走 Esc/收起/点外部）。
    fn toggle_picker(&mut self, cx: &mut Context<Self>) {
        self.picker_open = !self.picker_open;
        // 重开跟回当前集所在段（覆盖作废，Tauri groupOverride 同语义）
        self.picker_group_override = None;
        self.wake_controls(cx);
        cx.notify();
    }

    /// 切清晰度（Tauri onDefinitionChange 同款）：同集同位重播，缓存按
    /// (vid, 档位) 分条目——切回去不用重新下载，也不动正在播的那一档。
    /// `None` = 回到自动（沿用上轮解析到的档位）。
    fn switch_definition(&mut self, value: Option<u32>, cx: &mut Context<Self>) {
        let Some(now) = self.now.clone() else {
            return;
        };
        // 与当前实际档位同值就不折腾（自动档位下点解析出的那一档也算同值）
        if value.is_none() && self.preferred_definition.is_none() {
            return;
        }
        if value == Some(now.definition) && self.preferred_definition == value {
            return;
        }
        let position = self
            .player
            .as_ref()
            .map(|p| p.position_secs())
            .unwrap_or(0.0);
        self.preferred_definition = value;
        self.wake_controls(cx);
        if let Some(item) = self
            .display_items()
            .iter()
            .find(|i| i.series_id == now.series_id)
            .cloned()
        {
            self.play_target(item, Some(now.vid_index), Some(position), cx);
        }
        cx.notify();
    }

    /// 选集浮层里点某一集：跳集，**不关浮层**（hgplayer 同款连选连跳）；
    /// 信息流里主动选集 = 锁定本剧连播（Tauri onPickEpisode 同款）。
    fn pick_episode(&mut self, vid_index: u32, cx: &mut Context<Self>) {
        let Some(now) = self.now.clone() else {
            return;
        };
        if vid_index != now.vid_index
            && let Some(item) = self
                .display_items()
                .iter()
                .find(|i| i.series_id == now.series_id)
                .cloned()
        {
            self.binge = Some(now.series_id.clone());
            self.play_target(item, Some(vid_index), None, cx);
        }
        self.wake_controls(cx);
        cx.notify();
    }

    /// 进度轨道的指针位置 → 目标秒。轨道矩形缺失（首帧前）返回 `None`。
    fn scrub_target_at(&self, x: gpui_kit::Pixels) -> Option<f64> {
        let bounds = self.progress_bounds.get()?;
        let player = self.player.as_ref()?;
        let dur = player.duration_secs();
        if dur <= 0.0 {
            return None;
        }
        let ratio = ((x.to_f64() - bounds.origin.x.to_f64()) / bounds.size.width.to_f64().max(1.0))
            .clamp(0.0, 1.0);
        Some(ratio * dur)
    }

    /// 收口拖动：松手（轨道内/外都算）把预览位置交给引擎。松手的这次是
    /// 权威定位，**不受节流**——节流窗口内也要照发。
    fn commit_scrub(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.scrubbing.take() {
            self.scrub_seek_now(target, cx);
        }
        self.wake_controls(cx);
        cx.notify();
    }

    /// 立即定位（不受节流）：按下即跳与松手收口共用。
    fn scrub_seek_now(&mut self, target: f64, cx: &mut Context<Self>) {
        self.last_scrub_seek.set(Instant::now());
        if let Some(player) = &self.player {
            player.seek(target);
        }
        self.wake_controls(cx);
        cx.notify();
    }

    /// 拖动中的实时定位（对齐 Tauri ScrubBar 的 pointermove 即 seek）。节流
    /// 窗口内的重复请求直接跳过——预览位置由 `scrubbing` 承载，不受影响；
    /// 松手收口走 [`Self::commit_scrub`]，不受此限。
    fn scrub_seek_throttled(&mut self, target: f64, cx: &mut Context<Self>) {
        if self.last_scrub_seek.get().elapsed() < Self::SCRUB_SEEK_MIN_INTERVAL {
            return;
        }
        self.last_scrub_seek.set(Instant::now());
        if let Some(player) = &self.player {
            player.seek(target);
        }
        self.wake_controls(cx);
        cx.notify();
    }

    /// 键盘相对 seek（秒，可负）：钳到 [0, 时长]，唤醒控制条。
    fn seek_by(&mut self, secs: f64, cx: &mut Context<Self>) {
        let Some(player) = &self.player else {
            return;
        };
        let dur = player.duration_secs();
        let target = (player.position_secs() + secs).clamp(0.0, dur.max(0.0));
        player.seek(target);
        self.wake_controls(cx);
        cx.notify();
    }

    /// `m:ss`（与 Tauri 版时间格式一致）。
    fn fmt_secs(secs: f64) -> String {
        let s = secs.max(0.0) as u64;
        format!("{}:{:02}", s / 60, s % 60)
    }

    /// 倍速按钮文案：整数不带小数点（1x / 1.5x / 0.75x）。
    fn fmt_rate(rate: f64) -> String {
        if rate.fract() == 0.0 {
            format!("{}x", rate as i64)
        } else {
            format!("{rate}x")
        }
    }

    /// 连播：下一集；末集接流内下一部（剧终接力的跨季/换批语义后续对齐）。
    fn advance_after_end(&mut self, cx: &mut Context<Self>) {
        self.frame = None;
        #[cfg(target_os = "windows")]
        self.clear_frame_image();
        // 防重入：Ended 只在时钟走完才发，但引擎收尾、通道重连都可能让同一集
        // 的 Ended 再来一次——那时 vid_index 还停在旧值，会把刚起播的下一集
        // 又推进一格（连着跳集，看着就是「内容来回变」）。切换途中直接吞掉。
        if self.switching {
            return;
        }
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
                self.play_target(item, Some(next_idx), None, cx);
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
                    // 取证开关：HONGGUO_DUMP_COVER=<目录> 时每个封面都落盘
                    // 原始字节 + 我们的解码 PNG（sid 做文件名；离线与 ffmpeg
                    // 参照对拍用）
                    if let Ok(dir) = std::env::var("HONGGUO_DUMP_COVER") {
                        let _ = std::fs::create_dir_all(&dir);
                        let tag: String = sid
                            .chars()
                            .rev()
                            .take(8)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                        let _ = std::fs::write(
                            std::path::Path::new(&dir).join(format!("{tag}.bin")),
                            &bytes,
                        );
                        let rgba = decoded.to_rgba8();
                        let _ =
                            rgba.save(std::path::Path::new(&dir).join(format!("{tag}.ours.png")));
                        let head: Vec<u8> = bytes.iter().take(12).copied().collect();
                        log::info!(
                            "[Home] 封面落盘 {tag}（{}B，首字节 {head:02x?}，{}×{}）",
                            bytes.len(),
                            rgba.width(),
                            rgba.height()
                        );
                    }
                    view.covers.insert(
                        sid,
                        Arc::new(crate::image_upload::render_image(decoded.to_rgba8())),
                    );
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
                                this.play_target(item, None, None, cx);
                            }
                        }
                    }
                    cx.notify();
                }))
            }))
            .into_any_element()
    }

    /// 沉浸舞台：**同一时刻只画一层内容**——出画了画视频，没出画画封面。
    ///
    /// 两层都是绝对定位铺满舞台，谁盖谁由元素顺序决定。曾经两层都画、封面
    /// 垫在视频下面：视频是 object-contain（上下留黑边），封面就从黑边里
    /// 透出来——用户看到的正是「上下各一条封面夹着中间的画面」。二选一
    /// 渲染，结构上不可能叠。
    ///
    /// 层序仍与 Tauri `player-page.tsx` 一致：有画面时是画面，没有时是封面；
    /// 取流/解码的间隙看到的是这部剧的封面而不是黑屏。
    ///
    /// 返回 `Div`（不是 AnyElement）：调用方要挂滚轮监听与 `.id`。
    fn render_stage(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme();
        let video_ready = self.frame.is_some() && !self.switching;
        // 视频输出尺寸跟随舞台显示区（上一帧的布局矩形，Paint 后才精确；
        // contain 保宽高比在转换端算）
        #[cfg(target_os = "windows")]
        if let Some([w, h, ..]) = self.stage_rects.get()
            && w >= 2.0
            && h >= 2.0
        {
            self.video_target.set((w as usize, h as usize));
        }
        // 周期状态取证（5s 一跳）：封面收不收、帧/图/切换位各在哪一档，
        // 一行钉死——排障不用再猜是事件没到还是渲染没跟上。
        if self.stage_diag_at.get().elapsed() >= Duration::from_secs(5) {
            self.stage_diag_at.set(Instant::now());
            #[cfg(target_os = "windows")]
            let image_state = format!(
                "image={}（待回收 {}）",
                self.frame_image.is_some(),
                self.retired_images.len()
            );
            #[cfg(not(target_os = "windows"))]
            let image_state = "image=n/a".to_string();
            log::info!(
                "[Home][stage] 周期状态：video_ready={video_ready} frame={} switching={} {image_state} epoch={}",
                self.frame.is_some(),
                self.switching,
                self.epoch
            );
        }
        // 占位封面取**目标剧**（游标位）：切剧瞬间游标已指向下一部，先出
        // 它的封面；无封面条目（书架种子）退回首帧方向已在播的那部。
        let cover_sid = self
            .display_items()
            .get(self.cursor())
            .map(|i| i.series_id.clone())
            .or_else(|| self.now.as_ref().map(|n| n.series_id.clone()));
        // 封面层：图 + 压暗（Tauri 的 `brightness-[0.55]`；GPUI 没有亮度滤镜，
        // 用等价的 45% 黑罩）。
        let cover: gpui_kit::AnyElement = match cover_sid
            .as_ref()
            .filter(|_| !video_ready)
            .and_then(|s| self.covers.get(s))
        {
            Some(bitmap) => div()
                .absolute()
                .size_full()
                .overflow_hidden()
                .child(
                    img(ImageSource::Render(Arc::clone(bitmap)))
                        .absolute()
                        .size_full()
                        // 竖版封面铺满横屏舞台走 Cover（裁切，不拉伸）——与
                        // Tauri `player-page.tsx` 的 object-cover 同语义。
                        // 别用 Fill：那是拉伸，400×572 的海报会被横向拉宽
                        // ~1.7 倍（人物变胖）。
                        .object_fit(ObjectFit::Cover),
                )
                .child(div().absolute().size_full().bg(gpui_kit::rgba(0x00000073)))
                .into_any_element(),
            None => div().absolute().size_full().into_any_element(),
        };
        // 视频层：两端不同的像素载体，同一张布局皮。
        // - macOS：surface 是**无 measure 函数的布局叶子**，尺寸全靠样式给
        //   ——不给 size 就是 0×0 盒子（taffy 对无上下文的叶子返回零尺寸），
        //   paint_surface 画了个寂寞。Tauri 侧同款 `absolute inset-0
        //   size-full object-contain`，零拷贝上屏。
        // - Windows：gpui 的视频 surface 只有 Apple 目标，NV12 在事件任务里
        //   转成 RGBA 后按普通图像渲染（img 同款 object-contain）。
        // 外层 div 接点击切暂停（对齐 Tauri 点击画面暂停/继续）：控制条是
        // 兄弟层，点它不会冒泡到这里。
        #[cfg(target_os = "macos")]
        let video_layer = |frame: &VideoFrame| -> gpui_kit::AnyElement {
            // Apple 目标上 FrameBuffer 只有 CVPixelBuffer 一个变体（可驳尽）
            let FrameBuffer::CVPixelBuffer(buffer) = &frame.buffer;
            surface(buffer.clone())
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        };
        #[cfg(target_os = "windows")]
        let video_layer = |image: &Arc<RenderImage>| -> gpui_kit::AnyElement {
            img(ImageSource::Render(Arc::clone(image)))
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        };
        let video: gpui_kit::AnyElement = if video_ready {
            div()
                .absolute()
                .size_full()
                .id("stage-video")
                // 点画面：信息流语义（Tauri onStageClick）——未选中时单击 =
                // 「选中这部剧」（进入连播锁定，滚轮/↑↓ 变切集），选中后的
                // 单击才是播放/暂停。顺手把焦点带回舞台锚——弹层菜单收走
                // 焦点后，这是用户最自然的「回到播放操作」路径
                .on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    if !this.in_binge() {
                        if let Some(now) = &this.now {
                            this.binge = Some(now.series_id.clone());
                        }
                        this.wake_controls(cx);
                        cx.notify();
                        return;
                    }
                    this.toggle_pause(cx);
                }))
                .child({
                    #[cfg(target_os = "macos")]
                    {
                        let frame = self.frame.as_ref().expect("video_ready 已验帧在");
                        video_layer(frame)
                    }
                    #[cfg(target_os = "windows")]
                    {
                        match &self.frame_image {
                            Some(image) => video_layer(image),
                            // 转换产物还没到（首帧事件与转换同拍到达，这里
                            // 只是防御）：画黑，下一帧事件就补上
                            None => div().into_any_element(),
                        }
                    }
                })
                .into_any_element()
        } else {
            h_flex()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element()
        };
        // 缓冲角标（右上）：引擎水位状态机的 Buffering 事件驱动，区别于左上
        // 的切剧提示
        let buffering_badge = (self.buffering && self.player.is_some()).then(|| {
            h_flex()
                .absolute()
                .top_2()
                .right_2()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .bg(gpui_kit::rgba(0x000000aa))
                .child(Spinner::new())
                .child(
                    div()
                        .text_xs()
                        .text_color(gpui_kit::rgb(0xffffff))
                        .child("缓冲中"),
                )
        });
        // 控制条（底部悬浮，鼠标活动时可见）
        let controls =
            (self.player.is_some() && self.controls_visible()).then(|| self.render_controls(cx));
        // 弹幕层（画面上方滚动；开着才渲染，层本身不吃任何指针事件）
        let danmaku_layer = (self.danmaku_on && self.player.is_some()).then(|| {
            let stage_w = self
                .stage_rects
                .get()
                .and_then(|r| r.first().copied())
                .unwrap_or(1024.0) as f64;
            let stage_h = self
                .stage_rects
                .get()
                .and_then(|r| r.get(1).copied())
                .unwrap_or(576.0);
            let font_px = self.dm_font_px(stage_h);
            let layer_opacity = self.dm_opacity;
            div()
                .absolute()
                .size_full()
                .overflow_hidden()
                .opacity(layer_opacity)
                .children(self.danmaku_flights.iter().map(|f| {
                    let top = f.lane as f32 * (font_px + DM_LANE_GAP);
                    let x = f.x_offset(stage_w) as f32;
                    div()
                        .absolute()
                        .left(gpui_kit::px(x))
                        .top(gpui_kit::px(top))
                        .whitespace_nowrap()
                        .text_size(gpui_kit::px(font_px))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .text_color(gpui_kit::rgb(0xffffff))
                        .child(SharedString::from(f.text.clone()))
                }))
        });
        // 信息叠加层（左下）：剧名/集数/热度/简介——闲置淡出与控制条
        // 同时机（Tauri chromeShown 同款，静默期不亮）
        let overlay = self.controls_visible().then(|| self.render_overlay(cx));
        // 互动栏（右缘竖排：收藏/评论/点赞/分享）与评论面板（右侧滑出）
        let rail = self
            .controls_visible()
            .then(|| self.render_interaction_rail(cx));
        let comment_panel = self
            .comments_panel_open
            .then(|| self.render_comment_panel(cx));
        let rects = Rc::clone(&self.stage_rects);
        v_flex()
            .flex_1()
            .min_h_0()
            .bg(gpui_kit::rgb(0x000000))
            .relative()
            .overflow_hidden()
            // 播放快捷键（对齐 Tauri use-player-interactions 默认映射）：空格
            // 暂停/继续、←→ 快退/快进 5 秒、↑↓ 切上一部/下一部剧（沉浸流与
            // 滚轮同语义）。键盘事件只派发给焦点子树，焦点锚就在本元素上。
            .key_context("HomeStage")
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    match ev.keystroke.key.as_str() {
                        "space" => this.toggle_pause(cx),
                        "left" => this.seek_by(-5.0, cx),
                        "right" => this.seek_by(5.0, cx),
                        "down" => this.step(1, cx),
                        "up" => this.step(-1, cx),
                        "f" => this.toggle_fullscreen(window, cx),
                        // Esc 固定管「退出选中」（Tauri 同款语义）：全屏 →
                        // 浮层面板 → 连播锁定，逐层退出
                        "escape" => {
                            if window.is_fullscreen() {
                                window.toggle_fullscreen();
                                this.fullscreen = false;
                            } else if this.picker_open {
                                this.picker_open = false;
                                this.picker_group_override = None;
                            } else if this.in_binge() {
                                this.binge = None;
                            }
                            cx.notify();
                        }
                        _ => {}
                    }
                }),
            )
            .child(cover)
            .child(if video_ready {
                video
            } else {
                // 占位层也要铺满舞台：哨兵读的是子层矩形，给个 0×0 的 div
                // 会让日志恒报「视频层 0」，哨兵就成了假警报源
                div().absolute().size_full().into_any_element()
            })
            .children(danmaku_layer)
            .children(overlay)
            .children(rail)
            .children(comment_panel)
            // 下载面板（右侧滑出；字段快照由 Render::render 顶层刷新）
            .children(
                (self.downloads_open && self.player.is_some())
                    .then(|| self.render_downloads_panel(cx)),
            )
            .children(buffering_badge)
            // 连播锁定指示 + 返回（Tauri immersive-info-overlay 同款：左上
            // 「< 第N集」，点它解除锁定；显隐跟随控制条）
            .children(self.in_binge().then(|| {
                let now = self.now.as_ref().expect("in_binge 已验 now 在");
                h_flex()
                    .id("binge-pill")
                    .absolute()
                    .top_3()
                    .left_3()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .pr_3()
                    .py_1()
                    .rounded_full()
                    .bg(gpui_kit::rgba(0x0000004d))
                    .text_xs()
                    .text_color(gpui_kit::rgb(0xffffff))
                    .cursor_pointer()
                    .hover(|s| s.bg(gpui_kit::rgba(0x00000080)))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.binge = None;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(IconName::ChevronLeft)
                            .small()
                            .text_color(gpui_kit::rgb(0xffffff)),
                    )
                    .child(format!("第{}集", now.vid_index))
            }))
            // 选集浮层开着：舞台铺一层透明捕获层，点浮层外=关闭。捕获层
            // 拦下鼠标下传（舞台的播放/暂停与信息层都不该收到这次点击）
            .children((self.picker_open && self.player.is_some()).then(|| {
                div()
                    .absolute()
                    .size_full()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.picker_open = false;
                            this.picker_group_override = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            }))
            .children(controls)
            // 选集浮层本体（最顶层；自身拦鼠标，点击不落到捕获层）
            .children((self.picker_open && self.player.is_some()).then(|| self.render_picker(cx)))
            // 布局哨兵：子层矩形只在这里可见，两次准备绘制之间比对（见字段注释）。
            // 第一个子层是封面槽（恒在、恒为舞台整块），出画时第二个是视频层、
            // 尺寸必须与它一致——不一致就是布局回归。
            .on_children_prepainted(move |bounds, _window, _cx| {
                let Some(cover) = bounds.first() else { return };
                let size = |b: &gpui_kit::Bounds<gpui_kit::Pixels>| {
                    [b.size.width.to_f64() as f32, b.size.height.to_f64() as f32]
                };
                let (c, v) = (size(cover), bounds.get(1).map(size).unwrap_or([0.0; 2]));
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

    /// 信息叠加层（Tauri immersive-info-overlay 对齐）：热度/剧名/集数/
    /// 简介压在画面左下，控制条弹出时抬到其上沿（bottom_12），闲置淡出
    /// 由调用方按 controls_visible 决定渲染。
    fn render_overlay(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let Some(now) = &self.now else {
            return div().into_any_element();
        };
        // 展示标记跟着**在播的剧**（起播时抄存），不跟游标：换一批/钉住条目
        // 位移时游标可能落在别的条目上，跟游标会把隔壁剧的热度/角标贴错。
        let badge = (!now.badge.is_empty()).then(|| now.badge.clone());
        let season = (!now.season_tag.is_empty()).then(|| now.season_tag.clone());
        let heat = (!now.heat_text.is_empty()).then(|| now.heat_text.clone());
        let intro = self.intros.get(&now.series_id).cloned();
        let intro_expandable = intro.as_ref().is_some_and(|i| i.chars().count() > 40);
        v_flex()
            .absolute()
            .bottom_12()
            .left_3()
            .max_w(gpui_kit::relative(0.62))
            .gap_1()
            .children(heat.map(|h| {
                // 热度行（hgplayer 同款：剧名上方，琥珀色）
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(gpui_kit::rgba(0xfcd34de6))
                    .child(SharedString::from(h))
            }))
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .children(badge.map(|b| {
                        div()
                            .px_1()
                            .rounded(gpui_kit::px(2.0))
                            .bg(gpui_kit::rgba(0xef4444e6))
                            .child(
                                div()
                                    .text_size(gpui_kit::px(10.0))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(gpui_kit::rgb(0xffffff))
                                    .child(SharedString::from(b)),
                            )
                    }))
                    .children(season.map(|s| {
                        div()
                            .px_1()
                            .rounded(gpui_kit::px(2.0))
                            .bg(gpui_kit::rgba(0xffffff33))
                            .child(
                                div()
                                    .text_size(gpui_kit::px(10.0))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(gpui_kit::rgb(0xffffff))
                                    .child(SharedString::from(s)),
                            )
                    }))
                    .children((!now.title.is_empty()).then(|| {
                        // Tauri 的剧名可点跳详情页；GPUI 壳尚无该路由，
                        // 先按纯文本展示（交互随详情页对齐补）
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(gpui_kit::rgb(0xffffff))
                            .truncate()
                            .child(SharedString::from(format!("@{}", now.title)))
                    })),
            )
            .child(
                div()
                    .mt_0p5()
                    .text_xs()
                    .text_color(gpui_kit::rgba(0xffffffd9))
                    .child(format!("第{}集", now.vid_index))
                    .children((now.episode_count > 0).then(|| {
                        div()
                            .text_color(gpui_kit::rgba(0xffffffb3))
                            .child(format!(" · 全{}集", now.episode_count))
                    })),
            )
            .children(intro.map(|intro| {
                // 简介块只占舞台约三分之一（hgplayer 同款量级）；块内点击
                // 不落回舞台（防触发暂停/继续），展开交互只在按钮上
                let expanded = self.intro_expanded;
                h_flex()
                    .id("overlay-intro")
                    .mt_1()
                    .max_w(gpui_kit::relative(0.58))
                    .items_end()
                    .gap_2()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .child(
                        div()
                            .text_xs()
                            .line_height(gpui_kit::relative(1.6))
                            .text_color(gpui_kit::rgba(0xffffffb3))
                            .when(!expanded, |s| s.h(gpui_kit::px(32.0)).overflow_hidden())
                            .child(SharedString::from(intro.clone())),
                    )
                    .children((intro_expandable).then(|| {
                        div()
                            .id("intro-toggle")
                            .flex_shrink_0()
                            .pb(gpui_kit::px(2.0))
                            .text_xs()
                            .cursor_pointer()
                            .text_color(gpui_kit::rgba(0xffffff99))
                            .hover(|s| s.text_color(gpui_kit::rgb(0xffffff)))
                            .when(expanded, |s| {
                                s.on_click(cx.listener(|this, _, _, cx| {
                                    this.intro_expanded = false;
                                    cx.notify();
                                }))
                            })
                            .when(!expanded, |s| {
                                s.on_click(cx.listener(|this, _, _, cx| {
                                    this.intro_expanded = true;
                                    cx.notify();
                                }))
                            })
                            .child(if expanded { "收起" } else { "展开" })
                    }))
            }))
            .into_any_element()
    }

    /// 播放控制条（底部悬浮）：暂停/继续、进度（拖动 seek）、倍速菜单、
    /// 音量/静音。布局与操作面对齐 Tauri 版控制栏；倍速档位、静音独立于
    /// 音量的语义与 `playback-prefs` 一致。
    /// 相对时间（评论时间戳 → 「N 分钟前」等；Tauri relative-time 同口径）。
    fn relative_time(ts_ms: i64) -> String {
        if ts_ms <= 0 {
            return String::new();
        }
        let secs = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - ts_ms / 1000)
            .max(0);
        match secs {
            0..=59 => "刚刚".to_string(),
            60..=3599 => format!("{}分钟前", secs / 60),
            3600..=86_399 => format!("{}小时前", secs / 3600),
            86_400..=2_591_999 => format!("{}天前", secs / 86_400),
            2_592_000..=31_535_999 => format!("{}个月前", secs / 2_592_000),
            _ => format!("{}年前", secs / 31_536_000),
        }
    }

    /// 计数格式化（抖音系「1.4万」样式，Tauri fmtCount 同口径）。
    fn fmt_count(n: i64) -> String {
        if n >= 1_000_000 {
            return format!("{:.1}百万", n as f64 / 1_000_000.0).replace(".0", "");
        }
        if n >= 10_000 {
            let v = n as f64 / 10_000.0;
            let digits = if n % 10_000 >= 1000 { 1 } else { 0 };
            return format!("{:.*}万", digits, v).replace(".0万", "万");
        }
        format!("{n}")
    }

    /// 互动栏当前集/剧的命中（mget 列表里才有「已互动」态）。
    fn interact_hit(&self) -> (bool, bool) {
        let Some(now) = &self.now else {
            return (false, false);
        };
        let bare_vid = now.vid.split(':').next().unwrap_or("");
        let digged = self
            .interact_items
            .iter()
            .find(|i| i.vid == bare_vid)
            .map(|i| i.user_digg)
            .unwrap_or(false);
        let collected = self
            .interact_items
            .iter()
            .find(|i| i.series_id == now.series_id)
            .map(|i| i.followed)
            .unwrap_or(false);
        (digged, collected)
    }

    /// 互动栏右缘竖排（Tauri interaction-rail 对齐：收藏→评论→点赞→分享，
    /// icon 上计数下，激活态红心/黄星，显隐跟随控制条）。
    fn render_interaction_rail(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let (digged, collected) = self.interact_hit();
        // 计数优先档案公开计数（匿名可见）；mget 命中值兜底
        let digg_count = if self.ep_digg_count > 0 {
            self.ep_digg_count
        } else {
            self.interact_items
                .iter()
                .find(|i| i.vid == self.now.as_ref().map(|n| n.vid.clone()).unwrap_or_default())
                .map(|i| i.digged_count)
                .unwrap_or(0)
        };
        let collect_count = if self.follow_count > 0 {
            self.follow_count
        } else {
            self.interact_items
                .iter()
                .find(|i| {
                    Some(i.series_id.as_str()) == self.now.as_ref().map(|n| n.series_id.as_str())
                })
                .map(|i| i.followed_cnt)
                .unwrap_or(0)
        };
        let comment_count = self.ep_comment_count;

        // 单个互动项：icon 上、计数下，激活态换色（抖音系右栏单元）。
        // helper 收 cx.listener 的产物即可，避免闭包里再借 cx。
        // 红色是 hgplayer 皮肤（硬红线 11 的播放器豁免）。
        v_flex()
            .absolute()
            .right_2()
            .bottom_24()
            .items_center()
            .gap_4()
            .child(self.rail_item(
                "collect",
                gpui_kit::assets::IconName::Star,
                collect_count,
                collected,
                gpui_kit::rgba(0xfacc15ff),
                cx.listener(|this, _, _, cx| this.toggle_collect(cx)),
            ))
            .child(self.rail_item(
                "comments",
                gpui_kit::assets::IconName::MessageSquare,
                comment_count,
                false,
                gpui_kit::rgba(0xffffffff),
                cx.listener(|this, _, _, cx| this.toggle_comments_panel(cx)),
            ))
            .child(self.rail_item(
                "digg",
                gpui_kit::assets::IconName::Heart,
                digg_count,
                digged,
                gpui_kit::rgba(0xef4444ff),
                cx.listener(|this, _, _, cx| this.toggle_digg(cx)),
            ))
            .child(self.rail_item(
                "share",
                gpui_kit::assets::IconName::Share,
                0,
                false,
                gpui_kit::rgba(0xffffffff),
                cx.listener(|this, _, _, cx| this.share_series(cx)),
            ))
            .into_any_element()
    }

    /// 回复点赞（乐观 ±1 + 红心翻转，失败回滚；与评论点赞同一 do_action）。
    fn toggle_reply_digg(&mut self, reply_id: String, cx: &mut Context<Self>) {
        if !self.logged_in() {
            log::info!("[Home] 回复点赞需要登录，已忽略");
            return;
        }
        let Some(reply) = self
            .replies
            .values()
            .flat_map(|v| v.iter())
            .find(|r| r.reply_id == reply_id)
            .cloned()
        else {
            return;
        };
        let current = self
            .comment_digg_override
            .get(&reply_id)
            .copied()
            .unwrap_or(reply.user_digg);
        let next = !current;
        let delta = if next { 1 } else { -1 };
        self.comment_digg_override.insert(reply_id.clone(), next);
        for list in self.replies.values_mut() {
            if let Some(items) = Arc::get_mut(list)
                && let Some(hit) = items.iter_mut().find(|r| r.reply_id == reply_id)
            {
                hit.digg_count += delta;
            }
        }
        let state = Arc::clone(&self.state);
        let rid = reply_id.clone();
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            digg_comment(&rid, next, &env).await.map(|_| next)
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| {
                if let Err(e) = result {
                    log::warn!("[Home] 回复点赞失败: {e}");
                    view.comment_digg_override.remove(&reply_id);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// 互动栏单个 item（icon 上、计数下）。
    fn rail_item(
        &self,
        id: &'static str,
        icon: gpui_kit::assets::IconName,
        count: i64,
        active: bool,
        active_color: gpui_kit::Rgba,
        on_click: impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
    ) -> gpui_kit::AnyElement {
        div()
            .id(ElementId::from(format!("rail-{id}")))
            .flex()
            .flex_col()
            .items_center()
            .gap_0p5()
            .cursor_pointer()
            .text_color(if active {
                active_color
            } else {
                gpui_kit::rgb(0xffffff)
            })
            .on_click(on_click)
            .child(Icon::new(icon).size_7().text_color(if active {
                active_color
            } else {
                gpui_kit::rgb(0xffffff)
            }))
            .children((count > 0).then(|| {
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(gpui_kit::rgb(0xffffff))
                    .child(Self::fmt_count(count))
            }))
            .into_any_element()
    }

    /// 评论面板（Tauri comment-panel 对齐：右侧滑出，列表 + 发送框；
    /// 回复内联展开；点赞乐观 ±1；表情代码随表情基建对齐补）。
    fn render_comment_panel(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
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
            .into_any_element()
    }

    /// 评论列表一行：头像占位（昵称首字圆形）/昵称/文本/相对时间/♡。
    fn render_comment_row(&self, c: &CommentItem, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let cid = c.comment_id.clone();
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
                    .child(
                        // 头像占位：昵称首字圆形（远程头像图随 img 远程源对齐补）
                        div()
                            .size_8()
                            .flex_shrink_0()
                            .rounded_full()
                            .items_center()
                            .justify_center()
                            .bg(gpui_kit::rgba(0x404040ff))
                            .text_xs()
                            .text_color(gpui_kit::rgb(0xffffff))
                            .child({
                                let ch = c.user_name.chars().next().unwrap_or('游');
                                SharedString::from(ch.to_string())
                            }),
                    )
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
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .size_6()
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .items_center()
                                    .justify_center()
                                    .bg(gpui_kit::rgba(0x404040ff))
                                    .text_size(gpui_kit::px(10.0))
                                    .text_color(gpui_kit::rgb(0xffffff))
                                    .child({
                                        let ch = r.user_name.chars().next().unwrap_or('游');
                                        SharedString::from(ch.to_string())
                                    }),
                            )
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

    /// 播放控制条（底部悬浮）：暂停/继续、进度（拖动 seek）、倍速菜单、
    /// 弹幕发送框与开关、清晰度、选集、音量。
    fn render_controls(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let paused = self.player.as_ref().is_some_and(|p| p.is_paused());
        let duration = self
            .player
            .as_ref()
            .map(|p| p.duration_secs())
            .unwrap_or(0.0);
        let position = self.scrubbing.unwrap_or_else(|| {
            self.player
                .as_ref()
                .map(|p| p.position_secs())
                .unwrap_or(0.0)
        });
        let play_icon = if paused {
            IconName::Play
        } else {
            IconName::Pause
        };
        // 倍速菜单的点击回调拿不到 view 的 Context，经实体句柄回写
        let view = cx.entity();
        let rate = self.rate;
        let rate_menu = move |menu: gpui_kit::component::menu::PopupMenu,
                              _window: &mut Window,
                              _cx: &mut Context<gpui_kit::component::menu::PopupMenu>|
              -> gpui_kit::component::menu::PopupMenu {
            let mut menu = menu.label("倍速");
            for r in Self::RATES {
                let view = view.clone();
                menu = menu.item(
                    PopupMenuItem::new(Self::fmt_rate(r))
                        .checked(r == rate)
                        .on_click(move |_, _window, cx| {
                            view.update(cx, |v, cx| v.set_rate(r, cx));
                        }),
                );
            }
            menu
        };
        h_flex()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .bg(gpui_kit::rgba(0x000000aa))
            // 控制条悬浮在舞台之上：点击这里的任何控件都**不能**再落到舞台
            //（gpui 对重叠 bounds 的兄弟分支都会派发鼠标事件，不拦的话点
            // 进度条/按钮会同时触发舞台的点击暂停——「点了进度条却暂停了」
            // 的来源；Tauri 里控件层天然挡住指针，这里必须显式拦）。
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _: &gpui_kit::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _: &gpui_kit::MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                Button::new("stage-toggle-play")
                    .icon(play_icon)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(gpui_kit::rgba(0xffffffcc))
                    .child(Self::fmt_secs(position)),
            )
            .child(self.render_progress_track(cx))
            .child(
                div()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(gpui_kit::rgba(0xffffffcc))
                    .child(Self::fmt_secs(duration)),
            )
            .children(self.danmaku_input.as_ref().map(|input| {
                // 弹幕发送框（hgplayer 同款：常驻控制栏左段；Enter 提交，
                // 订阅在 subscribe_controls）
                Input::new(input)
                    .h(gpui_kit::px(28.0))
                    .w(gpui_kit::px(176.0))
                    .text_xs()
                    .rounded_full()
            }))
            .child(
                Button::new("stage-rate")
                    .label(Self::fmt_rate(self.rate))
                    .ghost()
                    .small()
                    .dropdown_menu(rate_menu),
            )
            .child({
                // 清晰度菜单（Tauri 同款）：自动 + 本集全部档位；本地集
                // 没有档位表，按钮置灰并保持可见（不渲染会让用户以为
                // 「这个功能本来就没有」，在线流那集它又出现了）
                let defs = self
                    .now
                    .as_ref()
                    .map(|n| n.definitions.clone())
                    .unwrap_or_default();
                let current = self.now.as_ref().map(|n| n.definition).unwrap_or(0);
                let label = match self.now.as_ref() {
                    Some(n) if n.definition > 0 => format!("{}P", n.definition),
                    _ => "自动".to_string(),
                };
                if defs.is_empty() {
                    // 无档位表（本地集）：置灰保持可见——直接不渲染会让用户
                    // 以为「这个功能本来就没有」，在线流那集它又出现了
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(gpui_kit::rgba(0xffffff66))
                        .child(label)
                        .into_any_element()
                } else {
                    let view = cx.entity();
                    Button::new("stage-definition")
                        .label(label)
                        .ghost()
                        .small()
                        .dropdown_menu(move |menu, _window, _cx| {
                            let mut menu = menu.label("清晰度");
                            menu = menu.item(
                                PopupMenuItem::new("自动").checked(current == 0).on_click({
                                    let view = view.clone();
                                    move |_, _, cx| {
                                        view.update(cx, |v, cx| v.switch_definition(None, cx))
                                    }
                                }),
                            );
                            for d in &defs {
                                let view = view.clone();
                                let value = d.value;
                                // 竖屏剧的宽高是反的，只报分辨率会误导
                                let lbl = if d.height > d.width {
                                    format!("{}P（{}×{}）", d.value, d.width, d.height)
                                } else {
                                    format!("{}P", d.value)
                                };
                                menu = menu.item(
                                    PopupMenuItem::new(lbl)
                                        .checked(d.value == current)
                                        .on_click(move |_, _, cx| {
                                            let view = view.clone();
                                            view.update(cx, |v, cx| {
                                                v.switch_definition(Some(value), cx)
                                            })
                                        }),
                                );
                            }
                            menu
                        })
                        .into_any_element()
                }
            })
            .child(
                // 选集浮层入口（Tauri immersive 同款：带总集数；浮层贴按钮
                // 向上弹，见 render_picker）
                Button::new("stage-picker")
                    .label(match self.now.as_ref() {
                        Some(now) if now.episode_count > 0 => {
                            format!("选集 · 全{}集", now.episode_count)
                        }
                        _ => "选集".to_string(),
                    })
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_picker(cx))),
            )
            .child(
                // 音量（Tauri VolumePopup 同款）：图标按钮单击=静音切换，
                // 悬停向上弹竖向滑杆（弹层是按钮 wrapper 的子层，移过去
                // 不会收）；拖到 0 即静音、拖离 0 自动解除静音
                div()
                    .id("volume-wrapper")
                    .relative()
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if this.volume_popup_open != *hovered {
                            this.volume_popup_open = *hovered;
                            cx.notify();
                        }
                    }))
                    .child(
                        Button::new("stage-mute")
                            .icon(if self.muted || self.volume == 0.0 {
                                gpui_kit::assets::IconName::VolumeX
                            } else if self.volume < 0.5 {
                                gpui_kit::assets::IconName::Volume1
                            } else {
                                gpui_kit::assets::IconName::Volume2
                            })
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                    )
                    .children(self.volume_popup_open.then(|| {
                        v_flex()
                            .absolute()
                            .bottom(gpui_kit::px(36.0))
                            // 右对齐向左展开：音量按钮贴窗口右缘，左锚会把
                            // 弹层推出窗外
                            .right_0()
                            .items_center()
                            .p_2()
                            .rounded(gpui_kit::px(8.0))
                            .bg(gpui_kit::rgba(0x171717f2))
                            .child(
                                Slider::new(&self.volume_slider)
                                    .vertical()
                                    .h(gpui_kit::px(96.0)),
                            )
                            .into_any_element()
                    })),
            )
            .child({
                // 弹幕开关（Tauri 同款：MessageSquare 图标，开=亮/关=暗）
                Button::new("stage-danmaku")
                    .icon(gpui_kit::assets::IconName::MessageSquare)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.danmaku_on = !this.danmaku_on;
                        // 关=清屏+游标落当前位（重开不补放积压）；落盘
                        this.danmaku_flights.clear();
                        this.danmaku_cursor = this
                            .player
                            .as_ref()
                            .map(|p| {
                                this.danmaku_items.partition_point(|d| {
                                    (d.offset_ms as f64) / 1000.0 <= p.position_secs()
                                })
                            })
                            .unwrap_or(0);
                        this.persist_prefs();
                        cx.notify();
                    }))
            })
            .child({
                // 弹幕设置：齿轮 + 上方面板（透明度/字号/密度/显示区域）
                div()
                    .id("dm-settings")
                    .relative()
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if this.dm_panel_open != *hovered {
                            this.dm_panel_open = *hovered;
                            cx.notify();
                        }
                    }))
                    .child(
                        Button::new("stage-dm-settings")
                            .icon(IconName::Settings2)
                            .ghost()
                            .small(),
                    )
                    .children(self.dm_panel_open.then(|| {
                        v_flex()
                            .absolute()
                            .right_0()
                            .bottom(gpui_kit::px(36.0))
                            .w(gpui_kit::px(240.0))
                            .gap_3()
                            .p_4()
                            .rounded(gpui_kit::px(12.0))
                            .bg(gpui_kit::rgba(0x171717f2))
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .child(self.dm_setting_row("不透明度", &self.dm_opacity_slider))
                            .child(self.dm_setting_row("字号", &self.dm_font_slider))
                            .child(self.dm_setting_row("密度", &self.dm_density_slider))
                            .child(self.dm_setting_row("显示区域", &self.dm_area_slider))
                            .into_any_element()
                    }))
                    .into_any_element()
            })
            .child(
                // 下载入口（Tauri download-sheet 同款位置：右段）
                Button::new("stage-downloads")
                    .label("下载")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_downloads(cx))),
            )
            .child(
                // 隐身模式（Tauri incognito 同款用途）：一键暂停 + 整窗近
                // 透明遮蔽，再点恢复显示
                Button::new("stage-incognito")
                    .icon(if self.incognito {
                        gpui_kit::assets::IconName::Eye
                    } else {
                        gpui_kit::assets::IconName::EyeOff
                    })
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_incognito(cx))),
            )
            .child(
                // 全屏切换（Tauri toggleFullscreen 同款；图标随状态换）
                Button::new("stage-fullscreen")
                    .icon(if self.fullscreen {
                        gpui_kit::assets::IconName::Minimize2
                    } else {
                        gpui_kit::assets::IconName::Maximize2
                    })
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_fullscreen(window, cx);
                    })),
            )
            .into_any_element()
    }

    /// 全屏切换（Tauri toggleFullscreen 同款）。
    fn toggle_fullscreen(&mut self, window: &mut gpui_kit::Window, cx: &mut Context<Self>) {
        window.toggle_fullscreen();
        self.fullscreen = window.is_fullscreen();
        cx.notify();
    }

    /// 隐身模式开关：一键暂停 + 整窗近透明（恢复显示但不自动续播）。
    fn toggle_incognito(&mut self, cx: &mut Context<Self>) {
        self.incognito = !self.incognito;
        if self.incognito && !self.player.as_ref().is_some_and(|p| p.is_paused()) {
            self.toggle_pause(cx);
        }
        cx.notify();
    }

    /// 下载面板（右侧滑出）：任务列表（集号/状态/进度）+ 全量入队。
    /// 任务快照每次渲染从队列直读——Frame 事件驱动的重渲染顺带刷新进度。
    fn render_downloads_panel(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
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
                            .children(self.downloads.iter().map(|t| self.render_download_row(t)))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// 下载任务一行：集号 + 状态文本 + 进度条。
    fn render_download_row(&self, t: &DownloadTask) -> gpui_kit::AnyElement {
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
                    .child(div().text_xs().text_color(status_color).child(status_text)),
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

    /// 弹幕设置面板的一行（标签 + 滑杆）。
    fn dm_setting_row(
        &self,
        label: impl Into<SharedString>,
        slider: &Entity<SliderState>,
    ) -> gpui_kit::AnyElement {
        h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(gpui_kit::px(52.0))
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(gpui_kit::rgba(0xffffffcc))
                    .child(label.into()),
            )
            .child(Slider::new(slider))
            .into_any_element()
    }

    /// 选集浮层（Tauri EpisodePicker 对齐）：贴控制栏向上弹的数字网格。
    /// 范围段每段 30 集（hgplayer 同款）；当前集红底高亮；点选**不关**浮层
    /// （连选连跳，关闭走 Esc/收起/点外部）。红色是 hgplayer 皮肤（硬红线
    /// 11 的播放器豁免）。「系列剧」tab 与下载完成角标随相关作品/预约、
    /// 下载能力的对齐步骤补。
    fn render_picker(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let Some(now) = &self.now else {
            return div().into_any_element();
        };
        let total = now.episode_count as u32;
        const GROUP_SIZE: u32 = 30;
        let group_count = total.div_ceil(GROUP_SIZE).max(1) as usize;
        let follow_group =
            (now.vid_index.saturating_sub(1) as usize / GROUP_SIZE as usize).min(group_count - 1);
        let active_group = match self.picker_group_override {
            Some((group, at)) if at == now.vid_index => group,
            _ => follow_group,
        };
        let start = active_group as u32 * GROUP_SIZE + 1;
        let end = (start + GROUP_SIZE - 1).min(total);

        // 头部：选集 tab（红条高亮）+ 全 N 集 + 收起
        let header = h_flex()
            .items_center()
            .justify_between()
            .mb_3()
            .child(
                div()
                    .relative()
                    .pb_1()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(gpui_kit::rgb(0xffffff))
                    .child("选集")
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(gpui_kit::px(2.0))
                            .rounded_full()
                            .bg(gpui_kit::rgb(0xef4444)),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(gpui_kit::rgba(0xa3a3a3ff))
                            .child(format!("全{total}集")),
                    )
                    .child(
                        Button::new("picker-close")
                            .icon(IconName::ChevronDown)
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picker_open = false;
                                this.picker_group_override = None;
                                cx.notify();
                            })),
                    ),
            );

        // 范围段 tab（1-30 / 31-60 / …；只有一段时不渲染）
        let group_tabs = div()
            .flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .mb_3()
            .children((0..group_count).map(|i| {
                let active = i == active_group;
                let lo = i as u32 * GROUP_SIZE + 1;
                let hi = ((i as u32 + 1) * GROUP_SIZE).min(total);
                div()
                    .id(gpui_kit::ElementId::from(format!("picker-group-{i}")))
                    .cursor_pointer()
                    .text_size(gpui_kit::px(13.0))
                    .text_color(if active {
                        gpui_kit::rgb(0xffffff)
                    } else {
                        gpui_kit::rgba(0xa3a3a3ff)
                    })
                    .hover(|s| s.text_color(gpui_kit::rgba(0xe5e5e5ff)))
                    .when(active, |s| s.font_weight(gpui_kit::FontWeight::SEMIBOLD))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let at = this.now.as_ref().map(|n| n.vid_index).unwrap_or(0);
                        this.picker_group_override = Some((i, at));
                        cx.notify();
                    }))
                    .child(format!("{lo}-{hi}"))
            }));

        // 8 列数字网格（右上角下载完成角标：绿点 = 该集已下载可离线播）
        let grid = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children((start..=end).map(|n| {
                let active = n == now.vid_index;
                let downloaded = self.done_vids.contains(&n);
                div()
                    .id(gpui_kit::ElementId::from(format!("picker-ep-{n}")))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(gpui_kit::px(48.0))
                    .h(gpui_kit::px(36.0))
                    .rounded(gpui_kit::px(6.0))
                    .text_sm()
                    .cursor_pointer()
                    .text_color(if active {
                        gpui_kit::rgb(0xffffff)
                    } else {
                        gpui_kit::rgba(0xe5e5e5ff)
                    })
                    .bg(if active {
                        gpui_kit::rgba(0xef4444ff)
                    } else {
                        gpui_kit::rgba(0x262626cc)
                    })
                    .hover(|s| s.bg(gpui_kit::rgba(0x404040ff)))
                    .when(active, |s| s.font_weight(gpui_kit::FontWeight::SEMIBOLD))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_episode(n, cx)))
                    .child(format!("{n}"))
                    .children(downloaded.then(|| {
                        div()
                            .absolute()
                            .top_1()
                            .right_1()
                            .size_1p5()
                            .rounded_full()
                            .bg(if active {
                                gpui_kit::rgba(0xfffffffa)
                            } else {
                                gpui_kit::rgba(0x34d399ff)
                            })
                    }))
            }));

        let hint = div()
            .mt_3()
            .pt_2p5()
            .border_t_1()
            .border_color(gpui_kit::rgba(0x262626ff))
            .text_xs()
            .text_color(gpui_kit::rgba(0x737373ff))
            .child("选定集数即选中本剧：单击画面也可选中，滚轮/↑↓ 切集，Esc 退出");

        v_flex()
            .id("picker-panel")
            .absolute()
            .right_4()
            .bottom_12()
            .w(gpui_kit::px(472.0))
            .max_h(gpui_kit::relative(0.62))
            .p_4()
            .rounded(gpui_kit::px(12.0))
            .bg(gpui_kit::rgba(0x171717f2))
            .overflow_y_scroll()
            // 浮层内的任何点击都不许落到捕获层/舞台（点选集把视频点停了
            // 就是这里漏的——Tauri EpisodePicker 根节点 onClick 同款）
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(header)
            .children((group_count > 1).then_some(group_tabs))
            .child(grid)
            .child(hint)
            .into_any_element()
    }

    /// 进度轨道（自绘）：细条填充按播放位置实时走（帧事件驱动重渲染）；
    /// 点击即跳转，按住拖动为预览（时间读数跟手），松手才 seek——对齐
    /// Tauri 版「拖动不抢播、松手定位」。轨道矩形经 prepainted 哨兵存取，
    /// 拖动事件只带窗口坐标。
    fn render_progress_track(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let duration = self
            .player
            .as_ref()
            .map(|p| p.duration_secs())
            .unwrap_or(0.0);
        let position = self.scrubbing.unwrap_or_else(|| {
            self.player
                .as_ref()
                .map(|p| p.position_secs())
                .unwrap_or(0.0)
        });
        let frac = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bounds = Rc::clone(&self.progress_bounds);
        h_flex()
            .flex_1()
            .items_center()
            // 命中区加高（好点），视觉条只有 4px
            .h_4()
            .relative()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h_1()
                    .rounded_full()
                    .bg(gpui_kit::rgba(0xffffff33))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_1()
                            .w(gpui_kit::relative(frac as f32))
                            .rounded_full()
                            .bg(gpui_kit::rgb(0xffffff)),
                    ),
            )
            // prepainted 哨兵是 Div 的方法：必须在 .id() 变 Stateful 之前挂
            .on_children_prepainted(move |children, _window, _cx| {
                // 第一个子层是视觉条；矩形存哨兵供指针位置换算
                if let Some(b) = children.first() {
                    bounds.set(Some(*b));
                }
            })
            .id("stage-progress-track")
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, ev: &gpui_kit::MouseDownEvent, _window, cx| {
                    // 对齐 Tauri ScrubBar 的 pointerdown：按下即定位（不是只
                    // 起预览），随后拖动持续跟手
                    if let Some(target) = this.scrub_target_at(ev.position.x) {
                        this.scrubbing = Some(target);
                        this.scrub_seek_now(target, cx);
                    }
                }),
            )
            .on_drag(ScrubDrag, |_, _, _, cx| cx.new(|_| ScrubGhost))
            .on_drag_move::<ScrubDrag>(cx.listener(
                |this, ev: &gpui_kit::DragMoveEvent<ScrubDrag>, _window, cx| {
                    if let Some(target) = this.scrub_target_at(ev.event.position.x) {
                        this.scrubbing = Some(target);
                        // 实时 scrub：拖到哪放到哪（引擎侧节流限频）
                        this.scrub_seek_throttled(target, cx);
                    }
                },
            ))
            // 松手收口：轨道内/外都要算（拖出轨道再松手也是一次明确的定位）
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _window, cx| {
                    this.commit_scrub(cx);
                }),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _window, cx| {
                    this.commit_scrub(cx);
                }),
            )
            .into_any_element()
    }
}

impl gpui_kit::Render for HomeView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        // 取证：render 调用率（64 次一跳）。绘制饿死时这行不涨而事件仍在流。
        static RENDERS: AtomicU32 = AtomicU32::new(0);
        let n = RENDERS.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(64) {
            log::info!("[Home] render #{n}");
        }
        // 纹理回收口：gpui 的 sprite atlas 按 RenderImage::id 常驻（paint 只增
        // 不删），视频每帧一个新 id——不在这里显式 drop，显存/内存就无上限
        // 涨（实测 5 分钟 5.5GB）。只在 render 时有 Window 句柄可调。
        #[cfg(target_os = "windows")]
        for old in std::mem::take(&mut self.retired_images) {
            if let Err(e) = window.drop_image(old) {
                log::warn!("[Home] 纹理回收失败: {e}");
            }
        }
        // render_tabs 要 &mut cx，先取（theme 借的是 cx 的不可变引用）
        let tabs = self.render_tabs(cx);
        // 播放快捷键的焦点种子：键盘事件只派发给焦点子树，舞台的
        // FocusHandle 得在首次渲染时拿到焦点（只播种一次——之后焦点可能
        // 在弹层菜单里，每帧强抢会打架；点舞台会带回焦点）。
        // 放在 theme 借用之前：focus 要可变借 cx。
        if !self.focus_seeded && !self.focus.is_focused(window) {
            self.focus_seeded = true;
            window.focus(&self.focus, cx);
        }
        // 弹幕发送成功后的输入框清空（set_value 要 window，这里补执行）
        if self.danmaku_clear_input
            && let Some(input) = &self.danmaku_input
        {
            self.danmaku_clear_input = false;
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        // 下载任务快照刷新（render_stage 是 &self，写不进去；借这里的 &mut）
        if self.downloads_open {
            let sid = self
                .now
                .as_ref()
                .map(|n| n.series_id.clone())
                .unwrap_or_default();
            self.downloads = Arc::new(crate::downloads::tasks_of(&self.state, &sid));
            self.done_vids = Arc::new(crate::downloads::completed_vids(&self.state, &sid));
        }
        // 全屏态图标跟随（Render::render 有 window 可查）
        self.fullscreen = window.is_fullscreen();
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
            // 隐身模式：整窗近透明遮蔽（Tauri incognito 的窗口透明在 gpui
            // 壳层用根 opacity 近似；再点隐身按钮恢复）
            .when(self.incognito, |s| s.opacity(0.06))
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
                    .on_mouse_move(cx.listener(|this, _: &gpui_kit::MouseMoveEvent, _, cx| {
                        this.wake_controls(cx);
                    }))
                    .on_scroll_wheel(cx.listener(
                        |this, ev: &gpui_kit::ScrollWheelEvent, window, cx| {
                            // Tauri 同参：|Δy|<15 忽略，400ms 冷却在 step 里；
                            // 方向同 Tauri（deltaY>0=下滚=下一部，y<0 即下一部）
                            let y = ev.delta.pixel_delta(window.line_height()).y.to_f64();
                            if y.abs() < 15.0 {
                                return;
                            }
                            this.step(if y < 0.0 { 1 } else { -1 }, cx);
                        },
                    ))
                    .into_any_element(),
            })
    }
}

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
//! Tauri 同一段 service（运行时共用，见 `hongguo_service::runtime`）。
//!
//! 控制区（对齐 Tauri 版控制栏的核心件）：暂停/继续（按钮 + 点击画面）、
//! 进度条拖动 seek、倍速菜单（0.75–3x，音频变速不变调）、音量/静音；偏好
//! 经 [`crate::playback_prefs`] 跨集跨启动保留。键盘快捷键（空格/方向键）
//! 与 ↑↓ 切剧随控制层下一批目标落地；「回首页 5 分钟换一批」的过期重开
//! 未做（换一批入口本身已对齐）。
//!
//! 本文件只留视图本体（HomeView/NowPlaying 状态、构造、订阅、引擎事件
//! 循环与 Render 入口）；按域拆出的子模块：`feed`（取流/展示列表）、
//! `playback`（播放编排）、`danmaku`（弹幕）、`comments`（评论/回复）、
//! `interact`（点赞/收藏/互动栏）、`auth`（登录/头像）、`controls`（控制条
//! 与进度轨道）、`mini_panel`（小屏）、`picker`(选集浮层)、`downloads`
//! （下载面板）、`stage`（舞台/叠加层渲染）、`fmt`（格式化）。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Entity, RenderImage, Window, div};
use hongguo_api::danmaku::{CommentItem, Danmaku, ReplyItem};
use hongguo_api::detail::RelatedItem;
use hongguo_api::history::WatchHistoryItem;
use hongguo_api::interact::InteractionItem;
use hongguo_common::model::{DownloadTask, VideoDefinition};
use hongguo_media_platform_api::video::VideoFrame;
use hongguo_playback::{PlayerEvent, PlayerHandle};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::events::ServiceEvent;
use tokio::sync::mpsc;

use danmaku::DmFlight;
use feed::{Source, SourceFeed, StreamItem};

pub(crate) mod auth;
pub(crate) mod comment_view;
pub(crate) mod comments;
pub(crate) mod controls;
pub(crate) mod danmaku;
pub(crate) mod downloads;
pub(crate) mod feed;
pub(crate) mod fmt;
pub(crate) mod interact;
pub(crate) mod mini_panel;
pub(crate) mod overlay;
pub(crate) mod picker;
pub(crate) mod playback;
#[cfg(feature = "screenshot")]
pub(crate) mod preview_seed;
pub(crate) mod stage;

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
    /// 已解码评论/回复头像（avatar URL → RGBA）；加载失败不入表（退回首字
    /// 圆形占位）。
    avatars: HashMap<String, Arc<RenderImage>>,
    avatars_pending: HashSet<String>,
    covers_pending: HashSet<String>,

    // —— 播放态 ——
    /// 引擎出生纪元：每次切剧 +1，陈旧事件/任务按它丢弃。
    epoch: u64,
    player: Option<std::sync::Arc<PlayerHandle>>,
    /// 预览的虚拟引擎标记（screenshot feature）：预览无真实引擎，而控制条/
    /// 选集/下载面板的渲染门槛都是 player.is_some()——种子置位后同一门槛
    /// 放行，这些 UI 走同一段渲染代码出画（见 [`Self::engine_ready`]）。
    #[cfg(feature = "screenshot")]
    preview_engine: bool,
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
    // —— 选集浮层「系列剧」tab ——
    /// 各季卡片（plan/v 相关作品·系列里季号可解析的条目；每剧一次）。
    related_seasons: HashMap<String, Arc<Vec<RelatedItem>>>,
    related_loading: HashSet<String>,
    /// 选集浮层当前 tab：true = 系列剧。
    picker_tab_series: bool,
    /// 已预约季（服务端离线预约列表 + 本地预约成功即插；未上线季按钮态）。
    reserved_ids: HashSet<String>,
    /// 预约请求在途（防重复提交）。
    reserve_pending: HashSet<String>,
    /// 离线预约列表是否已拉（会话级一次；预约按钮的已约态数据源）。
    reservations_loaded: bool,
    /// 回复目标：Some((comment_id, reply_to_reply_id, 被回复人昵称))。
    reply_to: Option<(String, Option<String>, String)>,
    /// 评论输入框（与弹幕发送框同款 InputState）。
    comment_input: Option<Entity<InputState>>,
    comment_sending: bool,
    comment_clear_input: bool,
    /// 评论面板的表情选择器开着（笑脸按钮 toggle；连选不关面板）。
    emoji_picker_open: bool,
    // —— 壳内登录（SMS；Tauri 登录页对齐的最小面）——
    login_panel_open: bool,
    login_mobile: Option<Entity<InputState>>,
    login_code: Option<Entity<InputState>>,
    login_busy: bool,
    login_message: Option<String>,
    /// MFA 上行短信重试上下文（retry_tag, sms_code_key）。
    mfa_retry: Option<(String, String)>,
    /// 待确认删除目标：Some((id, 父评论 id——回复场景才有))，Tauri
    /// deleteTarget 同构；id 是 comment_id 或 reply_id（同走 comment/del
    /// service_id=4）。
    comment_delete_target: Option<(String, Option<String>)>,
    /// 删除请求在途（防重复提交）。
    comment_deleting: bool,
    // —— 下载 ——
    /// 下载面板开着。
    downloads_open: bool,
    /// 隐身模式（Tauri incognito 简化对齐）：一键暂停 + 整窗近透明遮蔽，
    /// 再点恢复显示（播放由用户自行续）。
    incognito: bool,
    /// 全屏中（Render 时从 window.is_fullscreen 读，切图标用）。
    fullscreen: bool,
    /// 小屏播放态（Tauri miniScreen 同款）：Some 是进入前的几何快照。
    /// 同窗缩放语义——不换窗口、不暂停，只缩窗口 + 换紧凑控制条。
    mini: Option<crate::mini::SavedGeometry>,
    /// 窗口钉住（Tauri usePinWindow 同款：窗口级会话态，大小屏共用）。
    pinned: bool,
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
            avatars: HashMap::new(),
            avatars_pending: HashSet::new(),
            epoch: 0,
            player: None,
            #[cfg(feature = "screenshot")]
            preview_engine: false,
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
            related_seasons: HashMap::new(),
            related_loading: HashSet::new(),
            picker_tab_series: false,
            reserved_ids: HashSet::new(),
            reserve_pending: HashSet::new(),
            reservations_loaded: false,
            reply_to: None,
            comment_input: Some(cx.new(|cx| InputState::new(window, cx).placeholder("说点什么…"))),
            comment_sending: false,
            comment_clear_input: false,
            emoji_picker_open: false,
            login_panel_open: false,
            login_mobile: Some(
                cx.new(|cx| InputState::new(window, cx).placeholder("手机号（11 位）")),
            ),
            login_code: Some(cx.new(|cx| InputState::new(window, cx).placeholder("验证码"))),
            login_busy: false,
            login_message: None,
            mfa_retry: None,
            comment_delete_target: None,
            comment_deleting: false,
            downloads_open: false,
            incognito: false,
            fullscreen: false,
            mini: None,
            pinned: false,
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
        view.subscribe_service_events(cx);
        // 下载队列恢复：running 视为中断改回 pending（Tauri bootstrap 同款），
        // 然后触发一轮调度把中断的任务续跑起来。库读取失败只记错误不拦
        // 启动——壳的其余功能不依赖队列（显式容错：有日志、只此一处）。
        {
            match view.state.store.tasks() {
                Ok(tasks) => {
                    let limit = view.state.settings().max_concurrency as usize;
                    view.state.replace_queue(
                        hongguo_service::download_service::queue::DownloadQueue::restore(
                            tasks, limit,
                        ),
                    );
                    hongguo_service::download_service::kick(view.state.clone());
                }
                Err(e) => log::error!("[Home] 下载队列恢复失败，本轮从空队列开始：{e}"),
            }
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
        // 预览进程只吃 fixtures 假快照，不发任何启动请求
        if !crate::preview::is_active() {
            view.load_feed(Source::Feed, false, cx);
            view.load_history(cx);
        }
        view
    }

    /// 订阅 service 业务事件（登录 MFA、下载状态）并落到视图状态。
    ///
    /// 事件泵把 tokio broadcast 转成 GPUI 侧可 await 的无界通道
    /// （见 [`crate::events`]）；这里起一条消费循环——与 Tauri 壳消费
    /// 同一批事件（一份业务实现，两种 UI 适配）。
    fn subscribe_service_events(&mut self, cx: &mut Context<Self>) {
        let mut rx = crate::events::subscribe(&self.state);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.recv().await {
                let ok = this
                    .update(cx, |view, cx| view.on_service_event(event, cx))
                    .is_ok();
                if !ok {
                    break; // 视图已销毁
                }
            }
        })
        .detach();
    }

    /// service 事件 → 视图状态。
    fn on_service_event(&mut self, event: ServiceEvent, cx: &mut Context<Self>) {
        match event {
            ServiceEvent::LoginMfa(ev) => self.on_login_mfa_event(ev, cx),
            // 下载面板开着时随事件即时刷新（面板本身另有 1s 轮询兜底）
            ServiceEvent::DownloadTaskAdded(_)
            | ServiceEvent::DownloadProgress(_)
            | ServiceEvent::DownloadCompleted(_)
            | ServiceEvent::DownloadFailed(_)
            | ServiceEvent::DownloadStopped(_)
            | ServiceEvent::DownloadQueueChanged(_) => {
                if self.downloads_open {
                    self.refresh_downloads(cx);
                }
            }
            ServiceEvent::OnlineProgress(_) => {}
            ServiceEvent::MergeTaskAdded(_)
            | ServiceEvent::MergeProgress(_)
            | ServiceEvent::MergeCompleted(_)
            | ServiceEvent::MergeFailed(_) => {}
            ServiceEvent::CompatProgress(_) => {}
        }
    }

    /// 引擎是否就位：控制条、选集/下载面板渲染的共同门槛。真实壳=引擎
    /// 句柄在；预览进程由种子置位虚拟引擎（screenshot feature）。
    fn engine_ready(&self) -> bool {
        self.player.is_some() || self.preview_engine()
    }

    #[cfg(feature = "screenshot")]
    fn preview_engine(&self) -> bool {
        self.preview_engine
    }

    #[cfg(not(feature = "screenshot"))]
    fn preview_engine(&self) -> bool {
        false
    }

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

    /// 引擎事件循环（每个引擎一个，epoch 封口：换剧后陈旧事件自然断流）。
    pub(super) fn spawn_event_loop(
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
        // render_tabs 要 &mut cx，先取（theme 借的是 cx 的不可变引用）；
        // 小屏不渲染顶栏（480×270 里只留播放器，Tauri 小屏同款）
        let in_mini = self.mini.is_some();
        let tabs = (!in_mini).then(|| self.render_tabs(cx));
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
            self.downloads = Arc::new(self.state.queue().of_series(&sid));
            self.done_vids = Arc::new(self.state.queue().completed_vids(&sid));
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
            .children(tabs)
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
                    .render_stage(window, cx)
                    .id("home-stage")
                    .on_mouse_move(cx.listener(|this, _: &gpui_kit::MouseMoveEvent, _, cx| {
                        this.wake_controls(cx);
                    }))
                    .on_scroll_wheel(cx.listener(
                        |this, ev: &gpui_kit::ScrollWheelEvent, window, cx| {
                            // 小屏里滚轮不切剧（纯播放器语义，同 ↑↓ 守卫）
                            if this.mini.is_some() {
                                return;
                            }
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

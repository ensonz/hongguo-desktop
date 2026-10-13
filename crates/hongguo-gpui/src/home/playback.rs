//! 播放编排：起播链（档案 → 起点决策 → prepare → 引擎 open）、滚轮/连播
//! 切换、切清晰度、自动连播、进度落盘与流/档案预取。

#[cfg(target_os = "windows")]
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use hongguo_api::history::WatchHistoryItem;
use hongguo_common::AppResult;
use hongguo_playback::{OpenOptions, PlayerEvent, PlayerHandle};
use hongguo_service::app_state::AppStateInner;
use hongguo_service::play_service::online::{prefetch_stream, prepare};
use hongguo_service::play_service::position;
use tokio::sync::mpsc;

use super::NowPlaying;
use super::feed::StreamItem;
use crate::bridge;

/// 播放进度落盘最小间隔（秒）（Tauri 侧同为 5 秒一写）。
const PROGRESS_SAVE_EVERY_SECS: u64 = 5;
/// 滚轮切剧去抖（毫秒）：滚轮一格连发多事件的吸收窗口。
const WHEEL_SWITCH_DEBOUNCE_MS: u64 = 400;

impl super::HomeView {
    /// 连播锁定内的切集（滚轮/↑↓，Tauri useBingeRelay 的 stepEpisode 同款）：
    /// 越过第 1 集原地不动；滚过末集 = 剧终，跟随宿主刷下一部（本实现无
    /// 下一季接力，与信息流回落路径一致）。
    pub(super) fn step_episode(&mut self, dir: i32, cx: &mut Context<Self>) {
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
    pub(super) fn step(&mut self, dir: i32, cx: &mut Context<Self>) {
        if self.in_binge() {
            // Tauri 同款：滚轮也是「用户在场」，切集时唤醒悬浮层
            self.wake_controls(cx);
            self.step_episode(dir, cx);
            return;
        }
        let now = Instant::now();
        if self.wheel_at.is_some_and(|t| {
            now.duration_since(t) < Duration::from_millis(WHEEL_SWITCH_DEBOUNCE_MS)
        }) {
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

    /// 切到目标剧。旧的引擎继续播到新引擎就位（对齐 Tauri「store 目标
    /// 变了但画面还在上一部」的切换语义），就位即整体换装。
    /// `start_override` 给切清晰度保进度用（None = 走起点决策）。
    pub(super) fn play_target(
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
    ) -> AppResult<(std::sync::Arc<PlayerHandle>, NowPlaying)> {
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
            (None, None) => hongguo_service::play_service::resume::resolve_entry(
                &state.store,
                &item.series_id,
                &history,
            )?,
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
        let prepared = prepare(&state, &ep.vid, &progress_key, preferred_definition).await?;
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

    /// 拉当前剧的简介与全剧收藏数（video_detail 的 series_intro /
    /// followed_cnt，Tauri useSeriesDetailMeta 同源）：装饰性信息，失败
    /// 静默降级、允许重试；每剧只拉一次（Tauri staleTime 10 分钟同语义
    /// 的会话内缓存）。followed_cnt 是互动栏「收藏」键的计数源。
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
                .map(|meta| (sid.clone(), meta.intro, meta.followed_cnt))
                .map_err(|e| (sid, e))
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, _| match result {
                Ok((sid, intro, followed_cnt)) => {
                    view.intros_pending.remove(&sid);
                    if !intro.is_empty() {
                        view.intros.insert(sid.clone(), intro);
                    }
                    view.follow_count = followed_cnt;
                }
                Err((sid, e)) => {
                    // 失败不算数：允许下次起播重试（rank.rs 同款记一条 warn）
                    view.intros_pending.remove(&sid);
                    log::warn!("[Home] {sid} 简介拉取失败，下次起播重试：{e}");
                }
            });
        })
        .detach();
    }

    /// 切清晰度（Tauri onDefinitionChange 同款）：同集同位重播，缓存按
    /// (vid, 档位) 分条目——切回去不用重新下载，也不动正在播的那一档。
    /// `None` = 回到自动（沿用上轮解析到的档位）。
    pub(super) fn switch_definition(&mut self, value: Option<u32>, cx: &mut Context<Self>) {
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
    pub(super) fn pick_episode(&mut self, vid_index: u32, cx: &mut Context<Self>) {
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

    /// 连播：下一集；末集接流内下一部（剧终接力的跨季/换批语义后续对齐）。
    pub(super) fn advance_after_end(&mut self, cx: &mut Context<Self>) {
        self.frame = None;
        #[cfg(target_os = "windows")]
        self.clear_frame_image();
        // 防重入：Ended 只在时钟走完才发，但引擎收尾、通道重连都可能让同一集
        // 的 Ended 再来一次——那时 vid_index 还停在旧值，会把刚起播的下一集
        // 又推进一格（连着跳集，看着就是「内容来回变」）。切换途中直接吞掉。
        if self.switching {
            return;
        }
        // 自动连播静默起步（Tauri quietStart 同款）：新集首帧或用户真实输入
        // 之前，悬浮层一律不亮（on_player_event 首帧/wake_controls 解除）
        self.silent_until_input = true;
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
    pub(super) fn prefetch_next_stream(&mut self, cx: &mut Context<Self>) {
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
        let (vid_index, _) = hongguo_service::play_service::resume::resolve_entry(
            &state.store,
            &series_id,
            &history,
        )?;
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
        prefetch_stream(&state, &ep.vid, &progress_key).await
    }

    /// 相邻剧分集档案预取（换剧时 resolve 秒回）。
    pub(super) fn prefetch_neighbor_archives(&mut self) {
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

    /// 5 秒节流落盘（播放档案是真值：续播、详情页「继续看」都吃它）。
    pub(super) fn save_progress_maybe(&mut self) {
        if self.last_save.elapsed() < Duration::from_secs(PROGRESS_SAVE_EVERY_SECS) {
            return;
        }
        self.last_save = Instant::now();
        self.save_progress_now();
    }

    /// 立即落一次进度（不带节流）：关小屏等「要走了」的时刻强制落，
    /// 别让最后一段播放丢在节流窗里（Tauri onPause 强落同款）。
    pub(super) fn save_progress_now(&mut self) {
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
}

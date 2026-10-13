//! 弹幕：拉取、按 Frame tick 的入场/推进/回收、发送与设置面板滑杆。

use std::sync::Arc;

use gpui_kit::component::h_flex;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::prelude::*;
use gpui_kit::{Entity, SharedString, div};
use hongguo_api::danmaku::{Danmaku, fetch_danmaku_all};
use hongguo_api::interact::send_danmaku;

use crate::bridge;

/// 在飞的弹幕：泳道 + 入场墙钟 + 估宽。位置每渲染帧按「播放中才走的
/// 墙钟」推进——暂停即冻结（无 Frame 事件），倍速不改变滚动速度
///（hgplayer 同款：它家倍速下弹幕也不变速）。progress 以「屏宽」为单位的
/// 滚动进度：x = stage_w − progress × (stage_w + 估宽)，progress ≥ 1 出屏。
pub(super) struct DmFlight {
    pub(super) text: String,
    pub(super) lane: usize,
    width: f32,
    progress: f64,
}

impl DmFlight {
    /// 相对舞台左缘的横向偏移（像素，可为负 = 已滚出左沿）。
    pub(super) fn x_offset(&self, stage_w: f64) -> f64 {
        stage_w - self.progress * (stage_w + self.width as f64)
    }
}

/// 弹幕估宽：按表情分段估（表情 1.3 字号，文本 CJK/非 CJK）。
fn dm_text_width(text: &str, font_px: f32) -> f32 {
    crate::danmaku_emoji::segment_widths(text, font_px)
}

/// 滚过一屏的时长（hgplayer durationMs 默认 9000）。
const DM_CROSS_MS: f64 = 9000.0;
/// 同一泳道相邻两条的入场间隔（1.4 墙钟秒；记账按倍速折算成媒体秒，见
/// spawn 占位处）。
const DM_LANE_BUSY_S: f64 = 1.4;
/// 泳道高 = 字号 + 10（hgplayer top 计算同款）。
pub(super) const DM_LANE_GAP: f32 = 10.0;
/// 泳道数下限。
const DM_MIN_LANES: usize = 4;
/// 基准字号范围（hgplayer 默认上限 22px）。
const DM_FONT_MIN: f32 = 15.0;
const DM_FONT_MAX: f32 = 22.0;

impl super::HomeView {
    /// 拉当前集的全部弹幕（group_id = vid，book_id = series_id；社交态
    /// 不缓存，每次进集都重拉——Tauri useDanmaku 同款）。拉完游标归零，
    /// 由 tick 按「当前播放位置之前的不补放」自然对齐。
    pub(super) fn load_danmaku(&mut self, vid: String, cx: &mut Context<Self>) {
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
                        // 游标直接落到当前播放位置：位置之前的历史不补放。
                        // 归零的话 tick 会把开头到当前位置的弹幕**全部一次性
                        // 入场**——续播（起点 >3s）/中途进集必现「弹幕糊一脸」，
                        // 这正是本函数头注释承诺的「自然对齐」该有的样子
                        let pos = view
                            .player
                            .as_ref()
                            .map_or(0.0, |p| p.position_secs());
                        view.danmaku_items = Arc::new(items);
                        view.danmaku_cursor = view
                            .danmaku_items
                            .partition_point(|d| (d.offset_ms as f64) / 1000.0 <= pos);
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
    pub(super) fn danmaku_tick(&mut self, cx: &mut Context<Self>) {
        if !self.danmaku_on || self.danmaku_items.is_empty() {
            return;
        }
        let Some(player) = &self.player else {
            return;
        };
        let pos = player.position_secs();
        let paused = player.is_paused();
        // 倍速在 tick 开头取一次：泳道占用折算用（值在整个 tick 内不变，
        // 借 `player` 的时间越短越好）
        let rate = player.rate();
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
            // 泳道：挑最早空出来的那条并占位（叠满时自然排队）。占用期按
            // 「滚动 DM_LANE_BUSY_S 墙钟秒」折算成媒体秒（× 倍速）——在飞
            // 弹幕按墙钟推进，占用期却用媒体秒记账的话，倍速下泳道提前复用，
            // 新弹幕压在还没滚出右沿的旧弹幕上（叠幕必现）
            let mut pick = 0usize;
            for (i, free) in self.danmaku_lane_free.iter().enumerate() {
                if *free < self.danmaku_lane_free[pick] {
                    pick = i;
                }
            }
            if let Some(free) = self.danmaku_lane_free.get_mut(pick) {
                *free = pos + DM_LANE_BUSY_S * rate;
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
    pub(super) fn dm_font_px(&self, stage_h: f32) -> f32 {
        let base = (stage_h / 30.0).clamp(DM_FONT_MIN, DM_FONT_MAX);
        (base * self.dm_font_scale).max(10.0).round()
    }

    /// 泳道数：显示区域高度 ÷（字号 + 10），下限 4。
    fn dm_lane_count(&self, stage_h: f32) -> usize {
        let font_px = self.dm_font_px(stage_h);
        let lanes = ((stage_h * self.dm_area) / (font_px + DM_LANE_GAP)).floor() as usize;
        lanes.max(DM_MIN_LANES)
    }

    /// 弹幕设置滑杆：更新对应显示参数；Release 落盘。
    pub(super) fn on_dm_slider_event(
        &mut self,
        which: u8,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
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
    pub(super) fn submit_danmaku(&mut self, cx: &mut Context<Self>) {
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

    /// 弹幕设置面板的一行（标签 + 滑杆）。
    pub(super) fn dm_setting_row(
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
}

//! 控制区（对齐 Tauri 版控制栏核心件）：暂停/倍速/音量/清晰度入口、
//! 进度轨道（拖动 seek 与节流）、控制条显隐、偏好落盘、引擎事件分发，
//! 以及大屏控制条渲染。

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::IconName;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderValue};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::{Window, div};
use hongguo_playback::PlayerEvent;

use crate::bridge;

/// 控制条强制常显时长（秒）：交互后多 linger 一段时间再交回闲置淡出。
/// 只被 screenshot 门控的取证辅助使用，随消费点一起进 feature。
#[cfg(feature = "screenshot")]
pub(super) const CONTROLS_HOLD_SECS: u64 = 600;

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

impl super::HomeView {
    /// 取证辅助（screenshot feature 的 HONGGUO_SHOT_PAUSE）：暂停播放并把
    /// 控制条可见窗拉长到 10 分钟——无人值守抓帧才能拍到控制条/互动栏。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_pause_player(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.player
            && !player.is_paused()
        {
            player.pause();
        }
        self.controls_until = Some(Instant::now() + Duration::from_secs(CONTROLS_HOLD_SECS));
        cx.notify();
    }

    pub(super) fn on_player_event(&mut self, event: PlayerEvent, cx: &mut Context<Self>) {
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

    /// 倍速档位（与 Tauri 版 `player-controls.tsx` 的 RATES 完全一致）。
    const RATES: [f64; 6] = [0.75, 1.0, 1.25, 1.5, 2.0, 3.0];
    /// 控制条可见时长：鼠标静止/无操作后自动隐藏。
    const CONTROLS_VISIBLE: Duration = Duration::from_secs(3);
    /// 拖动中实时 seek 的最小间隔：引擎一次 seek = 冲流 + 解码器重建，
    /// 250ms 限频跟手又不拖垮供给（Tauri 侧浏览器自行合并，这里显式限）。
    const SCRUB_SEEK_MIN_INTERVAL: Duration = Duration::from_millis(250);

    /// 音量滑杆：拖动即生效（引擎在设备回调里乘 PCM，零延迟），松手落盘。
    pub(super) fn on_volume_event(&mut self, event: &SliderEvent, cx: &mut Context<Self>) {
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

    pub(super) fn toggle_pause(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn set_rate(&mut self, rate: f64, cx: &mut Context<Self>) {
        self.rate = rate;
        if let Some(player) = &self.player {
            player.set_rate(rate);
        }
        self.persist_prefs();
        self.wake_controls(cx);
        cx.notify();
    }

    pub(super) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        if let Some(player) = &self.player {
            player.set_muted(self.muted);
        }
        self.persist_prefs();
        self.wake_controls(cx);
        cx.notify();
    }

    /// 偏好落盘（数据目录键值文件，见 [`crate::playback_prefs`]）。
    pub(super) fn persist_prefs(&self) {
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
    pub(super) fn wake_controls(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn controls_visible(&self) -> bool {
        self.scrubbing.is_some()
            || self.picker_open
            || self.player.as_ref().is_some_and(|p| p.is_paused())
            || self.controls_until.is_some_and(|t| Instant::now() < t)
    }

    /// 键盘相对 seek（秒，可负）：钳到 [0, 时长]，唤醒控制条。
    pub(super) fn seek_by(&mut self, secs: f64, cx: &mut Context<Self>) {
        let Some(player) = &self.player else {
            return;
        };
        let dur = player.duration_secs();
        let target = (player.position_secs() + secs).clamp(0.0, dur.max(0.0));
        player.seek(target);
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

    /// 隐身模式开关：一键暂停 + 整窗近透明（恢复显示但不自动续播）。
    pub(super) fn toggle_incognito(&mut self, cx: &mut Context<Self>) {
        self.incognito = !self.incognito;
        if self.incognito && !self.player.as_ref().is_some_and(|p| p.is_paused()) {
            self.toggle_pause(cx);
        }
        cx.notify();
    }

    /// 播放控制条（底部悬浮）：暂停/继续、进度（拖动 seek）、倍速菜单、
    /// 弹幕发送框与开关、清晰度、选集、音量。
    pub(super) fn render_controls(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
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
                // 小屏播放（Tauri onOpenMini 同款）：**同一个窗口**缩成
                // 480×270 落工作区右下角，播放零中断
                Button::new("stage-mini")
                    .icon(gpui_kit::assets::IconName::PictureInPicture2)
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.enter_mini(window, cx);
                    })),
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

    /// 进度轨道（自绘）：细条填充按播放位置实时走（帧事件驱动重渲染）；
    /// 点击即跳转，按住拖动为预览（时间读数跟手），松手才 seek——对齐
    /// Tauri 版「拖动不抢播、松手定位」。轨道矩形经 prepainted 哨兵存取，
    /// 拖动事件只带窗口坐标。
    pub(super) fn render_progress_track(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
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

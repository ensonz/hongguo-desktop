//! 最小化自动暂停（Tauri `use-minimize-pause` 同语义移植）：窗口最小化 →
//! 自动暂停；还原 → 补上欠的那次播放。
//!
//! 判定走 Win32 `IsIconic` 轮询：Tauri 侧同样是 Rust 检测后发事件
//! （WebView2 最小化不派发 visibilitychange，前端无从感知），gpui 的
//! `PlatformWindow` 也没有最小化事件面——轮询与隐身模式同款套路
//! （[`crate::win32`]，间隔放宽到 300ms，出声停止的感知上限）。
//!
//! 欠账模型与 Tauri owedPlay 同构：最小化瞬间本来在播才记「欠一次播放」，
//! 还原补上；用户自己按停的（最小化前就是暂停态）还原后仍停着。设置
//! `pauseOnMinimize` 每拍现读（改开关即时生效，无需重启监听）。macOS：
//! gpui 无最小化事件面，先降级不支持（与隐身的 Apple 侧缺口同口径）。

#[cfg(target_os = "windows")]
use std::time::Duration;

#[cfg(target_os = "windows")]
use gpui_kit::prelude::*;

/// 轮询间隔：最小化到声音停止的感知上限。
#[cfg(target_os = "windows")]
const POLL_MS: u64 = 300;

impl super::HomeView {
    /// 起最小化监听（Windows 专用；随视图生命周期常驻，设置开关每拍现读，
    /// 关着时一拍只是一次 `IsIconic` 调用）。
    #[cfg(target_os = "windows")]
    pub(crate) fn spawn_minimize_poller(&mut self, hwnd: isize, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(POLL_MS))
                    .await;
                let ok = this
                    .update(cx, |view, cx| view.minimize_tick(hwnd, cx))
                    .is_ok();
                if !ok {
                    return; // 视图已销毁
                }
            }
        })
        .detach();
    }

    /// 一拍判定（主线程）。返回 `false` = 视图已销毁。
    #[cfg(target_os = "windows")]
    fn minimize_tick(&mut self, hwnd: isize, cx: &mut Context<Self>) -> bool {
        if crate::win32::is_iconic(hwnd) {
            if !self.state.settings().pause_on_minimize {
                return true;
            }
            if let Some(player) = &self.player
                && !player.is_paused()
            {
                // 只在「正在播」时欠：已经暂停着说明是用户自己按停的
                self.minimize_owed = true;
                player.pause();
                log::info!("[Home] 窗口最小化，自动暂停");
                cx.notify();
            }
        } else if self.minimize_owed {
            self.minimize_owed = false;
            if let Some(player) = &self.player {
                player.resume();
                log::info!("[Home] 窗口还原，续播");
                cx.notify();
            }
        }
        true
    }
}

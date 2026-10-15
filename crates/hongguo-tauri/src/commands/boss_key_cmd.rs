//! 老板键：全局快捷键隐藏 / 显示主窗口（issue #4）。
//!
//! 注册走 tauri-plugin-global-shortcut（Rust 侧直调，不经前端 → 无需
//! capabilities）。切换语义：窗口可见 → 隐藏并向前端发事件（暂停播放，
//! 声音不漏）；已隐藏 → 显示 + 聚焦（前端续播）。快捷键持久化在
//! `Settings.boss_key`（空串 = 关闭），启动装载与保存设置后各 apply 一次；
//! 换键时先注销旧键。GPUI 壳无此层（全局快捷键插件是 Tauri 侧能力）。

use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::ShortcutState;

use hongguo_common::{AppError, AppResult};

/// 前端事件名：老板键切换了窗口可见性（载荷 `{ hidden: bool }`，
/// 前端暂停/续播 `<video>`，声音不漏）。
pub const BOSS_KEY_HIDDEN: &str = "boss-key-hidden";

/// 当前已注册的快捷键（换键注销用）。std 锁 + 中毒恢复，同窗口命令纪律。
static CURRENT: Mutex<Option<String>> = Mutex::new(None);

/// 注册（或换绑 / 注销）老板键。空串 = 注销现有键。
/// 注册失败（快捷键被其他程序占用等）返回错误，由调用方决定提示口径。
pub fn apply(app: &AppHandle, shortcut: &str) -> AppResult<()> {
    let shortcut = shortcut.trim().to_string();
    let mut cur = CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cur.as_deref() == Some(shortcut.as_str()) {
        return Ok(());
    }
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    if let Some(old) = cur.take() {
        // 旧键注销失败会让新键注册时撞「already registered」，必须暴露
        gs.unregister(old.as_str())
            .map_err(|e| AppError::InvalidArgs(format!("旧老板键「{old}」注销失败: {e}")))?;
    }
    if shortcut.is_empty() {
        return Ok(());
    }
    gs.on_shortcut(shortcut.as_str(), |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            toggle(app);
        }
    })
    .map_err(|e| AppError::InvalidArgs(format!("老板键「{shortcut}」注册失败: {e}")))?;
    // 注册后显式核对一次，占用/被顶掉时给出可读错误
    if !gs.is_registered(shortcut.as_str()) {
        let _ = gs.unregister(shortcut.as_str());
        return Err(AppError::InvalidArgs(format!(
            "老板键「{shortcut}」注册后未生效（可能被其他程序占用）"
        )));
    }
    *cur = Some(shortcut);
    Ok(())
}

/// 启动装载后调用：按持久化设置注册。失败只记日志——快捷键被占用不该
/// 拦启动（用户进设置换一个即可）。
pub fn init(app: &AppHandle, shortcut: &str) {
    if shortcut.trim().is_empty() {
        return;
    }
    if let Err(e) = apply(app, shortcut) {
        log::warn!("[BossKey] {e}（进设置换一个快捷键）");
    }
}

/// 可见 ⇄ 隐藏切换，并广播状态给前端（暂停/续播由前端执行）。
fn toggle(app: &AppHandle) {
    use tauri::Emitter;
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let visible = win.is_visible().unwrap_or(false);
    if visible {
        let _ = win.hide();
        let _ = app.emit(BOSS_KEY_HIDDEN, true);
    } else {
        let _ = win.show();
        let _ = win.set_focus();
        let _ = app.emit(BOSS_KEY_HIDDEN, false);
    }
}

/// 设置保存后的重注册钩子（settings_cmd::save_settings 调用）。
#[tauri::command]
pub fn set_boss_key(app: AppHandle, shortcut: String) -> AppResult<()> {
    apply(&app, &shortcut)
}

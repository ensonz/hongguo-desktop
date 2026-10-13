//! 窗口管理命令：小屏播放（enter/exit_mini_screen）、置顶、红绿灯藏显。

use tauri::AppHandle;

use hongguo_common::AppResult;

// ---------------------------------------------------------------- 小屏播放

/// 小屏播放退出时要恢复的窗口几何（进入那一刻的快照，物理坐标）。
#[derive(Clone, Copy)]
struct SavedGeometry {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    maximized: bool,
}

/// 进入小屏前的窗口几何。std 锁 + 中毒恢复（本文件既定纪律：持锁内只有
/// 整体读写，恢复使用即可，不能让一次后台 panic 连环炸掉后续命令）。
static MINI_RESTORE: std::sync::Mutex<Option<SavedGeometry>> = std::sync::Mutex::new(None);

fn saved_geometry() -> std::sync::MutexGuard<'static, Option<SavedGeometry>> {
    MINI_RESTORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 小屏尺寸（对齐 hgplayer 的 Ud/Vd 常量）与落角边距。hgplayer 底部留 72
/// 是给 Windows 任务栏的；macOS 工作区已扣掉 Dock，给 16 即可。
const MINI_W: f64 = 480.0;
const MINI_H: f64 = 270.0;
const MINI_MARGIN: f64 = 16.0;
/// 主窗口平时的最小尺寸（tauri.conf.json 的 minWidth/minHeight）。进小屏
/// 必须先降到小屏尺寸以下，否则缩窗被最小值 clamp 住纹丝不动——
/// hgplayer 进小屏前同样先 `WindowSetMinSize`。
const MAIN_MIN: (f64, f64) = (1024.0, 680.0);

/// 藏/显 macOS 原生红绿灯的三颗圆点（小屏模式用）。
///
/// 刻意**不动 styleMask**：tao 的 `set_decorations` 是整体替换 styleMask，
/// 会把 Overlay 全出血依赖的 `FullSizeContentView` 位一起抹掉——标题条重新
/// 占位 28px、整个 app 内容被顶下去（外框实测 840→868）。而且它走 GCD 主
/// 队列异步块，事后用 `set_title_bar_style` 补位是内联立即执行，永远先于
/// 排队的替换落地、补不过它（探针实测 inner=840/outer=868 复证）。所以直接
/// 对三颗 `NSWindowButton` setHidden：窗口结构零变化，尺寸语义全程稳定。
/// hgplayer 小屏连标题栏都没有，480×270 的画面上再叠三颗圆点纯属多余；
/// Windows 本就 decorations:false，无需此操作。
#[cfg(target_os = "macos")]
fn set_traffic_lights_hidden(app: &AppHandle, hidden: bool) {
    use objc2_app_kit::{NSWindow, NSWindowButton};
    use tauri::Manager;

    // NSWindow/NSButton 的方法不是线程安全的，落到主线程执行
    let app = app.clone();
    let _ = app.run_on_main_thread({
        let app = app.clone();
        move || {
            let Some(win) = app.get_webview_window("main") else {
                return;
            };
            let Ok(ptr) = win.ns_window() else {
                return;
            };
            let ns_window = unsafe { &*(ptr as *const NSWindow) };
            for kind in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(btn) = ns_window.standardWindowButton(kind) {
                    btn.setHidden(hidden);
                }
            }
        }
    });
}

/// 进入小屏播放（对齐 hgplayer 的「小屏播放」按钮 ng()/rw()）：
/// **同一个窗口**缩成 480×270、落到工作区右下角——不另开窗口、不藏
/// 主窗、不暂停，`<video>` 元素原地不动、播放零中断。
///
/// 旧实现（独立置顶小窗 + 隐藏主窗）整套移除：主窗藏了但它的视频还在
/// 响，两路声音叠着播，窗口管理也全乱了套——那是没有对照第三方的
/// 杜撰设计。
#[tauri::command]
pub fn enter_mini_screen(app: AppHandle) -> AppResult<()> {
    use tauri::Manager;
    let Some(win) = app.get_webview_window("main") else {
        return Ok(());
    };

    // 幂等：已在小屏（有快照）就不重复缩，重复点按钮不该把大窗几何
    // 覆盖成 480×270（否则退出小屏恢复的也是小屏尺寸）
    if saved_geometry().is_some() {
        return Ok(());
    }

    // 快照用 inner_size（内容尺寸）：exit 的 set_size 语义就是「设内容
    // 尺寸」，拿外框当内容还原会每圈漂移。红绿灯改为只 setHidden 后
    // （见 set_traffic_lights_hidden）styleMask 全程不动，macOS 下外框
    // 与内容恒等，与无框的 Windows/Linux 行为一致，快照精确往返。
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) else {
        return Ok(());
    };
    *saved_geometry() = Some(SavedGeometry {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
        maximized: win.is_maximized().unwrap_or(false),
    });

    // macOS：藏掉原生红绿灯（只 setHidden 三颗圆点，不动 styleMask，
    // 理由见 set_traffic_lights_hidden）；退出时在 exit_mini_screen 里显回。
    #[cfg(target_os = "macos")]
    set_traffic_lights_hidden(&app, true);

    let mon = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let (left, top, right, bottom) = work_area_logical(mon.as_ref());
    // 小工作区（外接竖屏等）放不下就压到放得下为止
    let w = MINI_W.min(right - left - 2.0 * MINI_MARGIN);
    let h = MINI_H.min(bottom - top - 2.0 * MINI_MARGIN);

    let _ = win.set_min_size(Some(tauri::LogicalSize::new(w, h)));
    let _ = win.set_size(tauri::LogicalSize::new(w, h));
    // 手动 min/max 而非 clamp：极小工作区下 min>max 时 clamp 会 panic
    let x = (right - w - MINI_MARGIN).max(left);
    let y = (bottom - h - MINI_MARGIN).max(top);
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
    let _ = win.set_focus();
    Ok(())
}

/// 退出小屏：恢复进入前的窗口几何与最小尺寸约束。
///
/// 没有快照（应用启动就在小屏、或未进过）就只恢复 minSize 约束——
/// 尺寸位置保持现状，不瞎动。
#[tauri::command]
pub fn exit_mini_screen(app: AppHandle) -> AppResult<()> {
    use tauri::Manager;
    let Some(win) = app.get_webview_window("main") else {
        return Ok(());
    };
    // 原生红绿灯无条件显回（macOS 小屏时被藏了，见 enter_mini_screen）——
    // 放在「无快照提前 return」之前，异常路径也要还原。
    #[cfg(target_os = "macos")]
    set_traffic_lights_hidden(&app, false);
    let saved = saved_geometry().take();
    let _ = win.set_min_size(Some(tauri::LogicalSize::new(MAIN_MIN.0, MAIN_MIN.1)));
    let Some(g) = saved else {
        return Ok(());
    };
    // g.width/height 是进入时的内容尺寸（inner_size），与 set_size 语义
    // 一致，见 enter_mini_screen 里的快照注释
    let _ = win.set_size(tauri::PhysicalSize::new(g.width, g.height));
    let _ = win.set_position(tauri::PhysicalPosition::new(g.x, g.y));
    if g.maximized {
        let _ = win.maximize();
    }
    Ok(())
}

/// 窗口置顶开关（对齐 hgplayer 的 De.pinned → WindowSetAlwaysOnTop）。
///
/// **窗口级**的会话内状态：大屏置顶后进小屏依然置顶，退出小屏也不清——
/// 「钉住」是用户对整个窗口的意图，与窗口大小无关。
#[tauri::command]
pub fn set_always_on_top(app: AppHandle, enabled: bool) -> AppResult<()> {
    use tauri::Manager;
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_always_on_top(enabled);
    }
    Ok(())
}

/// 显示器工作区（逻辑坐标，`left/top/right/bottom`）。
///
/// monitor 的 work_area 是物理像素，HiDPI 下要除以缩放系数才是窗口 API
/// 用的逻辑坐标。拿不到显示器时给一版 14" MacBook 的保守估值——宁小勿
/// 出界，小窗被夹得小一点总比伸到屏幕外强。
fn work_area_logical(mon: Option<&tauri::Monitor>) -> (f64, f64, f64, f64) {
    match mon {
        Some(m) => {
            let sf = m.scale_factor();
            let wa = m.work_area();
            (
                wa.position.x as f64 / sf,
                wa.position.y as f64 / sf,
                (wa.position.x + wa.size.width as i32) as f64 / sf,
                (wa.position.y + wa.size.height as i32) as f64 / sf,
            )
        }
        None => (0.0, 40.0, 1440.0, 875.0),
    }
}

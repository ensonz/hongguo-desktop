//! 小屏播放的窗口几何（Tauri `enter_mini_screen`/`exit_mini_screen` 对齐）：
//! **同一个窗口**缩成 480×270、落到工作区右下角，退出按快照原样还原——
//! 不另开窗口、不藏窗、不暂停，播放零中断（「独立置顶小窗」是 Tauri 侧
//! 已否决的杜撰设计，见 hongguo-tauri `app_cmd.rs` 的注释）。
//!
//! gpui 没有窗口位置/置顶 API：落位与还原走 Win32 FFI（user32 手写、零新
//! 依赖，与 hongguo-media-platform 同风格），HWND 经 gpui `Window` 实现的
//! `HasWindowHandle` 取；钉住（Tauri `set_always_on_top` 同语义）同为 FFI。
//! macOS 暂只做 gpui 自带的 resize（位置/置顶后续补）。

use gpui_kit::Window;

/// 480×270 + 边距（Tauri MINI_W/MINI_H/MINI_MARGIN 同参）。
const MINI_W: f64 = 480.0;
const MINI_H: f64 = 270.0;
const MINI_MARGIN: f64 = 16.0;

/// 进入前的窗口几何快照（退出时原样还原）。
///
/// 存**外框物理矩形**（GetWindowRect 原样）：gpui 的 bounds() 是内容区，
/// 而 SetWindowPos 设的是外框——内容当外框还原，每圈小窗进出都会把窗口
/// 缩一圈边框偏移（实测 1024×680 两圈后变 1008×641）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct SavedGeometry {
    /// 外框矩形（物理像素，全局坐标）。
    rect: (i32, i32, i32, i32),
    maximized: bool,
}

/// 进入小屏：快照 → 缩放落位（工作区右下角，小工作区压到放得下为止）。
/// 几何不可得时返回 `None`，调用方保持原状（与 Tauri 幂等注释同口径：
/// 已在小屏时调用方不得再进来，否则快照会被 480×270 覆盖）。
pub(crate) fn enter(window: &Window, cx: &gpui_kit::App) -> Option<SavedGeometry> {
    let saved = SavedGeometry {
        rect: outer_rect(window)?,
        maximized: window.is_maximized(),
    };
    place_mini(window, cx)?;
    Some(saved)
}

/// 外框矩形（物理像素 left/top/right/width/height 形式的四元组）。
#[cfg(target_os = "windows")]
fn outer_rect(window: &Window) -> Option<(i32, i32, i32, i32)> {
    window_rect(hwnd_of(window)?)
}

#[cfg(not(target_os = "windows"))]
fn outer_rect(window: &Window) -> Option<(i32, i32, i32, i32)> {
    let b = window.bounds();
    Some((
        b.origin.x.to_f64() as i32,
        b.origin.y.to_f64() as i32,
        b.size.width.to_f64() as i32,
        b.size.height.to_f64() as i32,
    ))
}

/// 退出小屏：按快照还原几何与最大化态（Tauri exit 同款顺序：先落位，
/// 后最大化——最大化跟随窗口回到原显示器）。
pub(crate) fn exit(window: &Window, cx: &gpui_kit::App, saved: SavedGeometry) {
    restore(window, cx, saved);
}

/// 钉住窗口（Tauri `set_always_on_top` 同语义：窗口级会话态，大小屏共用
/// 同一个开关，退出小屏不清）。返回成败，调用方**仅在成功时**翻状态——
/// 与 Tauri「先调后端再落 store」同口径。
pub(crate) fn set_topmost(window: &Window, cx: &gpui_kit::App, on: bool) -> bool {
    set_topmost_impl(window, cx, on)
}

// —— 小屏态广播 ——

/// 小屏开着：HomeView 写，Shell 读（侧栏在小屏里藏掉，Tauri use-mini-window
/// 「侧栏隐藏」同款）。窗口缩放本身会触发全树重渲染，Shell 无需额外通知。
static MINI_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_active(on: bool) {
    MINI_ACTIVE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn is_active() -> bool {
    MINI_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(target_os = "windows")]
fn hwnd_of(window: &Window) -> Option<isize> {
    // Window 有个同名固有方法（返回 AnyWindowHandle），必须全限定调到
    // HasWindowHandle 这个 trait 方法才拿得到原生 HWND
    match raw_window_handle::HasWindowHandle::window_handle(window)
        .ok()?
        .as_raw()
    {
        raw_window_handle::RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

/// 把窗口操作派发到执行器（主线程）再执行。
///
/// **不能在调用点同步做**：这些函数都从点击回调里进来，而点击回调正嵌在
/// 窗口过程的一次 `handle.update` 里；同步 `SetWindowPos` 会让 WM_SIZE
/// **同步重入**同一个 update——重入被 gpui 拒绝（`log_err` 吞掉），视口
/// 从此停在旧尺寸（实测舞台停在 1024×680、控制条画到 270 高的窗口外）。
/// gpui 自己的 `resize()` 也是把 SetWindowPos 派发到执行器绕开这一点。
#[cfg(target_os = "windows")]
fn defer_window_op(window: &Window, cx: &gpui_kit::App, op: impl FnOnce() + Send + 'static) {
    window.spawn(cx, async move |_| op()).detach();
}

#[cfg(target_os = "windows")]
fn place_mini(window: &Window, cx: &gpui_kit::App) -> Option<()> {
    let hwnd = hwnd_of(window)?;
    let scale = window.scale_factor() as f64;
    let (left, top, right, bottom) = work_area(hwnd, scale)?;
    // 小工作区（外接竖屏等）放不下就压到放得下为止；手动 min 而非 clamp：
    // 极小工作区下 min>max 时 clamp 会 panic（Tauri 同注释）
    let w = MINI_W.min(right - left - 2.0 * MINI_MARGIN);
    let h = MINI_H.min(bottom - top - 2.0 * MINI_MARGIN);
    let x = (right - w - MINI_MARGIN).max(left);
    let y = (bottom - h - MINI_MARGIN).max(top);
    // SetWindowPos 设的是外框：内容要 480×270（Tauri set_size 语义），
    // 外框 = 内容矩形按四向边偏移外扩（位置也要减去左/顶偏移，否则底边
    // 会越过工作区压到任务栏上）
    let (off_l, off_t, off_r, off_b) = border_offset(hwnd);
    let maximized = window.is_maximized();
    defer_window_op(window, cx, move || {
        // 最大化态直接 SetWindowPos 会与 maximize 摆放打架：先还原到窗口态
        if maximized {
            show_window(hwnd, SW_RESTORE);
        }
        set_window_pos_raw(
            hwnd,
            HWND_TOP,
            (x * scale) as i32 - off_l,
            (y * scale) as i32 - off_t,
            (w * scale) as i32 + off_l + off_r,
            (h * scale) as i32 + off_t + off_b,
            SWP_NOACTIVATE,
        );
    });
    Some(())
}

#[cfg(target_os = "windows")]
fn restore(window: &Window, cx: &gpui_kit::App, saved: SavedGeometry) {
    let Some(hwnd) = hwnd_of(window) else {
        return;
    };
    let (x, y, w, h) = saved.rect;
    let maximized = saved.maximized;
    defer_window_op(window, cx, move || {
        set_window_pos_raw(hwnd, HWND_TOP, x, y, w, h, SWP_NOACTIVATE);
        if maximized {
            show_window(hwnd, SW_MAXIMIZE);
        }
    });
}

#[cfg(target_os = "windows")]
fn set_topmost_impl(window: &Window, cx: &gpui_kit::App, on: bool) -> bool {
    let Some(hwnd) = hwnd_of(window) else {
        return false;
    };
    let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
    defer_window_op(window, cx, move || {
        set_window_pos(
            hwnd,
            after,
            0.0,
            0.0,
            0.0,
            0.0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    });
    true
}

#[cfg(target_os = "windows")]
const HWND_TOP: isize = 0;
#[cfg(target_os = "windows")]
const HWND_TOPMOST: isize = -1;
#[cfg(target_os = "windows")]
const HWND_NOTOPMOST: isize = -2;
#[cfg(target_os = "windows")]
const SWP_NOSIZE: u32 = 0x0001;
#[cfg(target_os = "windows")]
const SWP_NOMOVE: u32 = 0x0002;
#[cfg(target_os = "windows")]
const SWP_NOACTIVATE: u32 = 0x0010;
#[cfg(target_os = "windows")]
const SW_RESTORE: i32 = 9;
#[cfg(target_os = "windows")]
const SW_MAXIMIZE: i32 = 3;
#[cfg(target_os = "windows")]
const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[cfg(target_os = "windows")]
unsafe extern "system" {
    unsafe fn SetWindowPos(
        hwnd: isize,
        after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    unsafe fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    unsafe fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    unsafe fn GetMonitorInfoW(monitor: isize, info: *mut MONITORINFO) -> i32;
    unsafe fn GetWindowRect(hwnd: isize, rect: *mut RECT) -> i32;
    unsafe fn GetClientRect(hwnd: isize, rect: *mut RECT) -> i32;
    unsafe fn MapWindowPoints(from: isize, to: isize, point: *mut POINT, count: i32) -> i32;
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy)]
#[expect(
    clippy::upper_case_acronyms,
    reason = "Win32 API 结构体名保真（POINT 契约）"
)]
struct POINT {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[expect(
    clippy::upper_case_acronyms,
    reason = "Win32 API 结构体名保真（RECTL 契约）"
)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[expect(
    clippy::upper_case_acronyms,
    reason = "Win32 API 结构体名保真（GetMonitorInfoW 契约）"
)]
struct MONITORINFO {
    cb_size: u32,
    rc_monitor: RECT,
    rc_work: RECT,
    dw_flags: u32,
}

/// Win32 调用薄封装：逻辑像素在这里换算成物理像素（窗口 API 只认物理）。
#[cfg(target_os = "windows")]
fn set_window_pos(hwnd: isize, after: isize, x: f64, y: f64, w: f64, h: f64, flags: u32) -> bool {
    // SAFETY：句柄来自 gpui 的 platform window，参数全是本次调用自造值
    unsafe {
        SetWindowPos(
            hwnd,
            after,
            x.round() as i32,
            y.round() as i32,
            w.round() as i32,
            h.round() as i32,
            flags,
        ) != 0
    }
}

/// 物理像素直通版（还原快照用，不做任何换算）。
#[cfg(target_os = "windows")]
fn set_window_pos_raw(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) {
    // SAFETY：同上
    unsafe {
        SetWindowPos(hwnd, after, x, y, w, h, flags);
    }
}

/// 外框矩形（物理像素 left/top/right/bottom）。
#[cfg(target_os = "windows")]
fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY：rc 是本次调用自造的合法缓冲
    let ok = unsafe { GetWindowRect(hwnd, &mut rc) };
    (ok != 0).then_some((rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top))
}

/// 边框偏移（物理像素）：`(左, 顶, 右侧宽, 底部高)`——外框−客户区的四向
/// 差。SetWindowPos 设外框，而小窗目标 480×270 是 Tauri set_size 的
/// 「内容尺寸」语义，落位/定尺寸都要补上这个差（只补总差会把底边推进
/// 任务栏，实测外框底到了 1415 > 工作区底 1376）。
#[cfg(target_os = "windows")]
fn border_offset(hwnd: isize) -> (i32, i32, i32, i32) {
    let mut client = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut outer = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut origin = POINT { x: 0, y: 0 };
    // SAFETY：三个结构都是本次调用自造的合法缓冲；MapWindowPoints 把
    // 客户区 (0,0) 映射到屏幕坐标，得到客户区原点在外框内的偏移
    let ok = unsafe { GetClientRect(hwnd, &mut client) } != 0
        && unsafe { GetWindowRect(hwnd, &mut outer) } != 0
        && unsafe { MapWindowPoints(hwnd, 0, &mut origin, 1) } != 0;
    if !ok {
        return (0, 0, 0, 0);
    }
    let (off_l, off_t) = (origin.x - outer.left, origin.y - outer.top);
    (
        off_l,
        off_t,
        (outer.right - outer.left) - (client.right - client.left) - off_l,
        (outer.bottom - outer.top) - (client.bottom - client.top) - off_t,
    )
}

#[cfg(target_os = "windows")]
fn show_window(hwnd: isize, cmd: i32) {
    // SAFETY：同上，句柄来自 gpui platform window
    unsafe {
        ShowWindow(hwnd, cmd);
    }
}

/// 当前显示器的工作区（任务栏除外；逻辑像素 left/top/right/bottom）。
#[cfg(target_os = "windows")]
fn work_area(hwnd: isize, scale: f64) -> Option<(f64, f64, f64, f64)> {
    let mut info = MONITORINFO {
        cb_size: std::mem::size_of::<MONITORINFO>() as u32,
        rc_monitor: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rc_work: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        dw_flags: 0,
    };
    // SAFETY：info 是本次调用自造的合法缓冲，cb_size 已按约定填好
    let ok =
        unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) };
    if ok == 0 {
        return None;
    }
    let rc = info.rc_work;
    Some((
        rc.left as f64 / scale,
        rc.top as f64 / scale,
        rc.right as f64 / scale,
        rc.bottom as f64 / scale,
    ))
}

#[cfg(not(target_os = "windows"))]
fn place_mini(window: &Window, _cx: &gpui_kit::App) -> Option<()> {
    // macOS：暂只缩不挪（gpui 无位置 API，Apple 侧 FFI 后补）
    window.resize(gpui_kit::Size {
        width: gpui_kit::Pixels(MINI_W as f32),
        height: gpui_kit::Pixels(MINI_H as f32),
    });
    Some(())
}

#[cfg(not(target_os = "windows"))]
fn restore(window: &Window, _cx: &gpui_kit::App, saved: SavedGeometry) {
    window.resize(gpui_kit::Size {
        width: gpui_kit::Pixels(saved.rect.2 as f32),
        height: gpui_kit::Pixels(saved.rect.3 as f32),
    });
}

#[cfg(not(target_os = "windows"))]
fn set_topmost_impl(_window: &Window, _cx: &gpui_kit::App, _on: bool) -> bool {
    log::warn!("[Mini] macOS 置顶暂未实现（窗口 FFI 后补）");
    false
}

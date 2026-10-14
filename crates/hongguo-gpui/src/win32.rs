//! Win32 窗口原语（user32 / dwmapi 手写 FFI，零新依赖）。
//!
//! gpui 没有暴露的窗口系统能力统一落这里（自 `mini.rs` 迁移），当前调用
//! 方：`mini`（小屏落位/还原/置顶）、无边框窗口的边框线消除与 `home::
//! incognito`（隐身藏显）。光标轮询、按窗口藏显在 gpui 的
//! `PlatformWindow` trait 里只有只读面（`visibility()` 无 setter，全局
//! 光标位置无 API），组件库也不覆盖——这是本项目仅存的 FFI 面，新增窗口
//! 系统能力先查 gpui / gpui-component，别往这里堆。

use gpui_kit::Window;

/// gpui `Window` 的原生 HWND。
pub(crate) fn hwnd_of(window: &Window) -> Option<isize> {
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

/// 全局光标位置（物理屏幕坐标）。
pub(crate) fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY：p 是本次调用自造的合法缓冲
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// 窗口是否最小化。
pub(crate) fn is_iconic(hwnd: isize) -> bool {
    // SAFETY：句柄来自 gpui 的 platform window
    unsafe { IsIconic(hwnd) != 0 }
}

/// 窗口当前是否可见（WS_VISIBLE 位；隐身的「藏」走它判定）。
pub(crate) fn is_window_visible(hwnd: isize) -> bool {
    // SAFETY：句柄来自 gpui 的 platform window
    unsafe { IsWindowVisible(hwnd) != 0 }
}

/// 显示窗口并夺焦点（关闭隐身这类「用户要窗口回来」的路径用）。
pub(crate) fn show_foreground(hwnd: isize) {
    show_window(hwnd, SW_SHOW);
    // SAFETY：句柄来自 gpui 的 platform window
    unsafe {
        SetForegroundWindow(hwnd);
    }
}

/// 显示窗口但不夺焦点（隐身里「鼠标回来了」的重现：不打断用户手上的事）。
pub(crate) fn show_no_activate(hwnd: isize) {
    show_window(hwnd, SW_SHOWNA);
}

/// 隐藏窗口。
pub(crate) fn hide(hwnd: isize) {
    show_window(hwnd, SW_HIDE);
}

/// 去掉窗口四周的 1px 系统边框线（Win11 22H2+；失败静默——旧系统没有
/// 这个属性，保留默认外观）。
///
/// gpui 在 Windows 的窗口带 `WS_THICKFRAME`（可缩放边 + 贴靠），DWM 给它
/// 描一圈边框线；Tauri `decorations:false` 是纯 `WS_POPUP` 所以没有这条
/// 线。组件库的 TitleBar 只自绘标题栏内容，边框线不归它管——这里按
/// `DWMWA_BORDER_COLOR = NONE` 关掉线，缩放/贴靠/最大化行为全部保留。
pub(crate) fn hide_border(window: &Window) {
    let Some(hwnd) = hwnd_of(window) else {
        return;
    };
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    let color = DWMWA_COLOR_NONE;
    // SAFETY：句柄来自 gpui 的 platform window，value 是本次调用自造值
    let hr = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            std::ptr::from_ref(&color).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    if hr != 0 {
        log::debug!("[Win32] 去边框线失败（hr={hr}）——系统不支持 DWMWA_BORDER_COLOR，保留默认");
    }
}

/// Win32 调用薄封装：逻辑像素在这里换算成物理像素（窗口 API 只认物理）。
pub(crate) fn set_window_pos(
    hwnd: isize,
    after: isize,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    flags: u32,
) -> bool {
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
pub(crate) fn set_window_pos_raw(
    hwnd: isize,
    after: isize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    flags: u32,
) {
    // SAFETY：同上
    unsafe {
        SetWindowPos(hwnd, after, x, y, w, h, flags);
    }
}

/// 外框矩形（物理像素 left/top/right/bottom）。
pub(crate) fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
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
pub(crate) fn border_offset(hwnd: isize) -> (i32, i32, i32, i32) {
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

pub(crate) fn show_window(hwnd: isize, cmd: i32) {
    // SAFETY：同上，句柄来自 gpui platform window
    unsafe {
        ShowWindow(hwnd, cmd);
    }
}

/// 当前显示器的工作区（任务栏除外；逻辑像素 left/top/right/bottom）。
pub(crate) fn work_area(hwnd: isize, scale: f64) -> Option<(f64, f64, f64, f64)> {
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

pub(crate) const HWND_TOP: isize = 0;
pub(crate) const HWND_TOPMOST: isize = -1;
pub(crate) const HWND_NOTOPMOST: isize = -2;
pub(crate) const SWP_NOSIZE: u32 = 0x0001;
pub(crate) const SWP_NOMOVE: u32 = 0x0002;
pub(crate) const SWP_NOACTIVATE: u32 = 0x0010;
pub(crate) const SW_HIDE: i32 = 0;
pub(crate) const SW_SHOW: i32 = 5;
pub(crate) const SW_SHOWNA: i32 = 8;
pub(crate) const SW_RESTORE: i32 = 9;
pub(crate) const SW_MAXIMIZE: i32 = 3;
const MONITOR_DEFAULTTONEAREST: u32 = 2;

// SAFETY：全部是 user32 / dwmapi 的标准函数签名（Windows 契约保真）。
// user32 由既有链接链带入（mini 路径裸跑多年）；dwmapi 本壳首次用到，
// 显式挂导入库。
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
    unsafe fn GetCursorPos(point: *mut POINT) -> i32;
    unsafe fn IsIconic(hwnd: isize) -> i32;
    unsafe fn IsWindowVisible(hwnd: isize) -> i32;
    unsafe fn SetForegroundWindow(hwnd: isize) -> i32;
}

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

#[link(name = "dwmapi")]
unsafe extern "system" {
    unsafe fn DwmSetWindowAttribute(
        hwnd: isize,
        attr: u32,
        value: *const core::ffi::c_void,
        size: u32,
    ) -> i32;
}

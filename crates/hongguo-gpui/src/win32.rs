//! Win32 窗口原语（user32 手写 FFI，零新依赖）。
//!
//! gpui 没有暴露的窗口系统能力统一落这里（自 `mini.rs` 迁移，小屏落位/
//! 还原/置顶是当前唯一调用方）。新增窗口系统能力先查 gpui /
//! gpui-component，别往这里堆。

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
pub(crate) fn set_window_pos_raw(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) {
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
pub(crate) const SW_RESTORE: i32 = 9;
pub(crate) const SW_MAXIMIZE: i32 = 3;
const MONITOR_DEFAULTTONEAREST: u32 = 2;

// SAFETY：全部是 user32 的标准函数签名（Windows 契约保真）。
// user32 由既有链接链带入（mini 路径裸跑多年）。
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

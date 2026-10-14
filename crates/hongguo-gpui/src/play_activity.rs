//! 播放期的系统活动断言：播放中防止系统闲睡/整机休眠，并向系统声明
//! 「本进程在做用户级工作」——静音、设备缺失等无声播放场景下，桌面
//! 进程仍可能被系统的节能策略降级，主流播放器（VLC/IINA）播放期间都
//! 持有同类断言。用户暂停即释放（用户在场与否由交互决定）。
//!
//! 平台形态：macOS = `NSProcessInfo beginActivityWithOptions`（手写 FFI，
//! 与 vt/ffi 同款纪律：只声明用到的符号，字符串必须用 C 字面量——
//! objc_getClass 不容忍非 NUL 结尾）；Windows =
//! `SetThreadExecutionState`（ES_CONTINUOUS 逐线程生效，调用点固定在
//! 主线程）。其余目标空操作。

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::{c_char, c_void};
    use std::sync::Mutex;

    /// `NSActivityUserInitiated = 0x00FFFFFF | NSActivityIdleSystemSleepDisabled`：
    /// 防 App Nap + 防系统闲睡 + 防突然/自动终止（SDK NSProcessInfo.h 实值）。
    const NS_ACTIVITY_USER_INITIATED: u64 = 0x01FF_FFFF;

    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend(receiver: *mut c_void, op: *mut c_void, ...) -> *mut c_void;
    }

    fn cls(name: &'static std::ffi::CStr) -> *mut c_void {
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn sel(name: &'static std::ffi::CStr) -> *mut c_void {
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// 当前断言的 token（NSActivity 对象地址；None = 未持有）。idempotent
    /// 的记账位：同一状态只 begin/end 一次。
    static TOKEN: Mutex<Option<usize>> = Mutex::new(None);

    pub fn set_active(active: bool) {
        let mut guard = TOKEN.lock().expect("活动断言记账锁");
        if active == guard.is_some() {
            return;
        }
        unsafe {
            let pi = objc_msgSend(cls(c"NSProcessInfo"), sel(c"processInfo"));
            if active {
                let reason = objc_msgSend(
                    cls(c"NSString"),
                    sel(c"stringWithUTF8String:"),
                    c"红果播放中".as_ptr(),
                );
                let token = objc_msgSend(
                    pi,
                    sel(c"beginActivityWithOptions:reason:"),
                    NS_ACTIVITY_USER_INITIATED,
                    reason,
                );
                if !token.is_null() {
                    *guard = Some(token as usize);
                    log::info!("[PlayActivity] 活动断言已持有");
                } else {
                    log::warn!("[PlayActivity] beginActivity 返回空，断言未生效");
                }
            } else if let Some(token) = guard.take() {
                objc_msgSend(pi, sel(c"endActivity:"), token as *mut c_void);
                log::info!("[PlayActivity] 活动断言已释放");
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
    }

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    pub fn set_active(active: bool) {
        if ACTIVE.swap(active, Ordering::Relaxed) == active {
            return;
        }
        // ES_CONTINUOUS 逐线程生效；调用点在主线程 render，翻状态沿也在
        // 主线程，同一线程内声明才持续有效
        let flags = if active {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        let ok = unsafe { SetThreadExecutionState(flags) };
        if ok == 0 {
            log::warn!("[PlayActivity] SetThreadExecutionState 失败");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    pub fn set_active(_active: bool) {}
}

/// 播放态→系统断言的幂等开关：`active` = 有引擎在且未暂停。调用方在
/// 渲染期以最新播放态反复喂入，翻沿才落到平台调用。
pub fn set_active(active: bool) {
    imp::set_active(active);
}

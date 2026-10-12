//! 数据目录与缓存目录解析。
//!
//! 沿用 Electron 版的路径（`%APPDATA%/hongguo-downloader`），便于用户平滑迁移
//! 下载记录与设置，不因为换了框架就丢数据。
//!
//! 多壳并存：壳可在启动最先声明自己的数据域（[`set_shell_scope`]），根目录
//! 下各占一个子目录——库、缓存互不混用，也互不锁库（GPUI 壳与 Tauri 客户端
//! 并存实录：两边抢同一把库锁，后启动的直接拒绝启动）。Tauri 壳不声明，
//! 维持根目录不变（存量用户数据不动）。

use std::path::PathBuf;
use std::sync::OnceLock;

static SHELL_SCOPE: OnceLock<String> = OnceLock::new();

/// 声明本进程的壳数据域（子目录名）。必须在任何路径解析前调用，一次为准。
pub fn set_shell_scope(scope: &str) {
    let _ = SHELL_SCOPE.set(scope.to_string());
}

/// 应用数据目录。
pub fn data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("HONGGUO_DATA_DIR") {
        return PathBuf::from(explicit);
    }
    let base = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hongguo-downloader");
    match SHELL_SCOPE.get() {
        Some(scope) => base.join(scope),
        None => base,
    }
}

/// 数据文件名。
pub const DATA_FILE: &str = "data.json";

/// 数据库文件名（M1 起的正式存储）。
pub const DB_FILE: &str = "hongguo.db";

/// 兼容模式转码缓存目录。
pub fn compat_cache_dir() -> PathBuf {
    data_dir().join("compat-cache")
}

/// 封面转码缓存目录（HEIC→JPEG 产物，按 URL 哈希寻址，可随时清）。
pub fn cover_cache_dir() -> PathBuf {
    data_dir().join("cover-cache")
}

/// 兼容转码缓存上限（沿用现版 4GB）。
pub const COMPAT_CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 数据文件完整路径。
///
/// 仅剩两个用途：旧档迁移检测、测试。新代码一律写 [`db_file`]。
pub fn data_file() -> PathBuf {
    data_dir().join(DATA_FILE)
}

/// 数据库文件完整路径。
pub fn db_file() -> PathBuf {
    data_dir().join(DB_FILE)
}

/// 单测期间把数据目录重定向到临时目录。
///
/// 服务层的落盘路径是写死的 [`data_file`]，直接测「有没有真的写进磁盘」就会
/// 写进用户真实的 `%APPDATA%/hongguo-downloader/data.json`，把下载记录冲掉。
///
/// `HONGGUO_DATA_DIR` 是进程级的：两个用例各改各的会互相把对方的路径顶掉，
/// 于是「断言写到 A 目录」的用例读到的是 B 的文件。所以这里带一把全局锁，
/// 并在 drop 时还原原值——用例中途 panic 也不会把污染留给后面的测试。
// 测试辅助件：本 crate 与下游 crate 的测试都要用（落盘用例不能写进用户真实
// %APPDATA%），而依赖 crate 对下游以非 test 方式编译——`#[cfg(test)]` 对下游
// 不可见。用 feature 显式开启，避免测试代码常驻生产构建。见 Cargo.toml。
#[cfg(any(test, feature = "test-support"))]
pub struct ScopedDataDir {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

#[cfg(any(test, feature = "test-support"))]
impl ScopedDataDir {
    /// 指向 `dir` 并保证它存在。返回的守卫活着期间，本进程的数据读写全落在 `dir`。
    pub fn new(dir: &std::path::Path) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        std::fs::create_dir_all(dir).expect("创建测试数据目录");
        let previous = std::env::var_os("HONGGUO_DATA_DIR");
        // SAFETY: 测试专用。进程级 env 写入在 edition 2024 起标为 unsafe；
        // 上面那把静态锁保证同进程的测试串行改这个变量，无并发窗口。
        unsafe { std::env::set_var("HONGGUO_DATA_DIR", dir) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for ScopedDataDir {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: 同 new——锁还在持有期间（守卫尚未析构完），串行访问
            Some(v) => unsafe { std::env::set_var("HONGGUO_DATA_DIR", v) },
            None => unsafe { std::env::remove_var("HONGGUO_DATA_DIR") },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这两个用例读的是**进程级**的 `HONGGUO_DATA_DIR`，而服务层的用例会通过
    /// [`ScopedDataDir`] 改它。不拿同一把锁的话，一次断言里的两次读可能落到
    /// 两个不同目录上——随机失败（实测约 1/15 次）。
    #[test]
    fn data_file_is_inside_data_dir() {
        let dir = std::env::temp_dir().join(format!("hg-paths-file-{}", std::process::id()));
        let _scope = ScopedDataDir::new(&dir);
        assert_eq!(data_file(), dir.join(DATA_FILE));
        assert_eq!(db_file(), dir.join(DB_FILE));
    }

    #[test]
    fn cache_dirs_are_separate() {
        let dir = std::env::temp_dir().join(format!("hg-paths-cache-{}", std::process::id()));
        let _scope = ScopedDataDir::new(&dir);
        assert_eq!(compat_cache_dir(), dir.join("compat-cache"));
        assert_ne!(compat_cache_dir(), data_dir());
    }

    #[test]
    fn limits_match_legacy_values() {
        assert_eq!(COMPAT_CACHE_MAX_BYTES, 4_294_967_296);
    }
}

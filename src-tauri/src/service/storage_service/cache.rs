//! 缓存占用统计与一键清理。
//!
//! 三处缓存在此汇成一个入口：在线内存流（`play_service::online` 背后的
//! `StreamCache`）、兼容转码产物（`transcode_service::cache` 落
//! `compat-cache/`）、封面/头像转码产物（`protocol::cover` 落
//! `cover-cache/`）。设置页此前只暴露前两处的清理按钮且都不报大小，
//! cover-cache 更是从来没有清理入口——封面图片只进不出。

use serde::Serialize;

use hongguo_store::paths::{compat_cache_dir, cover_cache_dir};

/// 单处缓存的占用。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachePart {
    /// 条目数（磁盘上是文件数，内存流是分条数）。
    pub files: u64,
    pub bytes: u64,
}

/// 三处缓存的占用总览（前端直接消费）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheUsage {
    /// 在线流缓存：在内存里，不占磁盘。
    pub online: CachePart,
    /// 兼容转码缓存：磁盘 `compat-cache/`，只统计 `.mp4`（与清理口径一致）。
    pub compat: CachePart,
    /// 封面/头像转码缓存：磁盘 `cover-cache/`。
    pub cover: CachePart,
}

impl CacheUsage {
    /// 磁盘占用合计。在线流活在内存里，重启即消失，不混进磁盘数字。
    pub fn disk_bytes(&self) -> u64 {
        self.compat.bytes + self.cover.bytes
    }
}

/// 当前占用总览。
pub fn usage() -> CacheUsage {
    let (files, bytes) = crate::service::play_service::online::status();
    CacheUsage {
        online: CachePart {
            files: files as u64,
            bytes,
        },
        compat: dir_part(&compat_cache_dir(), Some("mp4")),
        cover: dir_part(&cover_cache_dir(), None),
    }
}

/// 一键清理的结果（前端直接消费）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheClearResult {
    /// 清掉的条目数（内存分条 + 磁盘文件）。
    pub items: u64,
    /// 释放的磁盘字节（内存流不在磁盘上，不计入）。
    pub bytes: u64,
}

/// 一键清空三处缓存。
pub fn clear() -> CacheClearResult {
    let before = usage();
    let online = crate::service::play_service::online::clear();
    let compat = crate::service::transcode_service::cache::clear();
    let cover = clear_dir(&cover_cache_dir());
    CacheClearResult {
        items: online as u64 + compat as u64 + cover,
        bytes: before.disk_bytes(),
    }
}

/// 目录占用；`ext` 给定时只统计该扩展名的文件（与清理口径一致，统计
/// 与清理对不上就是「清完数字不变」的假清理）。
fn dir_part(dir: &std::path::Path, ext: Option<&str>) -> CachePart {
    let mut part = CachePart::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return part;
    };
    for e in entries.flatten() {
        if !e.path().is_file() {
            continue;
        }
        if let Some(want) = ext
            && e.path().extension().and_then(|x| x.to_str()) != Some(want)
        {
            continue;
        }
        if let Ok(m) = e.metadata() {
            part.files += 1;
            part.bytes += m.len();
        }
    }
    part
}

/// 清空目录下全部文件，返回删掉的个数。目录不存在按空处理。
fn clear_dir(dir: &std::path::Path) -> u64 {
    let mut n = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘用例把数据目录重定向到临时目录（RAII + 全局锁），不然统计和
    /// 清理扫到的是用户真实的 %APPDATA%。
    #[test]
    fn usage_and_clear_cover_both_disk_caches() {
        let dir = std::env::temp_dir().join(format!("hg-cache-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _scope = hongguo_store::paths::ScopedDataDir::new(&dir);

        let cover = cover_cache_dir();
        std::fs::create_dir_all(&cover).unwrap();
        std::fs::write(cover.join("a.jpg"), vec![0u8; 100]).unwrap();
        std::fs::write(cover.join("b.png"), vec![0u8; 50]).unwrap();

        let compat = compat_cache_dir();
        std::fs::create_dir_all(&compat).unwrap();
        std::fs::write(compat.join("x_001.mp4"), vec![0u8; 200]).unwrap();

        let u = usage();
        assert_eq!(u.cover.files, 2, "cover-cache 全部文件都算（jpg/png 混放）");
        assert_eq!(u.cover.bytes, 150);
        assert_eq!(u.compat.files, 1);
        assert_eq!(u.compat.bytes, 200);
        assert_eq!(u.disk_bytes(), 350);

        let r = clear();
        assert_eq!(r.items, 3, "内存流测试里为空，磁盘上 3 个文件");
        assert_eq!(r.bytes, 350);

        let after = usage();
        assert_eq!(after.cover.files + after.compat.files, 0, "清完应归零");
        assert_eq!(after.disk_bytes(), 0);
    }

    #[test]
    fn missing_dirs_read_as_empty() {
        let dir = std::env::temp_dir().join(format!("hg-cache-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _scope = hongguo_store::paths::ScopedDataDir::new(&dir);

        let u = usage();
        assert_eq!(u.compat.files, 0);
        assert_eq!(u.cover.files, 0);
        // 清理空缓存也是一次成功操作
        assert_eq!(clear().items, 0);
    }
}

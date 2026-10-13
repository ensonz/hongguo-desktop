//! 展示格式化：相对时间、计数、时间与倍速文案（与 Tauri 版口径一致）。

impl super::HomeView {
    /// `m:ss`（与 Tauri 版时间格式一致）。
    pub(super) fn fmt_secs(secs: f64) -> String {
        let s = secs.max(0.0) as u64;
        format!("{}:{:02}", s / 60, s % 60)
    }

    /// 倍速按钮文案：整数不带小数点（1x / 1.5x / 0.75x）。
    pub(super) fn fmt_rate(rate: f64) -> String {
        if rate.fract() == 0.0 {
            format!("{}x", rate as i64)
        } else {
            format!("{rate}x")
        }
    }

    /// 相对时间（评论时间戳）。分层与 Tauri `utils/relative-time.ts` 同口径
    /// （hgplayer：刚刚 / N 分钟前 / N 小时前 / 昨天 / N 天前 30 天内 /
    /// M月D日）——此前自创「N 个月前/N 年前」与前端漂移。技术栈不同不共享
    /// 实现，改分层两边同步。
    pub(super) fn relative_time(ts_ms: i64) -> String {
        if ts_ms <= 0 {
            return String::new();
        }
        let secs = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - ts_ms / 1000)
            .max(0);
        match secs {
            0..=59 => "刚刚".to_string(),
            60..=3_599 => format!("{}分钟前", secs / 60),
            3_600..=86_399 => format!("{}小时前", secs / 3_600),
            86_400..=172_799 => "昨天".to_string(),
            172_800..=2_591_999 => format!("{}天前", secs / 86_400),
            _ => {
                use chrono::{Datelike, TimeZone};
                let dt = chrono::Local
                    .timestamp_millis_opt(ts_ms)
                    .single()
                    .unwrap_or_else(chrono::Local::now);
                format!("{}月{}日", dt.month(), dt.day())
            }
        }
    }

    /// 计数格式化：万/亿一位小数、整数省略小数点——与 Tauri
    /// `utils/format.ts` 的 formatCountPrecise 同口径（hgplayer 详情页头部
    /// 实测「27.1万人追剧 / 3786万」）。此前自创「百万」档，已对齐。
    pub(super) fn fmt_count(n: i64) -> String {
        if n >= 100_000_000 {
            return format!("{:.1}亿", n as f64 / 100_000_000.0).replace(".0", "");
        }
        if n >= 10_000 {
            return format!("{:.1}万", n as f64 / 10_000.0).replace(".0", "");
        }
        format!("{n}")
    }
}

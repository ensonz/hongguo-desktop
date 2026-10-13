//! 播放偏好（GPUI 壳）：倍速 / 音量 / 静音 / 弹幕显示参数 / 隐身开关。
//!
//! 语义对齐 Tauri 版的 `playback-prefs`（localStorage）——播放偏好是壳层
//! 关注点，不进业务库；这里用数据目录下一个三行键值文件承载，坏档回默认。
//! 换集重建引擎（对齐 Tauri `<video>` 卸载重挂载）后由壳把存值重放给新引擎。
//!
//! 本壳是 bin-only target（`test = false`，开窗的进程不能进测试集合），
//! 解析逻辑刻意压到几行平凡的 parse/clamp，正确性由引擎侧的钳制测试兜底。

use std::path::Path;

/// 数据目录下的文件名。
pub(crate) const FILE_NAME: &str = "gpui-playback-prefs";

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlaybackPrefs {
    /// 倍速（0.25–4，与引擎 MIN_RATE/MAX_RATE 一致）。
    pub(crate) rate: f64,
    /// 音量（0–1）。
    pub(crate) volume: f32,
    /// 静音独立于音量（对齐 Tauri：静音后音量值保留）。
    pub(crate) muted: bool,
    /// 弹幕开关（Tauri readDanmakuEnabled 同语义）。
    pub(crate) danmaku_on: bool,
    /// 弹幕文字不透明度 0.1–1（Tauri DanmakuDisplaySettings.opacity）。
    pub(crate) dm_opacity: f32,
    /// 弹幕字号缩放 0.5–2（1 = 基准字号）。
    pub(crate) dm_font_scale: f32,
    /// 弹幕密度 0–1（1 = 全部显示，入场按比例丢弃）。
    pub(crate) dm_density: f32,
    /// 弹幕显示区域 0.25–1（占画面顶部高度比例）。
    pub(crate) dm_area: f32,
    /// 隐身模式开关（Tauri readIncognito 同语义：跨启动还原轮询态）。
    pub(crate) incognito: bool,
}

/// 弹幕显示参数默认值（Tauri DANMAKU_DEFAULTS 同款）。
pub(crate) const DM_DEFAULTS: DanmakuPrefs = DanmakuPrefs {
    on: true,
    opacity: 0.9,
    font_scale: 1.0,
    density: 1.0,
    area: 1.0,
};

/// 弹幕显示参数（与播放偏好同文件承载）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DanmakuPrefs {
    pub(crate) on: bool,
    pub(crate) opacity: f32,
    pub(crate) font_scale: f32,
    pub(crate) density: f32,
    pub(crate) area: f32,
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        Self {
            rate: 1.0,
            volume: 1.0,
            muted: false,
            danmaku_on: DM_DEFAULTS.on,
            dm_opacity: DM_DEFAULTS.opacity,
            dm_font_scale: DM_DEFAULTS.font_scale,
            dm_density: DM_DEFAULTS.density,
            dm_area: DM_DEFAULTS.area,
            incognito: false,
        }
    }
}

/// 读偏好；文件缺失/坏行/越界值一律回落默认，不因半写或手改坏档影响起播。
pub(crate) fn load_from(path: &Path) -> PlaybackPrefs {
    let Ok(text) = std::fs::read_to_string(path) else {
        return PlaybackPrefs::default();
    };
    let mut prefs = PlaybackPrefs::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "rate" => {
                if let Ok(v) = value.trim().parse::<f64>() {
                    prefs.rate = v.clamp(0.25, 4.0);
                }
            }
            "volume" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    prefs.volume = v.clamp(0.0, 1.0);
                }
            }
            "muted" => prefs.muted = value.trim() == "true",
            "dm_on" => prefs.danmaku_on = value.trim() == "true",
            "dm_opacity" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    prefs.dm_opacity = v.clamp(0.1, 1.0);
                }
            }
            "dm_font" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    prefs.dm_font_scale = v.clamp(0.5, 2.0);
                }
            }
            "dm_density" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    prefs.dm_density = v.clamp(0.0, 1.0);
                }
            }
            "dm_area" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    prefs.dm_area = v.clamp(0.25, 1.0);
                }
            }
            "incognito" => prefs.incognito = value.trim() == "true",
            _ => {}
        }
    }
    prefs
}

/// 写偏好：同目录临时文件 + rename，半写状态不会留下坏档。
pub(crate) fn save_to(path: &Path, prefs: &PlaybackPrefs) {
    let tmp = path.with_extension("tmp");
    let text = format!(
        "rate={}\nvolume={}\nmuted={}\ndm_on={}\ndm_opacity={}\ndm_font={}\ndm_density={}\ndm_area={}\nincognito={}\n",
        prefs.rate,
        prefs.volume,
        prefs.muted,
        prefs.danmaku_on,
        prefs.dm_opacity,
        prefs.dm_font_scale,
        prefs.dm_density,
        prefs.dm_area,
        prefs.incognito
    );
    if std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, path))
        .is_err()
    {
        log::warn!("[Home] 播放偏好写盘失败（下次启动回默认）");
    }
}

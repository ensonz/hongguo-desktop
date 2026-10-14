//! 预览场景矩阵：页面 × 主题 × 字号缩放 × 面板/空态/失败态分支。
//!
//! 本文件**自洽**（不引 `super::*`）：`tests/preview_scenes.rs` 用 `#[path]`
//! 原样引入跑覆盖率断言（壳是纯 bin，没有 lib 目标，集成测试进不来
//! crate 内部，唯一通路就是源文件级引入；bin 的 `test = false` 也让
//! `#[cfg(test)]` 单测在这 crate 里永远跑不了）。

use gpui_kit::component::theme::ThemeMode;

/// 一张预览图的一个场景。
pub(crate) struct PreviewCase {
    /// 产物文件名（`<name>.png`），全矩阵唯一。
    pub(crate) name: &'static str,
    pub(crate) mode: ThemeMode,
    pub(crate) width: f32,
    pub(crate) height: f32,
    /// 种子键：决定注入哪个面板/状态（`home/preview_seed.rs` 与壳分发消费）。
    pub(crate) scene: &'static str,
    /// 全局字号缩放（gpui-component Theme 的 font_size 倍率）。
    pub(crate) font_scale: f32,
}

/// 1024×680：与 `main.rs` 交互壳的默认窗口同尺寸，预览图即默认首屏的样子。
const W: f32 = 1024.;
const H: f32 = 680.;

pub(crate) fn cases() -> Vec<PreviewCase> {
    vec![
        // —— 首页沉浸流基底（在播叠层 + 控制条 + 封面占位）——
        PreviewCase {
            name: "feed-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "feed",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-dark",
            mode: ThemeMode::Dark,
            width: W,
            height: H,
            scene: "feed",
            font_scale: 1.,
        },
        // —— 首页面板 ——
        PreviewCase {
            name: "feed-picker-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "picker",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-picker-series-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "picker-series",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-downloads-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "downloads",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-downloads-dark",
            mode: ThemeMode::Dark,
            width: W,
            height: H,
            scene: "downloads",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-comments-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "comments",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-comments-dark",
            mode: ThemeMode::Dark,
            width: W,
            height: H,
            scene: "comments",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-login-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "login",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-dm-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "dm",
            font_scale: 1.,
        },
        // —— 首页分支态 ——
        PreviewCase {
            name: "feed-error-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "error",
            font_scale: 1.,
        },
        PreviewCase {
            name: "feed-empty-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "empty",
            font_scale: 1.,
        },
        // —— 排行榜 ——
        PreviewCase {
            name: "rank-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "rank",
            font_scale: 1.,
        },
        PreviewCase {
            name: "rank-dark",
            mode: ThemeMode::Dark,
            width: W,
            height: H,
            scene: "rank",
            font_scale: 1.,
        },
        PreviewCase {
            name: "rank-empty-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "rank-empty",
            font_scale: 1.,
        },
        // —— 磁盘清理 ——
        PreviewCase {
            name: "storage-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "storage",
            font_scale: 1.,
        },
        PreviewCase {
            name: "storage-dark",
            mode: ThemeMode::Dark,
            width: W,
            height: H,
            scene: "storage",
            font_scale: 1.,
        },
        PreviewCase {
            name: "storage-empty-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "storage-empty",
            font_scale: 1.,
        },
        // —— 详情页（头部 + 选集 tab / 相关推荐 tab；明暗各一）——
        PreviewCase {
            name: "detail-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "detail",
            font_scale: 1.,
        },
        PreviewCase {
            name: "detail-dark",
            mode: ThemeMode::Dark,
            width: W,
            height: H,
            scene: "detail",
            font_scale: 1.,
        },
        PreviewCase {
            name: "detail-related-light",
            mode: ThemeMode::Light,
            width: W,
            height: H,
            scene: "detail-related",
            font_scale: 1.,
        },
        // —— 小屏（480×270 播放器形态，侧栏藏掉）——
        PreviewCase {
            name: "mini-light",
            mode: ThemeMode::Light,
            width: 480.,
            height: 270.,
            scene: "mini",
            font_scale: 1.,
        },
        // —— 字号缩放（125% / 150%，对齐系统 DPI 变化下的布局）——
        PreviewCase {
            name: "feed-125-light",
            mode: ThemeMode::Light,
            width: 1100.,
            height: 720.,
            scene: "feed",
            font_scale: 1.25,
        },
        PreviewCase {
            name: "feed-150-dark",
            mode: ThemeMode::Dark,
            width: 1100.,
            height: 720.,
            scene: "feed",
            font_scale: 1.5,
        },
    ]
}

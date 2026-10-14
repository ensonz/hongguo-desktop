//! 目的地页面集合：侧栏 12 目的地里除首页沉浸流（`home/`）与壳内详情页
//! （`detail/`）之外的整页视图，与 Tauri `src/pages/` 的页面一一对应。
//! 每个页面一个模块，对外的入口是各自的 `*View` 实体（壳在 `shell.rs`
//! 装配、切换目的地）。

use gpui_kit::prelude::*;

pub(crate) mod collections;
pub(crate) mod history;
pub(crate) mod merge;
pub(crate) mod liked;
pub(crate) mod list_common;
pub(crate) mod rank;
pub(crate) mod reservations;
pub(crate) mod storage;

/// 页面卡面：Tauri Card 的 GPUI 对齐形态（圆角 12 + 边框 + 卡面底色 +
/// p-6），磁盘清理 / 一键合并 / 设置等设置型页面共用。
pub(crate) fn card(cx: &gpui_kit::App) -> gpui_kit::Div {
    use gpui_kit::component::{ActiveTheme, v_flex};
    let theme = cx.theme();
    v_flex()
        .w_full()
        .rounded(gpui_kit::px(12.0))
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .p_6()
        .gap_4()
}

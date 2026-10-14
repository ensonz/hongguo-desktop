//! 目的地页面集合：侧栏 12 目的地里除首页沉浸流（`home/`）与壳内详情页
//! （`detail/`）之外的整页视图，与 Tauri `src/pages/` 的页面一一对应。
//! 每个页面一个模块，对外的入口是各自的 `*View` 实体（壳在 `shell.rs`
//! 装配、切换目的地）。

pub(crate) mod rank;

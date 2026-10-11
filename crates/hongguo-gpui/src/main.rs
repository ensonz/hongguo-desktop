//! 红果桌面版 GPUI 壳（起步单壳，`docs/architecture-refactor.md` §4.3）。
//!
//! **操作与 Tauri 客户端一致**：装配序（开库 → 设备 adopt → 全局状态）、
//! 接口调用（`state.api_env()` + `hongguo_api`）与 Tauri 壳逐层同源——本壳
//! 不含任何业务逻辑，只做窗口、装配与渲染。演示能力：首页推荐流（tab 16）
//! 的拉取与换一换，同一操作在 Tauri 侧走 `recommend_feed` command 背后的
//! 同一批函数。
//!
//! 异步桥：service 的 async 族假定 tokio 运行时设施（`tokio::time::sleep`、
//! `Semaphore`），而 GPUI 自带的是 smol 系执行器。本壳持一个常驻 tokio
//! 运行时线程，GPUI 任务经 `oneshot` 回传结果——`Receiver` 不依赖 tokio
//! 上下文，可在 GPUI 执行器上直接 await。

use std::sync::Arc;
use std::sync::OnceLock;

use gpui::prelude::*;
use gpui::{
    App, Application, Bounds, Context, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use hongguo_api::recommend::fetch_recommend_feed;
use hongguo_common::AppError;
use hongguo_service::app_state::AppStateInner;
use hongguo_store::Store;

/// 常驻 tokio 运行时（service 异步族的桥）。
static TOKIO: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn tokio() -> &'static tokio::runtime::Runtime {
    TOKIO.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio 运行时初始化失败")
    })
}

fn main() {
    // 日志先于一切：装配失败的原因只在这里（与 Tauri 壳同一约定）。
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let state = build_app_state();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.), px(520.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| ShellWindow::new(state.clone(), cx)),
        )
        .expect("窗口创建失败");
        cx.activate(true);
    });
}

/// 开库并组装全局状态。与 Tauri 壳的装配逐层同源：
/// 开库（锁冲突短重试一次）→ [`hongguo_service::device::adopt_sync`] →
/// [`AppStateInner::with_device`]。
fn build_app_state() -> Arc<AppStateInner> {
    let db = match open_store_with_lock_retry() {
        Ok(db) => db,
        Err(AppError::StoreLocked(e)) => {
            log::error!(
                "[Store] 数据库被占用，终止启动: {e}\n\
                 另一个实例可能还在运行（Tauri 客户端隐身模式会把窗口整个藏掉）。"
            );
            std::process::exit(1);
        }
        Err(e) => {
            log::error!("[Store] 数据库打开失败，终止启动: {e}");
            std::process::exit(1);
        }
    };
    let (device, device_source) = hongguo_service::device::adopt_sync(&db);
    log::info!(
        "[Device] 现役档案 iid={}（{device_source}）",
        device.get("iid")
    );
    Arc::new(AppStateInner::with_device(db, device))
}

/// 开库；被锁时稍等重试一次（与 Tauri 壳同一口径）。
fn open_store_with_lock_retry() -> hongguo_common::AppResult<Store> {
    match Store::open(hongguo_store::paths::db_file()) {
        Ok(db) => Ok(db),
        Err(AppError::StoreLocked(first)) => {
            log::warn!("[Store] 数据库暂时被锁（{first}），1s 后重试一次");
            std::thread::sleep(std::time::Duration::from_secs(1));
            Store::open(hongguo_store::paths::db_file())
        }
        Err(e) => Err(e),
    }
}

// ————————————————————————————————————————————————————————————
// 壳窗口：首页推荐流（tab 16）
// ————————————————————————————————————————————————————————————

/// 推荐流页。`items` 为最近一次成功响应；`status` 是人话状态行
/// （加载中 / 条数 / 错误——错误原文进日志，界面只留一句）。
struct ShellWindow {
    state: Arc<AppStateInner>,
    items: Vec<hongguo_api::discover::FeedItem>,
    session: String,
    status: String,
    loading: bool,
}

impl ShellWindow {
    fn new(state: Arc<AppStateInner>, cx: &mut Context<Self>) -> Self {
        let mut win = Self {
            state,
            items: Vec::new(),
            session: String::new(),
            status: "加载中…".into(),
            loading: false,
        };
        win.refresh(cx);
        win
    }

    /// 拉一页推荐流。与 Tauri 的 `recommend_feed` command 同一服务面：
    /// `state.api_env()` + [`fetch_recommend_feed`]（tab 16 = 推荐）。
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.status = "加载中…".into();
        cx.notify();
        let state = Arc::clone(&self.state);
        let session = std::mem::take(&mut self.session);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio().spawn(async move {
            let env = state.api_env();
            let result = fetch_recommend_feed("16", &session, 0, &[], &env).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            this.update(cx, |win, cx| {
                win.loading = false;
                match result {
                    Ok(page) => {
                        log::info!(
                            "[Recommend] GPUI 壳 -> {} 条, 首条: {}",
                            page.items.len(),
                            page.items.first().map(|i| i.title.as_str()).unwrap_or("-")
                        );
                        win.status = format!("{} 条", page.items.len());
                        win.session = page.session_id;
                        win.items = page.items;
                    }
                    Err(e) => {
                        log::warn!("[Recommend] GPUI 壳失败: {e}");
                        win.status = "加载失败（详见日志）".into();
                    }
                }
                cx.notify();
            })
        })
        .detach();
    }
}

impl gpui::Render for ShellWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 深色底（与应用视觉一致的方向）；列表仅标题行——封面图与播放操作
        // 待 §5.3 帧流 spike 定契约后按 feature 演进。
        let items = self
            .items
            .iter()
            .map(|i| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x1F1F26))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xEDEDF0))
                            .child(i.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x8A8A94))
                            .child(format!("{} 集", i.episode_cnt)),
                    )
            })
            .collect::<Vec<_>>();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x101014))
            .child(
                // 顶栏：标题 + 换一换
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(0x1F1F26))
                    .child(
                        div()
                            .text_base()
                            .text_color(rgb(0xEDEDF0))
                            .child("红果 · GPUI 壳（首页推荐流）"),
                    )
                    .child(
                        div()
                            .id("refresh")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x1F1F26))
                            .text_sm()
                            .text_color(rgb(0xEDEDF0))
                            .hover(|s| s.bg(rgb(0x2A2A33)))
                            .child(if self.loading { "…" } else { "换一换" })
                            .when(!self.loading, |s| {
                                s.on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|win, _, _, cx| win.refresh(cx)),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(0x8A8A94))
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .id("feed-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(items),
            )
    }
}

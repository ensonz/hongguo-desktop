//! 红果桌面版 GPUI 壳（起步单壳，`docs/architecture-refactor.md` §4.3）。
//!
//! **操作与 Tauri 客户端一致**：装配序（开库 → 设备 adopt → 全局状态）、
//! 接口调用（`state.api_env()` + `hongguo_api`）与 Tauri 壳逐层同源——本壳
//! 不含任何业务逻辑，只做窗口、装配与渲染。UI 按 `.agents/skills/
//! gpui-ui-design` 的规范走 GPUI Kit（gpui-component 组件系统 + 主题令牌），
//! 持久侧栏导航对齐 Tauri 客户端的信息架构（首页 / 排行）。
//!
//! 异步桥：service 的 async 族假定 tokio 运行时设施（`tokio::time::sleep`、
//! `Semaphore`），而 GPUI 自带的是 smol 系执行器。本壳持一个常驻 tokio
//! 运行时线程，GPUI 任务经 `oneshot` 回传结果——`Receiver` 不依赖 tokio
//! 上下文，可在 GPUI 执行器上直接 await。
//!
//! 范围：浏览类操作（推荐流 / 排行榜，含封面、加载与失败态）。剧集详情与
//! 播放（帧流 → GPU 纹理）是 §5.3 spike，另行立项。

mod bridge;
mod home;
mod http_client;
mod rank;
#[cfg(feature = "screenshot")]
mod screenshot;
mod shell;

use std::sync::Arc;
use std::sync::OnceLock;

use gpui_kit::prelude::*;
use hongguo_common::AppError;
use hongguo_service::app_state::AppStateInner;
use hongguo_store::Store;

/// 常驻 tokio 运行时（service 异步族的桥）。
static TOKIO: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub(crate) fn tokio() -> &'static tokio::runtime::Runtime {
    TOKIO.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio 运行时初始化失败")
    })
}

fn main() {
    // 独立数据域最先声明：GPUI 壳与 Tauri 客户端各占一个数据子目录，
    // 互不锁库（实测并存时后启动者因库锁直接退出）。
    hongguo_store::paths::set_shell_scope("hongguo-gpui");
    // 日志先于一切：装配失败的原因只在这里（与 Tauri 壳同一约定）。
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let state = build_app_state();

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        // 封面是 CDN 签名 URL：gpui 图片缓存需要一个 HTTP 客户端才能拉远程资源。
        http_client::ReqwestClient::install(cx);
        // 客户端视觉身份是深色（与 Tauri 壳一致）；后续接跟随系统。
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        let bounds = gpui_kit::Bounds::centered(
            None,
            gpui_kit::size(gpui_kit::px(1024.), gpui_kit::px(680.)),
            cx,
        );
        // 默认构建用不到这个句柄（只有 screenshot feature 抓帧要），
        // 关掉 feature 时按「有意不用」放行，别为一个变量把 feature 拧到反。
        #[cfg_attr(
            not(feature = "screenshot"),
            expect(unused_variables, reason = "句柄只有 screenshot feature 用")
        )]
        let (shell_window, shell) = gpui_kit::open_window(
            gpui_kit::WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(bounds)),
                // 标题空着的话任务栏是一条空白条目，用户根本认不出这个窗口
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("红果".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), cx)),
        )
        .expect("窗口创建失败");
        shell.update(cx, |shell, cx| shell.activate(cx));
        // 图形链路取证：HONGGUO_SHOT=… 时进程内抓帧存 PNG（见截图模块头注释）
        #[cfg(feature = "screenshot")]
        screenshot::install(shell_window, cx);
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
                 另一个 GPUI 壳实例可能还在运行（隐身时窗口整个藏掉，注意杀进程）。"
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

# AGENTS.md — hongguo-desktop

红果短剧（Hongguo）非官方桌面客户端：Tauri 2 + 纯 Rust 业务内核 + React 19 界面层。Windows / macOS。仅供学习研究（PolyForm Noncommercial），不存储、不分发任何影视内容。

## 开始任何开发前

**先读 `.agents/skills/hongguo-dev/SKILL.md`**（完整开发规范），再按任务读对应参考文档：

| 你要做的事                                               | 先读                                                |
| -------------------------------------------------------- | --------------------------------------------------- |
| 环境搭建 / 日常命令 / 调试 / 提交 / 发版                 | `.agents/skills/hongguo-dev/references/workflow.md` |
| 新增（或改动）IPC 命令、事件、外部 API 端点              | `.agents/skills/hongguo-dev/references/ipc.md`      |
| 写前端：组件 / hooks / 查询 / store / 样式 / i18n / 路由 | `.agents/skills/hongguo-dev/references/frontend.md` |
| 写后端：command / service / store / 迁移 / 测试          | `.agents/skills/hongguo-dev/references/backend.md`  |
| 对接官方 App 接口（抓包 / 端点 / 参数）                  | `docs/hongguo-api-endpoints.md`                     |
| 逆向分析官方 APK / hgplayer（协议 / 端点考证）           | `.agents/skills/hongguo-dev/references/reverse.md`  |
| WebView 调试 / CDP 自动化                                | `docs/webview-cdp-testing.md`                       |

## 目录结构

- `src/` 前端：React 19 + TS + Vite + TanStack Router/Query + Zustand + Tailwind v4 + shadcn/ui + zod。`pages/` + `features/` + `service/` + `stores/` + `components/`
- `src-tauri/` Tauri 壳的**纯配置 + 入口**：只承载 `tauri.conf.json` / `capabilities` / `icons` / `gen` 与 `generate_context!`（必须与配置同 crate），`lib.rs` 仅数行；全部装配见 `crates/hongguo-tauri/`
- `crates/hongguo-common/` 共用底座：领域模型（`model/`）、统一错误（`error.rs`，无 tauri 依赖）、纯工具（`utils/`）、跨端常量（`consts/`）。Tauri 壳与未来 GPUI 壳共用；常量归属规则见 `consts/mod.rs` 头注释
- `crates/hongguo-signer/` 请求签名：字节系五头签名 + 设备注册原语；**JS 1:1 移植禁改写法**，`constants.rs` 的值与服务端一一对应不得改动（无 common / tauri 依赖）
- `crates/hongguo-mp4/` MP4 解析与 CENC 解密：box/样本表/稀疏缓冲/流式计划 + CENC-AES-CTR + 密钥派生；`fixtures` 是 feature `test-support` 门控的测试辅助（下游测试造 MP4 样本用）
- `crates/hongguo-api/` 官方接口客户端：14 个域（登录/推荐/搜索/详情/流地址/弹幕/互动等）；改端点或参数前必读 `docs/hongguo-api-endpoints.md`。commentapi 互动枚举（object_type / comment_type / commit_source / data_type 等，值即契约）集中在 `params::interaction`，使用点只引用不写字面量
- `crates/hongguo-store/` 持久化：Turso 内嵌库（`db` 连接与 schema 迁移 / `entity/` 六组实体 / `bridge` 同步门面）、旧 data.json 一次性导入、坏档恢复；`paths::ScopedDataDir` 是 feature `test-support` 门控的测试辅助（落盘用例把数据目录重定向到临时目录）。无 tauri 依赖
- `crates/hongguo-stream/` 在线流契约：内存渐进流（`ProgressiveStream`）、按 (vid, 档位) 分条的缓存（`StreamCache`）、Range 应答（`serve`）+ `range`（RangeSpec/ProtocolResponse）、进程级 `cache()` 单例。**填充侧（service）与应答侧（protocol）共用的上游**——两侧都只往下依赖它，禁止反向。无 tauri 依赖
- `crates/hongguo-media/` 纯 Rust 编解码：demux / hevc / remux / transcode / codec_probe / disk_space / heif / backend（转码链路的兜底层，零外部依赖）；`tests/fixtures/` 放随代码走的样本。平台硬编、ffmpeg 执行与能力探测在 `crates/hongguo-media-platform`，依赖本 crate。无 tauri 依赖
- `crates/hongguo-media-platform/` 平台层编解码：`platform`（macOS VideoToolbox / Windows Media Foundation / WIC 平台硬编，FFI 手写零新增依赖）+ `ffmpeg`（外部进程的探测与执行）+ `capability`（解码能力探测）；转码三级分流编排仍在 service 的 `transcode_service::pipeline`。`platform::transcode_h264_for_tests` 由 feature `test-support` 门控（壳 e2e 用）。无 tauri 依赖
- `crates/hongguo-protocol/` 自定义 URI 协议供数逻辑：`url`（按平台的 URL 形态与编解码）+ `local` + `cover`（HEIC 封面三级转码阶梯）+ `pool`（常驻工作池与 panic 加固）+ `serve`（三路供数入口，产出元组）。**无 tauri 依赖**——HTTP 组装与 Builder 注册是壳胶水（hongguo-tauri 的 `protocol.rs`）
- `crates/hongguo-service/` 业务编排层：九域 service（download / play / merge / login / series / settings / storage / transcode / capability）+ 全局业务状态 `AppState`（壳经 Tauri `manage` 注入，GPUI 壳直接构造）。tauri 依赖仅取 `async_runtime`（后台任务统一走它，红线 1）
- `crates/hongguo-tauri/` Tauri 装配层：`commands`（IPC 薄层）、`bootstrap`（启动装配）、`diagnostics`（panic 取证）、`protocol.rs`（协议注册胶水）、`testing`（跨层 e2e 与守卫元测试）——`prepare()` 产出装好的 Builder，`build(generate_context!)` 留在 src-tauri
- `crates/hongguo-gpui/` GPUI 壳（起步单壳）：直接构造 `AppStateInner` 并调用 service/api，**操作与 Tauri 客户端一致**（不复制业务；`make gpui` 运行）。service 的 async 族假定 tokio 设施，壳内持常驻 tokio 运行时经 oneshot 桥接；视频帧流是 §5.3 spike，未做
- Rust 侧已 workspace 化：根 `Cargo.toml`（members / `resolver` / `[patch]` / `[profile]` 都在根），`Cargo.lock`、`target/`、`vendor/` 也都在仓库根；cargo 一律在**仓库根**跑（`--workspace`）。`vendor/` 被 `[workspace] exclude` 排除，不参与 lint/test/format
- `captures/` 抓包工具与数据（`addon.py` 抓包 / `audit.py` 参数审计），`docs/` 仓库文档，`scripts/` 构建辅助脚本

前后端只通过三条通道交互：IPC command、事件推送、自定义协议 `hongguo-local/stream/cover`。

## 常用命令

```bash
make dev        # 开发模式（Vite + Tauri，端口 1420 固定、单实例）
make test       # 前端 vitest（= pnpm test）
make test-rust  # cargo test
make lint       # 质量闸门一票否决：ESLint + Prettier + cargo clippy×3（Windows 宿主 + macOS 双目标交叉）
make typecheck  # tsc 两个 tsconfig --noEmit
make fmt        # cargo fmt
make release    # tauri build 打包
```

包管理用 pnpm。门禁命令必须**裸跑**确认退出码——管道（如 `pnpm x | tail`）会吞掉失败。

## 硬红线（违反即返工，完整清单与背景见 skill）

1. command 层禁止 `tokio::spawn` / `Handle::current` / `block_on`——同步 command 跑在主线程 IPC 回调里没有 Tokio 上下文，会 panic→abort；后台任务一律 `tauri::async_runtime::spawn`（有静态测试扫源码盯着）。
2. 前端组件不直接 `invoke`——统一走 `service/commands/` 与 `service/tauri/invoke.ts` 的 `call<T>()`（zod 校验 + 错误 i18n 归一）。
3. 事件名两端同步改：Rust `crates/hongguo-service/src/service/download_service/events.rs` 的 `names` ↔ 前端 `service/tauri/types.ts` 的 `EVENTS`，只改一端会静默失效。
4. 签名（`signer/ticket.rs` 产出 x-gorgon 等五件套）之后不能再动 url/body/query——连 query 顺序、URL 编码方式都会失配；`signer/` 是 JS 1:1 移植，禁改写法。
5. 锁永远不跨 await（parking_lot guard 非 Send）。
6. Cargo release profile 绝不能加 `panic = "abort"`；clippy 带 `-D warnings -D clippy::allow_attributes`，确需豁免用 `#[expect(reason)]` 而非 `#[allow]`。
7. `src-tauri/lib.rs` 只做入口（数行）；装配在 `hongguo-tauri`，各 crate 边界显式定义对外 API，禁止 `pub use *` 泄底。
8. 启动顺序：单实例插件必须第一个注册，数据库在 `.setup()` 里晚于它打开。
9. 对接官方接口禁止猜参数——照 `docs/hongguo-api-endpoints.md` 抓包逐字段对齐。
10. 抓包对齐必须「请求 + 响应 + UI 效果」三点一线，只对齐请求等于没对齐：响应 body 逐字段解剖（发送类接口的响应是完整对象回显，丢弃=UI 缺字段）；计数类字段第三方做本地乐观 ±1；对照第三方实操效果核对渲染。
11. UI 用 shadcn `Button` variant + 语义令牌，禁止硬编码色值（`bg-red-500` 等）；唯一例外是播放器控制栏/互动栏/评论面板的 hgplayer 红色皮肤。
12. 禁止用 Python 脚本改项目代码——改代码只准 Edit/Write 工具；Python 可以写辅助工具（放工具目录正常提交），一次性分析脚本用完即删。
13. 开发在 dev 分支，conventional commits + 中文描述；纯结构移动/拆分与行为修改分开提交。
14. 大规模结构调整先出计划文档，经确认再动代码。

## 其他 gotcha

- git hooks 需克隆后手动启用：`git config core.hooksPath .githooks`（pre-commit 跑三目标 clippy + cargo test + eslint）。
- 用户可见文案改动必须同步 `src/locales/zh-CN.json` 与 `en-US.json` 两份。
- 改名 / 换 logo 后重跑 `make assets`（离线，产物生成后提交）；升级 Tauri 后重跑 `make nsis`。

# 红果桌面客户端 · 架构重构方案

> 版本：2026-10-11（rev3，定序已确认）
> 状态：已定序，按 §8 分批执行（阶段 0/1 先行，其余待对应阶段）
> 目标：提升后端扩展性，支持 Tauri（React）与 GPUI 长期共存，流处理契约与 UI 壳解耦
>
> **rev3**：双端共存确立为方案前提（非待决项，本方案即双端改造方案）；定序确认——拆分前置项先做、重叠项并入对应 crate（见 §8）。
>
> **rev2 变更**（对着 `src-tauri/src` 实测校正，v1 的部分设计已推翻）：
> - §5 全量重写。v1 的 `StreamSource/StreamPipeline/StreamHandle` 是凭空设计的，与现有
>   `protocol/stream` 的「推 + 缓存 + Range 应答」形态对不上（详见 §5.1）。
> - 砍掉投机抽象：`hongguo-media-gpu`、9 个 `hongguo-gpui-*` feature crate、前端 `apps/web`
>   搬家、pnpm workspace——目标用不上，属「为将来铺路」（见 §14 不作）。
> - 拆分优先级改按**实测耦合**排，不按 v1 的列表顺序（见 §0）。
> - 新增 §9「workspace 化的机械耦合清单」——v1 一处没列，照它落地会成批炸。
> - 修正 §13 的失效引用（v1 引了两篇已删文档）、§12 决策改标「待确认」。

---

## 0. 现状基线与结论（实测，2026-10-11）

| 项 | 测量值 | 说明 |
| --- | --- | --- |
| 后端规模 | **198 文件 / 41,890 行**（`src-tauri/src`） | 单 crate，无 workspace、无根 `Cargo.toml` |
| 模块（文件/行） | domain 58/15994、media 27/8561、service 38/5184、signer 19/3953、commands 19/2302、protocol 8/2249、store 13/1818、bootstrap 6/611、utils 5/410 | `Get-ChildItem -Recurse -Include *.rs` 实测 |
| 前端规模 | 223 文件 / 20,128 行（`src`） | |
| 实测耦合（`use crate::X` 站点数） | `signer` 33、`domain::mp4` 33、`media` 11、`protocol` **4** | 枢纽是 signer / mp4 / media |
| 元测试 | `commands/mod.rs`、`service/mod.rs` 用 `include_str!` **硬编码清单**扫源码 | command 禁 tokio、事件名稳定 |
| 流处理现状 | Rust 只到「解密后的明文 MP4 字节」；解码/渲染全在 WebView | 见 §5.1 |

**结论**：拆分优先级按**实测耦合**排——`signer` / `domain::mp4` / `media` 是枢纽先拆，`protocol` 极薄（4 处）靠后。v1 的 crate 列表顺序与实测不符。

---

## 1. 背景与问题

当前 `src-tauri` 是单体 crate（`hongguo-desktop` / `hongguo_lib`），内部已按 `commands → service → domain/api + store + media + protocol + signer` 逻辑分层，但：

- 编译单元过大（198 文件 / 41,890 行），改动触发大范围重编译（**未有量化基线，见 §11**）。
- 签名、官方 API、媒体管线、协议、业务编排全耦合在同一 crate。
- 引入 GPUI 时无法干净复用核心能力。
- 流处理逻辑与 Tauri 自定义协议绑定较深——**且现有形态是「推 + 缓存 + Range 应答」，不是可拉取的字节流**（§5.1）。

---

## 2. 设计原则

| 原则 | 说明 |
| --- | --- |
| 依赖单向向下 | 上层依赖下层，禁止反向与循环依赖 |
| UI 壳薄化 | Tauri / GPUI 只做装配、窗口、协议注册、IPC，不写业务 |
| **契约与实现分离** | 契约 crate 不吃实现依赖——否则「与 UI 解耦」只是纸面（v1 把流 trait 塞进带 remux/ffmpeg 的 media crate，正是反例） |
| **闭集用 enum，不用 trait object** | 对齐仓库既有范式 `media/backend.rs`（`Backend` 枚举 + 优先级），不引热路径动态分发 |
| **按实测耦合定拆分优先级** | 枢纽先拆，薄层靠后（见 §0） |
| 零无用抽象 | 只在有独立状态与生命周期的能力上拆分；不建「将来会用到」的 crate |
| 契约同源 | 跨端传输字段、事件名、IPC 命令严格同步，禁止漂移 |
| 产品视觉身份显式化 | 设计 token / 主题有明确归属，避免模板化默认风格 |

---

## 3. 整体架构

```
┌─────────────────────────────────────────────────────────────────┐
│                      UI 壳层（薄）                               │
│  ┌──────────────────────┐      ┌──────────────────────────────┐ │
│  │   hongguo-tauri      │      │   hongguo-gpui（起步单壳）    │ │
│  │ IPC/协议注册/装配      │      │ 窗口装配；feature 有代码再拆   │ │
│  └──────────┬───────────┘      └──────────────┬───────────────┘ │
│             └───────────────┬─────────────────┘                 │
│                             ▼                                   │
│                    hongguo-service（业务编排，UI 无关）           │
│         ┌───────────────────┼───────────────────┐               │
│         ▼                   ▼                   ▼               │
│  hongguo-protocol    hongguo-media       hongguo-store          │
│  （自定义 URI 处理）  （编解码实现）        （Turso 持久化）        │
│         │                   │                                   │
│         └────────┬──────────┘                                   │
│                  ▼                                              │
│           hongguo-stream   ← 新增：流契约（下游投递的共用上游）    │
│                  │                                              │
│   hongguo-mp4（MP4/CENC） · hongguo-api ← hongguo-signer · common │
└─────────────────────────────────────────────────────────────────┘
```

- **Tauri 路径**：React 前端 → IPC → `hongguo-tauri` → `hongguo-service`
- **GPUI 路径**：GPUI 壳 → `hongguo-service`（直接调用）
- 两端共享：service / stream 契约 / mp4 / store / api 客户端
- **下游投递天然分叉**（这是 v1 没看清的关键）：Tauri 按 Range 回**明文 MP4 字节**，由 WebView 解码；GPUI 需**解码帧 → GPU 纹理**，是全新末端（§5.3）

---

## 4. Crate 划分

### 4.1 基础设施（稳定、少变）

| Crate | 职责（对应现有代码） | 依赖 |
| --- | --- | --- |
| `hongguo-common` | 共用 serde 模型（`domain/model/`）、`AppError`（`error.rs`）、工具（`utils/`） | 几乎无 |
| `hongguo-signer` | `signer/`——五头签名（x-gorgon 等）+ 设备注册原语；**自带 TT 哈希/加密（helios/medusa/tt_hash）**，JS 1:1 移植禁改写法 | common |
| `hongguo-mp4` | `domain/mp4/` + `domain/crypto/`——MP4 盒/样本表/稀疏缓冲/流式计划 + CENC-AES-CTR 解密 + 密钥派生 | common |
| `hongguo-api` | `domain/api/`——官方 API 客户端（登录/推荐/搜索/详情/流地址/弹幕/互动等） | signer + common |
| `hongguo-store` | `store/`——Turso 持久化、entity、迁移、恢复 | common |

> **命名修正**：v1 叫 `hongguo-crypto`，但它的内容（CENC + MP4 解析）主体是容器解析，`domain/crypto/` 只是 CENC 原语。改叫 `hongguo-mp4`。**`signer` 的 TT 哈希/加密与 CENC 是两回事，不合并**。

### 4.2 流契约、媒体与协议

| Crate | 职责 | 依赖 |
| --- | --- | --- |
| `hongguo-stream` | **新增薄契约**：流源类型、明文供给抽象、读者需求登记等**共用数据类型**（把今天的 `ProgressiveStream` / `StreamCache` / `ReaderDemand` / `StreamingPlan` 收拢到这里） | mp4 + common |
| `hongguo-media` | `media/`——demux / remux / 纯 Rust 软解 / 能力探测 / 后端选择器 | mp4 + common |
| `hongguo-media-platform` | `media/platform`（MF / VT / WIC）+ `media/ffmpeg`——平台硬编与 ffmpeg 执行，feature 门控 | media |
| `hongguo-protocol` | `protocol/`——自定义 URI 逻辑（`hongguo-local/stream/cover`）+ Range 应答。**注意命名：这是 URI 协议，不是 HTTP/传输协议** | stream + mp4 + common |

> **不设 `hongguo-media-gpu`**：GPU 路径尚不存在，有真需求再立 crate（§14）。
> **`service` 不再依赖 `protocol`**：今天 `service/play_service` 直接 `use protocol::stream::{ProgressiveStream, StreamCache}`——收拢进 `hongguo-stream` 后，service → stream，protocol → stream，去掉这层别扭耦合。

### 4.3 业务与壳

| Crate | 职责（对应现有代码） | 依赖 |
| --- | --- | --- |
| `hongguo-service` | `service/`——download / play / merge / series / settings / storage / transcode 编排 | api + media + stream + store + common |
| `hongguo-tauri` | `commands/` + `app_state.rs` + `bootstrap/` + `diagnostics.rs` + 入口（`lib.rs`）；IPC、协议注册、装配 | service + protocol + stream + tauri |
| `hongguo-gpui` | GPUI 应用壳：初始化、开窗口、（后续）组合 feature | service + stream + media |

### 4.4 GPUI Feature 拆分（**降级为有代码再谈**）

v1 预列了 9 个 `hongguo-gpui-*` crate（theme/player/discover/series/download/library/settings/shared）。**GPUI 一行未写，先摆骨架 = 预留扩展点**，与「零无用抽象」冲突，本版删除。起步期只建 `hongguo-gpui` 单壳；等某个能力真的有了独立状态与生命周期，再按同一规则拆（参照 `gpui-ui-design` 的 GPUI Kit 规范，届时另出拆分方案）。

---

## 5. 流处理设计（按实际代码重写）

### 5.1 现状：Rust 不出帧，且不是「拉」模型

现有链路（代码即事实）：

```
选流   domain/api/stream_pick 算分 + media::capability::playback::hevc_playable() 能力过滤
取数   service/play_service/online：建 ProgressiveStream{ sparse 密文 + StreamingPlan }
       注册进 protocol/stream（此刻起可应答 Range），后台 fill 顺序填充（三级调度）
解密   domain/mp4/streaming::StreamingPlan 惰性解密（按明文 Range 反推依赖密文区间）
供给   protocol/register.rs 的异步 handler 按 HTTP Range 应答（渐进 4MB 窗口 / 30s 超时）
解码   ★ WebView 自己 demux + decode——Rust 侧到此为止
兜底   service/play_service/compat_play：解不出 → pipeline::transcode 转 H.264，改播 local://
```

三个必须尊重的形态事实：

1. **推 + 缓存 + Range 应答，不是 `AsyncRead` 拉流**。`ProgressiveStream` 是「稀疏密文缓冲 + 解密计划 + 读者需求登记」，由协议 handler 拉出来应答；数据是后台任务推进去的。
2. **键是 `(vid, 档位)`**，一集多档各一份，切档不重下、不碰正在播的那份。
3. **Rust 只到「明文 MP4 字节」**；解码/渲染在 WebView。所以 GPUI 要出帧，是**新增一条末端**，不是复用现有末端。

> v1 的 `StreamSource::open_range -> Box<dyn AsyncRead>` / `StreamHandle::read_chunk(&mut [u8])`（同步阻塞）描述的都不是上面这东西——属凭空设计。

### 5.2 契约切点：切在「共用上游」，不切在投递末端

```
CDN 分片 → CENC 解密 → 明文 MP4 + 样本表/解密计划     ← 共用上游（收进 hongguo-stream / hongguo-mp4）
        ├─ Tauri：按 Range 回明文字节 → WebView 解      ← 现状，投递末端 A
        └─ GPUI ：样本表 → 平台解码 → GPU 纹理          ← 全新末端 B（需 spike）
```

**接口草案（以实际类型为底，随 §5.3 spike 定稿；本版只钉边界与归属）**：

```rust
/// 流源（闭集，enum 分发——对齐 media/backend.rs 的 Backend 范式，不用 trait object）
pub enum StreamSource {
    /// 在线加密流：CDN 地址 + 代理 + 密钥材料（解密计划在内部构建）
    OnlineEncrypted { vid: String, definition: u32, cdn_url: String, proxy: ProxyConfig },
    /// 已下载的本地成品
    LocalFile(PathBuf),
    /// HEIC 封面
    Cover(String),
}

/// 已注册、可按明文 Range 供数的流（统一今天的「渐进 ProgressiveStream」与「整集缓存」）
pub trait PlainStream: Send + Sync {
    fn plain_len(&self) -> u64;
    /// 按明文区间供数；依赖的密文未就绪时登记 ReaderDemand 并返回 Pending
    fn read_range(&self, spec: RangeSpec) -> PlainRange;
    fn demand(&self) -> &ReaderDemand;
}
```

- 契约引用 `hongguo-mp4` 的 `StreamingPlan` / `SparseBuffer`，**不引用 media 的编解码实现**——这是「契约与实现分离」的落点。
- 归属：源实现（`OnlineEncryptedSource` / `LocalFileSource` / `CoverSource`）落 `hongguo-protocol`；编解码实现落 media / media-platform。

### 5.3 GPUI 帧流是新增末端（前置 spike）

- GPUI 无内建视频元素（GPUI Kit 规范不含 video/texture/surface）。要出帧，需：平台解码（复用 `media/platform` 的 VT/MF）→ GPU 纹理（macOS `CVMetalTextureCache`、Windows `IMFSample`→D3D11）→ 与 GPUI 渲染器集成。
- **无成熟先例，风险最高**。落地前必须先做技术验证（spike）：macOS 一个 `CVPixelBuffer → Metal → GPUI 上屏`的最小闭环。
- 清晰度口径：帧流的价值是**免转码**——平台解得出 HEVC 就直出（原画）；解不出可退回软解出帧，仍不重编码；而 Tauri 侧解不出只能转码（有损）。这是「帧流更清晰」的唯一成立场景，本质是不转码，与「字节/帧」无关。
- **spike 结论决定 §5.2 草案是否要引入帧侧抽象**——先验证再定契约。

---

## 6. 目录结构

```
hongguo-desktop/
├── Cargo.toml                 # workspace 根（新增：members + [patch.crates-io] + [profile.*]）
├── Cargo.lock
├── rust-toolchain.toml
├── Makefile
├── .cargo/config.toml         # 从 src-tauri/.cargo 迁根（三目标 rustflags 软解提速）
├── package.json
├── README.md / README.en.md / AGENTS.md / LICENSE / NOTICE / CHANGELOG.md
│
├── crates/
│   ├── hongguo-common/
│   ├── hongguo-signer/
│   ├── hongguo-mp4/
│   ├── hongguo-api/
│   ├── hongguo-store/
│   ├── hongguo-stream/        # 新增薄契约
│   ├── hongguo-media/
│   ├── hongguo-media-platform/
│   ├── hongguo-protocol/
│   ├── hongguo-service/
│   └── hongguo-tauri/
│
├── src/                       # 现有 React 前端（位置不动，见 §14）
├── src-tauri/                 # 过渡期：tauri.conf.json / capabilities / icons / gen
│
├── docs/
│   ├── architecture-refactor.md
│   ├── stream-contract.md     # 流契约（§5 定稿后落）
│   ├── design-identity.md     # 产品视觉决策
│   └── …
│
├── scripts/  captures/  vendor/  .github/  .githooks/
└── …
```

> **相对 v1 的删改**：去掉 `crates/hongguo-media-gpu/`、9 个 `hongguo-gpui-*`、`apps/web/`、`apps/gpui/`、`pnpm-workspace.yaml`。前端留在 `src/`（搬家与目标正交，见 §14）。`vendor/` 从 `src-tauri/vendor` 迁根（patch 路径须与根 manifest 对齐，见 §9）。

---

## 7. 两端共存策略

| 维度 | Tauri + React | GPUI |
| --- | --- | --- |
| 入口 | `hongguo-tauri` | `hongguo-gpui` |
| UI 技术 | React 19 + TanStack + Zustand + shadcn | GPUI + GPUI Kit / gpui-component |
| 业务调用 | IPC → commands → service | 壳直接依赖 service |
| 流呈现 | 自定义协议 Range → WebView 解码 | 消费样本流 → 平台解码 → 纹理（§5.3） |
| 状态持久化 | 同一 `hongguo-store` | 同一 `hongguo-store` |
| 视觉身份 | `src/theme` | `hongguo-gpui` 内 theme（独立时再抽 crate） |
| 生命周期 | 可长期保留 | 并行演进，互不影响 |

> **双端长期共存是本方案的既定前提**（本方案即双端改造方案），不作为待决项；§12 已标 ✅。

---

## 8. 迁移顺序（保持可运行）

> **执行顺序（已定）**：
> - **阶段 0 · 地基**：`src-tauri` 原地成为 workspace 唯一 member（§9 机械耦合清单迁根），**零行为变更**，`make dev/test/lint` 全绿即通过。
> - **阶段 1 · 清前置障碍**（**已完成 2026-10-11**）：fixture 迁出生产源码树（消 `signer→domain` 反向依赖）、生成器归位 `scripts/`、元测试改目录自动发现、下载选集器重命名——这些不清，阶段 2 会把它们带进 crate 结构。
> - **阶段 2 · 自底向上拆 crate**：下列 1–6 步，**优先级按实测耦合**（§0）。
> - **阶段 3 · 前端并行**（与后端拆分正交，随时插入）：评论 / 剧评组件的双实现收敛（唯一的真实逻辑重复）、`components/` 两级散装归位、前端入参 zod 化。
> - **会被拆分「吞掉」的结构项**（**并入对应 crate 一起做**，不单独排期——避免同一文件搬两遍）：`domain/api` 的 `mod` + `payload/parse` 拆分（随 `hongguo-api`）、前后端域粒度对齐 / `rank` 域拆分（随 `hongguo-api` · `hongguo-tauri`）、请求体与事件载荷强类型 struct（随对应 crate）。

严格按依赖自底向上，每步保证 Tauri + React 路径可编译可运行。**优先级按实测耦合**（§0）：

1. **抽枢纽底层**：`hongguo-common` → `hongguo-signer` → `hongguo-mp4` → `hongguo-api`
2. **抽持久化**：`hongguo-store`
3. **立流契约再抽媒体**：`hongguo-stream`（先落 §5.2 边界）→ `hongguo-media` → `hongguo-media-platform`
4. **抽协议与业务**：`hongguo-protocol` → `hongguo-service`（service 改为只依赖 stream 契约）
5. **薄化 Tauri**：`commands/app_state/bootstrap/diagnostics` 迁入 `hongguo-tauri`，`src-tauri` 退化为纯配置与入口
6. **清理**：删过渡代码、回收 vendor 补丁、补齐文档与测试

每一步用 `path` 依赖替换，避免一次性大爆炸。**GPUI 不在本序列内**——它是末端新增，待 §5.3 spike 有结论后单独立项。

---

## 9. workspace 化的机械耦合清单（v1 缺失，照它落地会成批炸）

| 项 | 现状位置 | 拆分后必须做 |
| --- | --- | --- |
| `[patch.crates-io]`（4 个 vendor 补丁：muxide / rusty_alloc×2 / rusty_h264-common / tauri-plugin-webdriver） | `src-tauri/Cargo.toml` | 迁**根** manifest，path 相对根重算 |
| `[profile.release]`（**含「禁 panic=abort」铁律注释**）+ `[profile.dev.package.rusty_h265-accel]` | 同上 | 迁根；铁律注释随行，勿留在 `src-tauri` |
| `.cargo/config.toml`（三目标 rustflags） | `src-tauri/.cargo/` | 迁根；确认对全 workspace `[target.*]` 生效 |
| `.gitignore`（`src-tauri/target/`、`src-tauri/gen/schemas/`） | 根 | 更新为新 target 目录 |
| `vendor/` | `src-tauri/vendor/` | 迁根，patch 路径同步 |
| Makefile（dev/build/test/lint/release… 全部 target） | 根 | 路径改指 workspace |
| `tauri.conf.json`（`frontendDist: ../dist`、`beforeBuildCommand: pnpm build`、`devUrl: 1420`） | `src-tauri/` | 后端 crate 迁出后相对路径重算 |
| CI workflow / `.githooks`（pre-commit 三目标 clippy + cargo test + eslint） | `.github/` `.githooks/` | 路径与 crate 名同步 |
| **元测试清单** `commands/mod.rs`、`service/mod.rs` 的 `include_str!` 列表 | 各模块 | **已改目录自动发现**（`utils::source_guard`）；迁 crate 时随模块走 |
| **pub 面收敛**：`lib.rs` 现「模块全私有」（红线 7） | `lib.rs` | crate 边界必须 pub；需为每个 crate 显式定义对外 API，禁止 `pub use *` 泄底 |
| 双端契约测试 / 黄金向量（签名、抓包比对） | `signer`、`domain/api` | 随对应 crate 迁移，勿丢守门人 |

---

## 10. 跨端契约约定（强制）

- 传输字段一律 **camelCase**（Rust `#[serde(rename_all = "camelCase")]`，禁手写映射）
- 整型 ID 一律 **字符串**
- 时间一律 **UTC 毫秒时间戳**；非毫秒单位字段名带单位
- 布尔只传 `true` / `false`；枚举序列化为 **string 字面量**
- 空列表返 `[]`，`null` 只表示「明确没有」（`Option<T>` ↔ `T | null`）
- IPC 命令名 **snake_case**，事件名 **kebab-case**；两端常量镜像同 commit 改
- **事件 / IPC 载荷一律强类型 struct**，禁 `serde_json::json!` 裸拼；struct 与前端 zod schema 两端同 commit 改
- 前端禁止 `any`，响应必须过 zod schema 校验

> 待清：`service/play_service/online.rs` 的进度载荷（`ProgressReporter::report`/`done`）仍是 `json!` 裸拼，随 crate 迁移一并 struct 化。

---

## 11. 风险与验收

| 风险 | 应对 |
| --- | --- |
| **拆分后编译未必变快**（跨 crate 改动级联重编） | **先测基线**：整包 clean build 耗时 + 单模块增量改动的重编译范围；验收指标随拆分逐 crate 对比。v1 无基线、无指标，本版补 |
| 契约 crate 形态定错（尤其帧侧） | §5.3 spike 先行；契约只钉「共用上游」，下游分叉不进契约 |
| 平台硬编 FFI 迁移回归 | VT/MF 有真机验收（macOS CI 跑 VideoToolbox；Windows 本机），拆 `media-platform` 后同样真跑 |
| 签名 / API 回归 | 黄金向量测试与抓包比对测试随 crate 迁移，逐一确认 IMAGE 后仍绿 |
| 两端视觉不一致 | 维护 `docs/design-identity.md`，语义 token 对齐 |

---

## 12. 决策记录

> **状态栏含义**：✅ 已定 / ⏳ 待产品确认 / ❓ 待技术验证（spike）

| 决策 | 选择 | 状态 |
| --- | --- | --- |
| 后端分 workspace | 是（自底向上，保持可运行） | ✅ 技术方向 |
| 流处理契约 | 切在「共用上游」（解密后明文 + 样本表），非投递末端 | ✅ 已按代码校正 |
| 闭集分发 | enum（对齐 `media/backend.rs`），非 trait object | ✅ |
| GPUI 组织 | 起步单壳，有代码再拆 feature | ✅ |
| 主题归属 | 先在 GPUI 壳内，独立成 crate 待有必要 | ✅ |
| 两端长期共存 | 是（本方案前提，非待决项） | ✅ 已定 |
| **GPUI 帧流 → 纹理集成** | —— | ❓ 待 spike（§5.3） |
| 遗留结构项的执行时机 | 前置项已完成；前端项并行；重叠项（`domain/api` 拆分 / `rank` 域对齐 / 载荷 struct 化）并入对应 crate（§8） | ✅ 已定 |

---

## 13. 与既有文档的关系

| 文档 | 关系 |
| --- | --- |
| `docs/structure-improvement-plan.md` | **已删**（2026-10-11）：剩余项已并入本方案 §8；`git log --follow -- docs/structure-improvement-plan.md` 可找回 |
| `docs/refactor-plan.md` | **已删**（git 历史可找回，`git log --follow`）。前期前后端重构计划 P0~P6 已落地 |
| `docs/platform-transcode-progress.md` | **已删**（提交 `dd982a2` 清理转码档案）。转码决策档案见 git 历史 |
| `docs/hongguo-api-endpoints.md` | 对接官方接口的唯一事实源；改 `domain/api` 前必读 |
| `AGENTS.md` | 项目硬红线与开发约定，迁移过程必须遵守 |

---

## 14. 不作（本方案明确不碰）

- **不建 `hongguo-media-gpu`**、不预拆 GPUI feature crate——有代码再立。
- **不搬前端**（不加 `apps/web/`、不建 pnpm workspace）——GPUI 是 Rust，用不上 JS workspace；搬家与目标正交，属独立决策。
- **不动 `signer/` 写法**（JS 1:1 移植，红线 9）；不动 `media/platform` 的 FFI 结构；不动 `player/` 渲染结构。
- **不写数据兼容回退分支**（产品未上线）。
- **不在本方案内动代码**——大规模结构调整先出计划、经确认再动；确认后按 §8 分 PR 推进（纯移动/拆分与行为修改分开提交）。

---

## 15. 后续可交付物

- 根 `Cargo.toml` workspace members + 依赖关系图（含 §9 机械耦合清单的核对）
- `docs/stream-contract.md`：§5.2 契约定稿（依赖 §5.3 spike）
- `hongguo-gpui` 单壳 + 播放 spike（`CVPixelBuffer → Metal → GPUI 上屏`）
- 编译基线报告（clean build 耗时 + 增量重编译范围）
- 迁移 Checklist（按 PR 粒度）

---

*本文档随架构演进持续更新。重大变更需同步更新依赖图、迁移顺序与决策状态。*

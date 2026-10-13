# 红果桌面客户端 · 架构重构方案

> 版本：2026-10-13（rev4：二期——平台四 crate 拆分 + service 去 Tauri 化，见 §16）
> 状态：**重构完成**——阶段 0~3 与二期全部落地（16 个成员 crate + 双壳）；`hongguo-gpui` 起步单壳已交付（操作与 Tauri 客户端一致，播放引擎已接入——进度与遗留见 [`gpui-shell-progress.md`](./gpui-shell-progress.md)）
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

## 进度看板（随每步更新）

> 口径：行数用 `Get-ChildItem -Recurse -Include *.rs` 实测；包数用 `cargo metadata`。
> **最后更新 2026-10-13（GPUI 播放接入与颜色根因定案后）**；每完成一步在 `docs/build-baseline.md` 复测编译数字。

| 步骤 | 内容 | 状态 | 实测 |
| --- | --- | --- | --- |
| 阶段 0 | 地基：`src-tauri` 原地成为 workspace 唯一 member | ✅ 2026-10-11 | 包数 721，零行为变更 |
| 阶段 1 | 清前置障碍：fixture 迁出 / 生成器归位 / 元测试目录自动发现 | ✅ 2026-10-11 | |
| 1 | 枢纽底层：`common` → `signer` → `mp4` → `api` | ✅ 2026-10-11 | 四个 crate 全部落地 |
| 2 | 持久化：`hongguo-store` | ✅ 2026-10-11 | 13 文件 / 1,820 行；包 725 → 726 |
| 3a | 流契约：`hongguo-stream` | ✅ 2026-10-11 | 4 文件 / 1,459 行；包 726 → 727 |
| 3b | 媒体核心：`hongguo-media` | ✅ 2026-10-11 | 16 文件 / 4,477 行；包 727 → 728 |
| 3c | 平台层：`hongguo-media-platform` | ✅ 2026-10-12 | 12 文件 / 4,420 行（含 lib.rs 与 test-support 门控）；包 728 → 729 |
| 4 | 协议与业务：`hongguo-protocol` → `hongguo-service` | ✅ 2026-10-12 | protocol 5 文件（无 tauri 依赖）；service 41 文件（含 AppState 随迁）；包 729 → 731 |
| 5 | 薄化 Tauri：`commands` / `app_state` / `bootstrap` / `diagnostics` → `hongguo-tauri` | ✅ 2026-10-12 | tauri 壳装配 31 文件 / 4,568 行迁出；壳降至 2 文件 / 22 行；包 731 → 729（壳瘦身净减 2 包） |
| 6 | 清理：删过渡代码、回收 vendor 补丁、补文档与测试 | ✅ 2026-10-12 | 事件载荷 struct 化（§10 挂账清零）；vendor 补丁上游未修，保留 |
| 阶段 3 | 前端并行：评论/剧评双实现收敛、`components/` 归位、入参 zod 化 | ✅ 2026-10-12 | 回复线程收敛共享 reply-thread（视觉零变化）；6 散装组件归位；入参 zod 化经核已是既定设计（registry 编译期约束） |
| GPUI 壳 | `hongguo-gpui` 起步单壳（§15 交付物） | ✅ 2026-10-12 | 窗口 + 双壳同源装配 + 推荐流；`make gpui`；帧流 spike 另行立项 |
| GPUI 播放 | §5.3 帧流闭环 → 引擎接入：解码门面 → `hongguo-playback` → 首页沉浸流 / 排行榜 / 封面 | ✅ 2026-10-13 | 本机可播（HEVC+AAC、倍速/seek/音量、连播、榜单封面）；通道序 RGBA→BGRA 真因与封面键修复见 `docs/gpui-shell-progress.md` |
| 二期·平台拆分 | `hongguo-media-platform` 一拆四：`-api` 契约 / `-macos`/`-windows` 实现 / 门面（含 ffmpeg + capability） | ✅ 2026-10-13 | 纯结构移动；protocol 封面去平台分支、gpui 帧类型改依赖契约；三目标 clippy + 全量测试绿 |
| 二期·去 Tauri | service 摘除 tauri 依赖：共用任务运行时 `runtime` + 业务事件总线 `events`；Tauri 事件转发器（原名原载荷） | ✅ 2026-10-13 | 源码静态闸；事件契约零漂移（`tests/wire_contract.rs` 快照 + 映射表测试） |
| 二期·编排下沉 | 下载/登录/合并/兼容转码编排全归 service；GPUI 下载调度与登录轮询复刻删除，接入服务事件泵 | ✅ 2026-10-13 | 停止语义统一真实取消（Tauri 修复）+ 重试立即调度；前端零改动 |

**当前体量**（2026-10-13 二期后实测）：

| 位置 | 文件 | 行数 |
| --- | --- | --- |
| 壳 `src-tauri/src`（纯配置 + 入口） | 2 | 22 |
| `crates/`（16 个成员） | 304 | 57,856 |

对照 §0 的重构前快照：壳是 **198 文件 / 41,890 行**。逐 crate 明细与依赖方向见 §4 与 §16。

---

## 0. 重构前基线与结论（实测 2026-10-11，重构启动时的快照——不是「现状」，现状见进度看板）

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
│   hongguo-video（MP4/CENC） · hongguo-api ← hongguo-signer · common │
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
| `hongguo-common` | 共用 serde 模型（`model/`）、`AppError`（`error.rs`，**不含 tauri 变体**）、工具（`utils/`）、跨端常量（`consts/`） | serde / serde_json / thiserror / uuid / chrono / dirs |
| `hongguo-signer` | `signer/`——五头签名（x-gorgon 等）+ 设备注册原语；**自带 TT 哈希/加密（helios/medusa/tt_hash）**，JS 1:1 移植禁改写法 | base64 / rand / serde / md-5 / sm3 / hex / chrono（**不依赖 common 与 tauri**） |
| `hongguo-video`（2026-10-14 更名，原名 `hongguo-mp4`） | `domain/mp4/` + `domain/crypto/`——MP4 盒/样本表/稀疏缓冲/流式计划 + CENC-AES-CTR 解密 + 密钥派生 | common + aes / ctr / base64 / log / parking_lot |
| `hongguo-api` | `domain/api/`——官方 API 客户端（登录/推荐/搜索/详情/流地址/弹幕/互动等，14 个域） | signer + common + aes / chrono / flate2 / futures-util / hex / log / parking_lot / rand / reqwest / serde / serde_json / tokio |
| `hongguo-store` | `store/`——Turso 持久化、entity、迁移、恢复 | common + signer（`DeviceProfile` 是库里的一张表） |

> **命名修正**：v1 叫 `hongguo-crypto`，但它的内容（CENC + MP4 解析）主体是容器解析，`domain/crypto/` 只是 CENC 原语。改叫 `hongguo-mp4`（2026-10-14 再更名 `hongguo-video`，去名中数字）。**`signer` 的 TT 哈希/加密与 CENC 是两回事，不合并**。

### 4.2 流契约、媒体与协议

| Crate | 职责 | 依赖 | 状态 |
| --- | --- | --- | --- |
| `hongguo-stream` | 流数据面：`ProgressiveStream` / `StreamCache` / `ReaderDemand` / `serve`（Range 应答 + 窗口策略）+ `range.rs`（`RangeSpec` / `ProtocolResponse`）+ 进程级缓存 `cache()` | mp4 + common | **已落地 2026-10-11** |
| `hongguo-media` | `media/` 纯 Rust 编解码——backend / demux / hevc / transcode / remux / codec_probe / disk_space / heif | mp4 + common | **已落地 2026-10-11** |
| `hongguo-media-platform` | `media/platform`（MF / VT / WIC）+ `media/ffmpeg` + `media/capability`——平台硬编、ffmpeg 执行与能力探测 | media + mp4 + common | **已落地 2026-10-12** |
| `hongguo-protocol` | `protocol/`——自定义 URI 逻辑（`hongguo-local/stream/cover`）+ 路由。**注意命名：这是 URI 协议，不是 HTTP/传输协议** | stream + media + media-platform + common | 待第 4 条 |

> **依赖修正（2026-10-11 实测，v1/rev3 原表有误）**：
> - **`capability/` 必须随 platform、ffmpeg 一起进 `hongguo-media-platform`**。实测 `media/platform/{vt,mf}` 调 `media::{demux,hevc,transcode}`（19 处），而 `media/capability` 反手调 `media::{platform,ffmpeg}`（14 处）——照原表把 capability 留在 `hongguo-media` 里会成环。
> - **`hongguo-protocol` 的依赖不止 stream + mp4 + common**：`protocol/cover.rs` 还要 `hongguo_media::heif` / `media::platform::wic` / `media::ffmpeg`（封面 HEIC→JPEG 的三级阶梯）。（二期后 WIC 一级经门面 `platform::cover` 收口，protocol 里已无平台分支，见 §16.1。）
> - **`hongguo-stream` 同时收 `range.rs` 与进程级 `cache()`**：`RangeSpec` / `ProtocolResponse` 是流应答的线格式；`cache()` 是填充侧与应答侧唯一的共享句柄——不搬它，`serve()` 只能回头调 `service::play_service::online::cache()`，反向边消不掉。原表把 stream 描述为「只放共用数据类型」，偏窄。
> - **`hongguo-media-platform` 实测依赖还含 `mp4` 与 `common`**（2026-10-12，3c 落地时核对）：`platform/{vt,mf}` 直接消费 `hongguo_mp4::sample_table::TrackInfo`（hvcC 参数集 → Annex-B 喂解码器），门面与 ffmpeg / capability 消费 `AppError` / `AppResult`。原表只写「media」。

> **不设 `hongguo-media-gpu`**：GPU 路径尚不存在，有真需求再立 crate（§14）。
> **`service` 不再依赖 `protocol`**（**已落地 2026-10-11**）：`service/play_service/online` 改为 `use hongguo_stream::{ProgressiveStream, StreamCache, cache}`，`protocol::register` 改为 `hongguo_stream::serve`——两侧都只往下依赖 `hongguo-stream`，那条「协议层回头找服务层要缓存」的反向边没了。

### 4.3 业务与壳

| Crate | 职责（对应现有代码） | 依赖 |
| --- | --- | --- |
| `hongguo-service` | `service/`——download / play / merge / series / settings / storage / transcode 编排 | api + media + stream + store + common |
| `hongguo-tauri` | `commands/` + `app_state.rs` + `bootstrap/` + `diagnostics.rs` + 入口（`lib.rs`）；IPC、协议注册、装配 | service + protocol + stream + tauri |
| `hongguo-gpui` | GPUI 应用壳：初始化、开窗口、（后续）组合 feature | service + stream + media | **起步单壳已落地 2026-10-12**（窗口 + 双壳同源装配 + 推荐流；tokio 桥见 §12） |

### 4.4 GPUI Feature 拆分（**降级为有代码再谈**）

v1 预列了 9 个 `hongguo-gpui-*` crate（theme/player/discover/series/download/library/settings/shared）。**GPUI 一行未写，先摆骨架 = 预留扩展点**，与「零无用抽象」冲突，本版删除。起步期只建 `hongguo-gpui` 单壳；等某个能力真的有了独立状态与生命周期，再按同一规则拆（参照 `gpui-ui-design` 的 GPUI Kit 规范，届时另出拆分方案）。

---

## 5. 流处理设计（按实际代码重写）

### 5.1 现状：Rust 不出帧，且不是「拉」模型

现有链路（代码即事实）：

```
选流   domain/api/stream_pick 算分 + media::capability::playback::hevc_playable() 能力过滤
取数   service/play_service/online：建 ProgressiveStream{ sparse 密文 + StreamingPlan }
       注册进 hongguo-stream（此刻起可应答 Range），后台 fill 顺序填充（三级调度）
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
CDN 分片 → CENC 解密 → 明文 MP4 + 样本表/解密计划     ← 共用上游（收进 hongguo-stream / hongguo-video）
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

- 契约引用 `hongguo-video` 的 `StreamingPlan` / `SparseBuffer`，**不引用 media 的编解码实现**——这是「契约与实现分离」的落点。
- 归属：源实现（`OnlineEncryptedSource` / `LocalFileSource` / `CoverSource`）落 `hongguo-protocol`；编解码实现落 media / media-platform。

> **落地与草案的差异（2026-10-11，3a 实际做完）**：`hongguo-stream` 只做「搬」，**没有引入**上面草案里的 `StreamSource` enum 与 `PlainStream` trait。现有两种形态——`ProgressiveStream`（渐进，稀疏密文 + 解密计划）与 `StreamCache`（整集，明文原子写入）——已覆盖今天全部供数路径，且都是具体类型、无动态分发；在第二套投递末端（GPUI 帧流，§5.3）落地前引入 trait 就是「为将来铺路」，与「零无用抽象」冲突。`read_range(&self, spec: RangeSpec)` 的语义由 `serve(vid, definition, range_header)` + `StreamCache` 的区间读承担。
> 实测结论：**先搬后抽象**——等 §5.3 spike 有结论，再决定要不要真的引入这层抽象。

### 5.3 GPUI 帧流是新增末端（前置 spike）

- GPUI 无内建视频元素（GPUI Kit 规范不含 video/texture/surface）。要出帧，需：平台解码（复用 `hongguo-media-platform` 的 VT/MF）→ GPU 纹理（macOS `CVMetalTextureCache`、Windows `IMFSample`→D3D11）→ 与 GPUI 渲染器集成。
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
│   ├── hongguo-video/
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
> - **阶段 0 · 地基**（**已完成 2026-10-11**）：`src-tauri` 原地成为 workspace 唯一 member（§9 机械耦合清单迁根）——根 `Cargo.toml`（`resolver="3"` + members + `[patch]` + `[profile]`）、`.cargo/config.toml`、`vendor/`、`Cargo.lock`、`target/` 均落仓库根；**零行为变更**（迁移前后 `cargo metadata` 721 包逐包比对一致、patch 仍命中 `vendor/`），`cargo test` 651 全绿 + clippy 三目标零警告 + 前端门禁全绿。
> - **阶段 1 · 清前置障碍**（**已完成 2026-10-11**）：fixture 迁出生产源码树（消 `signer→domain` 反向依赖）、生成器归位 `scripts/`、元测试改目录自动发现、下载选集器重命名——这些不清，阶段 2 会把它们带进 crate 结构。
> - **阶段 2 · 自底向上拆 crate**：下列 1–6 步，**优先级按实测耦合**（§0）。
> - **阶段 3 · 前端并行**（与后端拆分正交，随时插入）：评论 / 剧评组件的双实现收敛（唯一的真实逻辑重复）、`components/` 两级散装归位、前端入参 zod 化。
> - **会被拆分「吞掉」的结构项**（**并入对应 crate 一起做**，不单独排期——避免同一文件搬两遍）：`domain/api` 的 `mod` + `payload/parse` 拆分（随 `hongguo-api`）、前后端域粒度对齐 / `rank` 域拆分（随 `hongguo-api` · `hongguo-tauri`）、请求体与事件载荷强类型 struct（随对应 crate）。

严格按依赖自底向上，每步保证 Tauri + React 路径可编译可运行。**优先级按实测耦合**（§0）：

1. **抽枢纽底层**（**全部完成 2026-10-11**）：`hongguo-common` → `hongguo-signer` → `hongguo-mp4` → `hongguo-api`
2. **抽持久化**（**已完成 2026-10-11**）：`hongguo-store`——13 文件 / 1,820 行迁出，`cargo metadata` 725 → 726 包（仅新增本 crate，无版本漂移）。两处解耦：`persist_tasks` 从 store 移入 `service::download_service`（去掉 store → `AppState` 反向依赖）；`paths::ScopedDataDir` 由 `#[cfg(test)]` 改 feature `test-support` 门控（跨 crate 后 `#[cfg(test)]` 对下游测试不可见）
3. **立流契约再抽媒体**：**3a 已完成 2026-10-11**——`hongguo-stream` 落地（`protocol/stream/*` + `protocol/range.rs` 共 4 文件 / 1,459 行迁出，进程级 `cache()` 单例随迁）；`cargo metadata` 726 → 727 包。**3b 已完成 2026-10-11**——`hongguo-media` 落地（16 文件 / 4,477 行迁出，含 `tests/fixtures/heic-cover-sample.heic`）；`cargo metadata` 727 → 728 包。两处结构性决定：跨层的端到端用例 `remux/e2e_tests.rs` 归壳的 `src/testing/media_pipeline_e2e.rs`（它同时要平台硬编、ffmpeg 与服务层）；`worker::temp_path_for` 上移 `hongguo_common::utils::path`（下载 / 转码 / ffmpeg 三条链路的共同落盘约定，留在 service 会让 media 反向依赖）。**3c 已完成 2026-10-12**——`hongguo-media-platform` 落地（12 文件 / 4,420 行迁出，含 `capability/` 与新增 `lib.rs`；`cargo metadata` 728 → 729 包）。一处结构性决定：`transcode_h264_for_tests` 测试通道（及 mf 软件编码器路径）由 `#[cfg(test)]` 改 `any(test, feature = "test-support")` 门控——跨 crate 后 `pub(crate)` 对壳的 e2e 够不着，经壳的 dev-dependencies feature 开启（对齐 hongguo-mp4 `fixtures` / hongguo-store `ScopedDataDir` 先例）。增量复测：平台层文件 build 18.6 s → 21.5 s，壳文件持平（16 s），见 `docs/build-baseline.md`。编译基线见 `docs/build-baseline.md`（3b 复测：改 media 文件的增量 build 18.6 s → 26 s，是拆分的真实代价，换来的是边界而非秒数）
4. **抽协议与业务**：`hongguo-protocol` → `hongguo-service`（service 改为只依赖 stream 契约）
5. **薄化 Tauri**（**已完成 2026-10-12**）：`commands/bootstrap/diagnostics/protocol 胶水/testing` 迁入 `hongguo-tauri`（31 文件 / 4,568 行），`src-tauri` 降至 2 文件 / 22 行（纯配置 + 入口；`generate_context!` 必须与 tauri.conf.json 同 crate）。`AppState` 实测为纯业务状态，随 service 走（修正本条原列清单）。两处 tauri-build 机械约束记入 §12
6. **清理**（**已完成 2026-10-12**）：事件载荷 `json!` 裸拼全部 struct 化（§10 挂账清零，载荷形状零变化）；vendor 4 个补丁上游均未发修复版，保留；过渡注释清零

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
| **元测试清单** `commands/mod.rs`、`service/mod.rs` 的 `include_str!` 列表 | 各模块 | **已改目录自动发现**（`testing::source_guard`）；迁 crate 时随模块走 |
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
| `hongguo-stream` 的物理边界 | 全量数据面（`ProgressiveStream`/`StreamCache`/`ReaderDemand`/`serve`）+ `range.rs` + 进程级 `cache()` 单例一起进；**不引入** `PlainStream` trait / `StreamSource` enum（先搬后抽象） | ✅ 已定（3a，2026-10-11） |
| `media/capability` 归属 | 随 `platform` + `ffmpeg` 进 `hongguo-media-platform`（留 `media` 会成环） | ✅ 已定（实测，2026-10-11） |
| 编译基线 | 已采：clean build 208.8 s，单文件增量 check 1.4 s / build ≈18 s（链接主导）；3a 后持平，3b 后改 media 的 build 升到 26 s | ✅ 已采（`docs/build-baseline.md`） |
| 跨层 e2e 用例的归属（3b） | `remux/e2e_tests.rs` 同时依赖媒体核心 + 平台硬编 + ffmpeg + 服务层，任何单一 crate 都装不下 → 迁到壳的 `src/testing/media_pipeline_e2e.rs`（Cargo 集成测试目录够不着私有模块，只能是壳内 `#[cfg(test)]` 模块） | ✅ 已定（3b） |
| `worker::temp_path_for` 的归属（3b） | 上移 `hongguo_common::utils::path`——下载/转码/ffmpeg 三条链路共用的落盘命名约定；留在 service 会让 `hongguo-media` 反向依赖服务层 | ✅ 已定（3b） |
| `transcode_h264_for_tests` 的可见性（3c） | 跨 crate 后 `pub(crate)` 对壳 e2e 够不着 → 改 `pub` + `any(test, feature = "test-support")` 门控（连带 mf 的软件编码器通道同步门控）；生产构建不带 feature 不编译 | ✅ 已定（3c） |
| `AppState` 的归属（第 4 条实测修正） | 实测为纯业务状态句柄（store/settings/device/队列，无 UI 语义）→ 随 `hongguo-service` 走而非原定的 `hongguo-tauri`；壳经 Tauri `manage` 注入不变，GPUI 壳直接构造 | ✅ 已定（第 4 条） |
| `service → protocol` 正向边（第 4 条实测） | play_service 构造 WebView 播放 URL 需要 `local_play_url`/`stream_url`——方向向下无环，保留并记入 §4.2；GPUI 走帧流不经 URL，未来投递末端重构时再议上移壳层 | ✅ 已定（第 4 条） |
| 协议 crate 的切线（第 4 条） | `hongguo-protocol` **无 tauri 依赖**：供数产出 `(状态码, 头, 体)` 元组，HTTP 组装与 Builder 注册归壳胶水（§2「协议注册归壳」）；工作池与 panic 加固随供数逻辑进 crate | ✅ 已定（第 4 条） |
| tauri-build 的直接依赖约束（第 5 条实测） | `generate_context!` 须与 tauri.conf.json 同 crate；conf 声明 assetProtocol → 壳的 tauri 依赖须带 `protocol-asset`；capabilities 的插件权限清单只从壳的**直接依赖**收集 → 六个插件在壳保留直接依赖声明（代码不经手） | ✅ 已定（第 5 条） |
| GPUI 壳的异步桥（单壳实测） | service 的 async 族假定 tokio 设施，GPUI 执行器是 smol 系 → 壳内持常驻 tokio 运行时线程，GPUI 任务经 `oneshot::Receiver` 回传（不依赖 tokio 上下文，可在 GPUI 执行器 await） | ✅ 已定（单壳） |
| 双壳设备引导（单壳） | `adopt_sync` + 轮换元数据下沉 `hongguo_service::device`（纯 store+signer，双壳同源）；健康探针/轮换异步生命周期暂留 tauri bootstrap，GPUI 待生命周期装配时接入 | ✅ 已定（单壳） |

---

## 13. 与既有文档的关系

| 文档 | 关系 |
| --- | --- |
| `docs/structure-improvement-plan.md` | **已删**（2026-10-11）：剩余项已并入本方案 §8；`git log --follow -- docs/structure-improvement-plan.md` 可找回 |
| `docs/refactor-plan.md` | **已删**（git 历史可找回，`git log --follow`）。前期前后端重构计划 P0~P6 已落地 |
| `docs/platform-transcode-progress.md` | **已删**（提交 `dd982a2` 清理转码档案）。转码决策档案见 git 历史 |
| `docs/hongguo-api-endpoints.md` | 对接官方接口的唯一事实源；改 `domain/api` 前必读 |
| `docs/build-baseline.md` | 编译基线：拆分前实测 + 各步复测口径（§11 的对照依据） |
| `AGENTS.md` | 项目硬红线与开发约定，迁移过程必须遵守 |

---

## 14. 不作（本方案明确不碰）

- **不建 `hongguo-media-gpu`**、不预拆 GPUI feature crate——有代码再立。
- **不搬前端**（不加 `apps/web/`、不建 pnpm workspace）——GPUI 是 Rust，用不上 JS workspace；搬家与目标正交，属独立决策。
- **不动 `signer/` 写法**（JS 1:1 移植，红线 9）；不动 `media-platform` 的 FFI 结构；不动 `player/` 渲染结构。
- **不写数据兼容回退分支**（产品未上线）。
- **不在本方案内动代码**——大规模结构调整先出计划、经确认再动；确认后按 §8 分 PR 推进（纯移动/拆分与行为修改分开提交）。

---

## 15. 后续可交付物

- ~~根 `Cargo.toml` workspace members + 依赖关系图~~ → **已交付**（阶段 0，成员清单见根 manifest）
- `docs/stream-contract.md`：§5.2 契约定稿（依赖 §5.3 spike，未做）
- ~~`hongguo-gpui` 单壳~~ → **已交付**（2026-10-12：窗口 + 双壳同源装配 + 推荐流，`make gpui`）；~~播放 spike~~ 仍 ❓（`CVPixelBuffer → Metal → GPUI 上屏` / Windows `IMFSample → D3D11`，另行立项）
- ~~编译基线报告（clean build 耗时 + 增量重编译范围）~~ → **已交付**：`docs/build-baseline.md`
- ~~迁移 Checklist（按 PR 粒度）~~ → **已交付**：本文件进度看板（逐条实测记录）
- 健康探针/轮换异步生命周期下沉 service（GPUI 壳接入同一套设备轮换）——待 GPUI 壳演进时处理

---

## 16. 二期（2026-10-13）：平台四 crate 与 service 去 Tauri 化

> 二期把「职责边界」落实到 crate 与依赖方向：平台实现彻底隔离、业务核心不再感知 UI 框架；编排只有一份。本节取代 §4.2/§4.3 中 media-platform 与 service 的旧描述。

### 16.1 平台层一拆四

```
hongguo-media-platform-api（契约：帧载体 / 解码器 trait / PlatformRequest / 码率策略）
    ├── hongguo-media-platform-macos（VideoToolbox + AudioToolbox；#![cfg(macos)]）
    └── hongguo-media-platform-windows（Media Foundation + WIC；#![cfg(windows)]）
hongguo-media-platform（门面：按编译目标 re-export 实现 + ffmpeg + capability）
```

- 门面保留 `platform::{video,audio}` 路径与全部对外函数名（下游零改动）；新增
  `platform::cover::heic_to_jpeg`（全平台同签名，非 Windows 返回 `None`）——protocol 的
  封面阶梯从此没有平台分支。
- 实现 crate 的平台专属依赖全部进各自 target 表；异目标上以 `#![cfg(target_os = …)]`
  空壳编译，`--workspace` 在三个宿主目标（Windows 宿主 + macOS 双目标交叉）全绿，
  Makefile/CI 的排除清单无需改动。
- 依赖方向：实现 → 契约；门面 → 契约 +（按目标）实现；**实现不得反向依赖门面/gpui/tauri**
  （反向即 Cargo 依赖环，编不过）。契约 crate 只依赖 common 与（macOS）core-video。
- gpui 壳只认契约（`hongguo-media-platform-api::video`）；播放引擎经门面拿具体解码器。

### 16.2 service 去 Tauri 化与全量编排下沉

- `hongguo-service::runtime`：进程级多线程 tokio 运行时（`spawn` / `spawn_blocking`），
  任意线程可派发——取代 `tauri::async_runtime` 与壳注入的 `PlayRuntime`。
- `hongguo-service::events`：`ServiceEvent` 强类型事件 + broadcast 总线（容量 1024）。
  发布点=状态变更之后；Tauri 壳 `events.rs` 原名原载荷 `app.emit` 转发，GPUI 壳
  `events.rs` 事件泵（broadcast→mpsc）接进视图。**事件名与载荷零漂移**
  （`tests/wire_contract.rs` 快照 + 映射表测试锁定，前端零改动）。
- 编排全量下沉：下载队列变更（`download_service::actions`）、登录流程
  （`login_service::flow`）、合并启动（`merge_service::start`）、兼容转码
  （`play_service::compat_play::compat_for_playback`）；命令层纯转调，GPUI 的下载
  调度与登录轮询复刻删除。
- 行为变更（有意，提交中显式标注）：调度器统一持「全局暂停位 + 每任务取消位」，
  「停止」= 真实取消（Tauri 侧从「只标记、跑完还会覆写成 completed」修正）；
  重试后立即触发调度（对齐 GPUI 现状）。其余行为保持。

### 16.3 二期决策记录

| 决策 | 选择 | 状态 |
| --- | --- | --- |
| 平台实现选择 | Cargo 目标平台依赖 + 门面 `cfg` 分派；不引运行时注册 | ✅ 2026-10-13 |
| 契约形态 | 数据与 trait 进 `-api`；无状态操作走同名普通函数（静态分派，不开 dyn） | ✅ |
| service 任务与事件 | 自带运行时 + 总线；壳只订阅适配 | ✅ |
| 事件机制 | tokio broadcast（1024）；订阅者滞后丢旧 + 日志 | ✅ |
| 「共用任务管理器」边界 | 运行时 + 各域既有取消机制；不建中央句柄登记表 | ✅ |
| 停止语义 | 统一真实取消（Tauri 行为修复） | ✅ |
| GPUI 事件消费 | 事件泵（broadcast→mpsc→视图）；下载面板保留 1s 轮询兜底 | ✅ |
| macOS 回归口径 | CI 双腿编译 + 单测；实机功能回归列发版前 checklist | ✅ |
| 服务层 UI 能力适配 | 当前无（文件选择/打开目录在命令层）；有真实需求再加窄接口 | ✅ |
| `hongguo-mp4` 更名 | `hongguo-video`（crate 名去数字；纯改名零行为变更） | ✅ 2026-10-14 |
| 入口 crate 归位 | `src-tauri` → `crates/hongguo-tauri-app`（消除根层遗留目录；CLI 经 package.json script `cd` 定位，tauri-action 用 `projectPath`，before 命令用 `cwd` 对象形式指回仓库根） | ✅ |

### 16.4 遗留

- macOS 实机功能回归（下载 / 合并 / 转码 / 播放）列发版前 checklist。
- `capability_service` 的 HEVC 扩展安装保留单点 Windows `cfg`（产品功能，不属媒体平台层）。
- 健康探针/轮换异步生命周期下沉（§15 延续）。

---

*本文档随架构演进持续更新。重大变更需同步更新依赖图、迁移顺序与决策状态。*

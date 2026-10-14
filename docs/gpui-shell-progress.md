# GPUI 壳 · 进度与遗留（hongguo-gpui）

> 最后更新 **2026-10-13**（封面与视频颜色根因定案、护栏落地、提交推送完成）。
> 范围：`crates/hongguo-gpui`（`make gpui`）与它消费的播放引擎
> （`hongguo-playback` / `hongguo-media-platform`）的壳侧对接。
> 开发机是 Windows，macOS 与 Windows 共用同一批壳代码。

## 状态一览

| 议题 | 状态 | 落点 |
| --- | --- | --- |
| 首页沉浸播放流（推荐/漫剧/真人三 tab、滚轮切剧、种子排前、起点决策） | ✅ | `src/home.rs` |
| 播放引擎接入（HEVC/AAC 解码、音频时钟、倍速/seek/音量/静音、自动连播） | ✅ 本机可播 | `hongguo-playback` + `hongguo-media-platform-windows`（MF） |
| 服务事件接线（架构二期）：登录 MFA / 下载状态随 `hongguo_service::events` 事件泵进视图 | ✅ | `src/events.rs`；下载调度与登录轮询的两份壳侧复刻已删 |
| 排行榜视图（热度榜 10 部 + 封面） | ✅ | `src/rank.rs` |
| 封面（首页切剧占位 + 榜单卡片，HEIC 三级阶梯） | ✅ 颜色/裁切均对 | `src/image_upload.rs`、`hongguo-media::bgra` |
| 数据根独立（`%APPDATA%\hongguo-gpui`，与 Tauri 互不锁库） | ✅ | `src/main.rs`、`hongguo-store::paths` |
| 图片通道序（RGBA→BGRA，本仓最贵的一次误诊） | ✅ 带护栏 | `src/image_upload.rs`、`tests/render_image_guard.rs` |
| 控制栏（进度/倍速/音量） | ⚠️ 布局待整（见遗留 2） | `src/home.rs` |
| 音画同步告警 | ⚠️ 未收口（见遗留 1） | `hongguo-playback::video/audio` |
| 键盘快捷键 /「回首页 5 分钟换一批」/ 跨季接力 | ❌ 未做 | `src/home.rs` 模块头有账 |

## 2026-10-13：封面与视频颜色（已修，两笔提交）

**症状**：封面蓝底显示成橙底；视频里人物正常肤色变蓝脸、蓝发变橙发。此前两轮
排查都落在错的层（先按 NV12 色度序倒 U/V，后归因 `ObjectFit` 采样越界），"修"
完要么不生效、要么换个面目复现。

**定案手段**（可复用）：

1. **色带探针**：往视频帧数据里叠已知色带（红→绿→蓝→白四条），截屏读回。
   屏幕上量到的是「蓝→绿→红→白」——直接证明 R 与 B 整体互换，与色度、取样
   无关。
2. **屏幕 ↔ 源数据逐通道对账**：同一张榜单封面，比较屏幕像素与我们自己解码
   出的源图均值。修前 `dR=+45.7 / dB=-52.8`（正好是互换），修后两侧相等；
   源图另用 ffmpeg 独立解码对照，确认它就是官方原色。

**根因**：gpui 的 `RenderImage` 约定帧缓冲是 **BGRA**（gpui-pre `assets.rs` 头
注释如此写，其自带图片加载路径解码后也都显式交换成 BGRA 再构造），而壳里三处
手工造帧（播放帧、首页占位封面、榜单封面）一直按 **RGBA** 递进去。三个平台上
后端都指望"输入已是 BGRA"：DX 图集 `B8G8R8A8`、Metal 图集 `BGRA8Unorm`、wgpu
默认 `Bgra8Unorm`——所以 macOS 同样中招（**视频除外**：macOS 帧流走 `surface()`
的 CVPixelBuffer 零拷贝，不经 `RenderImage`；受影响的是封面）。

**为什么"修了又坏"**：U/V 互换在数值上近似 R/B 互换，上一轮加的那层色度交换
恰好把**视频**的症状盖住（封面没被盖住，所以两处症状看起来不一致）；后来改成
"按 hvcC profile 分流"时把 Main 判成直通，补偿消失 → 视频复现蓝脸。

**修法**：RGBA→BGRA 收敛到唯一入口 `image_upload::render_image`（底层
`hongguo-media::bgra` 纯函数）；同一批把色度交换那套错位补偿删净，`nv12_of`
回到标准 NV12（U 在前）直通。另修两处独立封面毛病：

- 榜单封面与行 id 原按**名次**做键，而榜单名次会并列（本机实测两行同为「5」）
  → 第二行的封面加载被去重吞掉、渲染取到第一行那张，那一行的封面就成了别的剧；
  改按**系列 id**（缺失退回行下标）。
- 占位封面原用 `ObjectFit::Fill`（为绕那次误诊的"采样越界"），会把 400×572 的
  竖版海报横向拉宽约 1.7 倍；回到 `Cover`（裁切，与 Tauri `player-page.tsx` 的
  `object-cover` 同语义）。

**护栏**：`hongguo-media::bgra` 5 条单测（只动 R/B、自反、逐像素、尾字节不碰、
中性像素不变）+ `hongguo-gpui/tests/render_image_guard.rs`（扫源码：全壳只有
`image_upload.rs` 允许构造 `RenderImage`，且必须过交换）。本壳是 bin-only
（`[[bin]] test = false`，跑起来会开窗口），守卫只能做成集成测试。守卫验证过
会红：临时在 `home.rs` 插一句 `RenderImage::new` 即失败并点名该文件。

**提交**：`1f3ab97`（壳侧：通道序 + 封面两处 + 护栏 + 上一轮壳侧取证/稳固改动）、
`cbd9e19`（引擎侧：MF 解码器协商/样本形态 + 音频设备域重采样）。

## 遗留与下一步

1. **音画同步告警**：日志仍偶发 `[Playback][sync] 第 N 帧偏差 -0.2 ~ -5.0s
   （画面落后）`，量级比早期小但没消失；早期 vt（macOS）实测更大（-3s 级）。
   要收口得先定"偏差读数"的口径（音频可闻时钟 vs 上屏时刻），再定补偿。
2. **控制栏布局**：底部「第 N 集 / 共 M 集」与进度条同排叠字（对 Tauri
   `player-page.tsx` 的行序整一次）。
3. **切到排行榜后仍在出声**：视图切走后播放引擎继续跑（Tauri 那边是否同行为待核）。
4. **macOS 眼验**：Windows 上已实证；mac 上需看一次封面（切剧占位 + 榜单卡片）
   是否与官方海报同色。同一处共享代码，预期已好。
5. **取证脚本**：抓窗、屏幕↔源图对账、拼对照图那套一次性脚本目前只在本机
   `%TEMP%\gpui-grab\`，未入库（要常驻就搬进仓库工具目录）。
6. **未做的行为面**：键盘快捷键（空格/方向键）、「回首页 5 分钟换一批」、
   末集跨季接力（Tauri 语义）——`src/home.rs` 模块头有记账。

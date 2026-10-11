# 旧 Windows 无 HEVC 扩展的播放修复计划

> 背景：旧版 Win10（未装「HEVC 视频扩展」）反馈「有声音没画面」。WebView2 的 HEVC
> 解码器来自系统扩展而非自带，选流层又把 H.265 与 H.264 同等优先，选中 HEVC 系流即黑屏。
> 本文是处置计划，**确认后再动代码**。v4：安装层改为按需下载（不捆绑），新增常见问题文档。
> 关联文档：`platform-transcode-progress.md`（转码链）、`hongguo-api-endpoints.md`（端点台账）。

## 一、实证盘点

### 1.1 官方 APK 怎么做（TTVideoEngine CodecStrategy 还原）

逆向产物：`C:/Users/liu13/tools/rea/analysis/hongguo-apk/jadx-src13/sources/com/ss/ttvideoengine/`
（classes13.dex，本次补反编译；关键类 `source/strategy/CodecStrategy.java`、
`TTVideoEngineImpl.java`、`InfoWrapper.java`）。官方**从不假设任何 HEVC 系编码可解**，
选流完全由「解码维度 × 设备能力」驱动：

**五个解码维度**（`Dimension(decoder, encodeType)`）：

| 维度              | 解码方式 | encodeType | 设备支持判定                                |
| ----------------- | -------- | ---------- | ------------------------------------------- |
| BYTEVC1_HARDWARE  | 硬解     | bytevc1    | 宿主探测注册（MediaCodec 枚举）             |
| BYTEVC1_SOFTWARE  | 软解     | bytevc1    | 宿主探测 + SDK/服务端特性开关               |
| BYTEVC2_SOFTWARE  | 软解     | bytevc2    | 需 ByteVC1 软解在位 + 特性开关 `bytevc2`    |
| H264_HARDWARE     | 硬解     | h264       | 宿主探测注册                                |
| **H264_SOFTWARE** | 软解     | h264       | **恒 true**——Android 内置软解 AVC，永不缺席 |

**三条择优序列**（按 `codecStrategy` 配置选一，逐维度试 `findItemByEncodeType`）：

```
COST_SAVING_FIRST:      bytevc2软 → bytevc1硬 → bytevc1软 → h264硬 → h264软
HARDWARE_DECODE_FIRST:  bytevc1硬 → h264硬   → bytevc2软 → bytevc1软 → h264软
FALLBACK_LIST:          h264软   → h264硬   → bytevc1软 → bytevc1硬 → bytevc2软
                        ↑ 兜底序列从「必然可解」起步
```

选不出时走 `resolveFallback`（FALLBACK_LIST），而 H264_SOFTWARE 恒真 ⇒ **任何机器都有
一条必然可播的路**。此外运行期还有性能监测（`_doSupportByteVC1Playback`）：起播后盯
丢帧数/解码耗时，解码跟不上即判「不支持」触发降级——「能力」不只是静态探测，还包括
播着看跟不跟得上。字节另在 native 引擎内置了 bytevc1 软解器（`JX_ByteVC1_codec` 等字符串
佐证），由特性开关放行——这是它在 Android 上的「代码层 HEVC 支持」，因为 Android 没有
「装系统扩展」的概念。

**对本项目的直接推论**：官方的答案不是换编码协商参数、也不是硬塞解码器，而是
**「设备能解什么就选什么，兜底序列永远以必然可解的编码收底」**。这正是本计划 v1 的
能力感知选流（方案 A）——v2 曾因其撤下，现按官方架构恢复。附带的观察：官方维度表里
没有独立的 `h265`（Android 端 item 的 encodeType 只有 bytevc1/bytevc2/h264），我们
桌面端 video_list 里的 `h265/hvc1` 与 `bytevc1` 同属「有标准参数集的 HEVC」，能力映射
归并处理即可。

### 1.2 hgplayer（第三方）怎么做

分发目录捆绑 `Microsoft.HEVCVideoExtension` 原始 appx（2.1.451.0，微软商店付费包，
纯商店签名链，MinVersion 1709；WebView2 支持下限 1803 ≥ 1709，能跑本应用的机器必然
满足包门槛）。其《使用说明》的处置次序：缺扩展时后台自动装 → 失败提示手动双击 →
**装好前用软解顶着播**。即第三方同样是多层，捆绑只是其一。

### 1.3 本项目现状与缺口

```
取流 fetch_play_url → 选流 stream_pick → hongguo-stream 渐进供流 → <video>
  → 兜底 use-transcode-fallback：videoWidth==0 探测 → compat_play
      → 转码分流链 平台 MF（需 HEVC 解码 MFT）→ ffmpeg → 纯 Rust 软解
```

- 选流把 h265 与 h264 同列 rank 0，清晰度优先 ⇒ 无扩展机器经常选中解不了的流。
- 转码兜底链**已存在**（Rust 软解上限「几十秒/集」），但触发有缺口：解码类
  MediaError（code 3/4）在 `handleVideoError` 里先走「在线重试一次 → setError」，
  而 `videoWidth==0` 探测 effect 对 `error` 短路——确定性解码失败**绕过兜底**死在错误页。
- bytevc2 维持排除（私有 bv2C box、无 in-band 参数集，与扩展无关，官方
  `VeLivePlayerErrorBytevc2NotSupport` 同判）。

## 二、目标与非目标

**目标**：无 HEVC 扩展的 Windows 机器上开箱即可播（可能非最高档）；装上扩展即恢复
全档位；一切异常路径可靠落到既有转码兜底。

**非目标**：

- 不做 WASM 软解 + canvas 播放器：WebView2 的 `<video>` 塞不进外部解码器，等于手写
  一个播放器（音画同步/seek/缓冲），旧 CPU 上 1080p 软解吃力。官方的「代码层 HEVC
  支持」靠的是 native 引擎内置软解器——我们的对应物是**已有的 Rust 转码兜底**，不必重造。
- 不动下载选流与 compat 转码源取流（`transcode_cmd.rs` 的 `fetch_play_url` 必须保持
  不过滤：HEVC-only 剧集的兜底转码还要拿 HEVC 源）。
- 不做后台静默安装（hgplayer 是自动装；我们按「提示 + 用户一键确认」做）。
- 不采用 CDN URL `codec_type=3/4` query 协商（无实证，猜参数踩红线 9）。

## 三、方案（三层，各有官方/第三方先例）

```
① 可播性保证（官方 CodecStrategy 同款）
   探测 hevc_playable() → 无 HEVC 时选流只认 h264（+未知编码次优先）
② 画质补充（hgplayer 同款）
   无 HEVC 机器引导一键安装内置 appx → 装好恢复全档位
③ 异常底座（两层共有的兜底）
   MediaError 直接触发既有 compat 转码兜底（修触发缺口）
```

### 3.1 ① 能力感知选流（对齐官方架构）

**探测（后端，单一事实源）**

- 新增 `media/capability/playback.rs`：`pub fn hevc_playable() -> bool`，进程级缓存
  （对齐 `capability/probe.rs` 惯例）。Windows 实现：`MFTEnumEx(MFT_CATEGORY_DECODER, …,
MFVideoFormat_HEVC)` 判「HEVC 解码 MFT 在位」——`media/platform/mf/mod.rs` 平台探测
  已有同一检查，抽出独立廉价函数复用（只枚举不试编）。macOS 恒 `true`（`#[cfg]` 门控）。
- 调试开关 `HONGGUO_NO_HEVC`：对齐 `HONGGUO_NO_FFMPEG` / `HONGGUO_NO_PLATFORM` 先例，
  开发机全链模拟无扩展机器。
- IPC：新命令 `playback_capability`（响应 `{ hevcPlayable: boolean }`），按 ipc.md 五步
  全链；staleTime Infinity。3.2 与 3.3 也消费它。

**选流参数化（`domain/api/stream_pick/`）**

- 引入 `DecoderCapability`（`Full` / `NoHevc`），贯穿 `is_playable` / `stream_score` /
  `list_definitions` / `pick_stream_at`。语义对齐官方 `CodecStrategy` 的
  `Decoder.isSupport`：

| codec_type                                         | Full（现状）       | NoHevc                                                                                             |
| -------------------------------------------------- | ------------------ | -------------------------------------------------------------------------------------------------- |
| `h264` / `avc1`                                    | rank 0 可播        | rank 0 可播，**最优先**（对应官方 H264 底座——Win8 起系统内置 H.264 MF 解码器，永不缺席）           |
| 未知 / 空                                          | 按可播（保守放行） | **次优先**（大概率仍是 HEVC 系；「整集播不了」底线优先，官方对未知同样不赌）                       |
| `bytevc1` / `h265` / `h265_hvc1` / `hevc` / `hvc1` | rank 0/1 可播      | 不可播（WebView2 无内置软解，等价于官方「BYTEVC1_HARDWARE 探测失败」；装上扩展即恢复 = 转入 Full） |
| `bytevc2`                                          | 不可播             | 不可播（维持）                                                                                     |

- 排序：先「本机可解」，再沿用清晰度 > 像素 > 编码器 > 码率。NoHevc 下某档全不可解时
  该档从菜单消失（`list_definitions` 同步过滤）；手动 pin 了该档走现有静默回退
  （`pick_stream_at` target 未命中回退），`PlayResponse.definition` 回传实际档位。
- **接入点收口在 `play_service/online.rs`**（`prepare` 与 `prefetch_stream` 传探测结果
  给 `fetch_play_url`）；下载 scheduler 与 compat 转码源两个调用方不受影响（见非目标）。

### 3.2 ② 按需下载安装层（画质补充，**不捆绑**）

与 `longDescription`「安装包不捆绑外部二进制」的现有定位一致：appx 与 WebView2 安装器
都不进安装包，缺什么现场下载。

**资源常量**（链接已实测可用，哈希与 hgplayer 1.1.8 分发副本逐字节一致）：

| 资源                | 下载地址                                            | SHA-256（下载后必须校验）                                                         |
| ------------------- | --------------------------------------------------- | --------------------------------------------------------------------------------- |
| HEVC 视频扩展 appx  | `https://pixeldrain.com/api/file/jZmUr1cp?download` | `46695112aa70a05faf71c69ecf4ff0502a3b82d152168d3ffb67219eafe51b85`（2,491,371 B） |
| WebView2 离线安装器 | `https://pixeldrain.com/api/file/DE8nquEB?download` | `7ebc4ce80143ef89cea86a61ea151502868db6caaa678b8b43660a66ace11c3a`（1,793,816 B） |

WebView2 侧**应用内不做处理**（运行时缺 WebView2 应用根本起不来，无 UI 可言）：NSIS
默认 downloadBootstrapper 在安装时从微软源自动装；离线/微软源不通的场景由常见问题
文档给出上面的备用链接手动安装。

**安装命令 `install_hevc_extension`**

- 后端流程：reqwest 下载 appx 到临时目录（百 KB 级任务，不接进度事件，前端 loading 即可）
  → **SHA-256 校验**（不匹配即失败，防链接被换/传输损坏）→
  `tauri::async_runtime::spawn_blocking` 跑 `powershell -NoProfile -ExecutionPolicy Bypass
-Command Add-AppxPackage -Path '<临时文件>'` → 无论成败清理临时文件。
- `Add-AppxPackage` 用户级、免管理员、不依赖 App Installer；商店签名链全 Win10/11 受信，
  1709+ 侧载默认放开。超时与非零退出码映射 `AppError`（message 带原始错误码）。
- 装好即系统级 MFT 可见：成功后提示「重试播放」（复用 `retryTick`）；重试仍失败提示
  重启应用。

**触发时机与 UI**

- 进播放页且 `hevcPlayable === false` 出引导卡：说明（视频为 H.265 编码，缺系统扩展会
  「有声音没画面」）+ 一键安装 Button（下载+安装全程 loading）+ 暂不安装；每会话至多
  一次，可关闭。
- 装失败错误卡两条手动指引：① 按常见问题文档手动下载 appx 双击安装（文档附直链与
  校验值）；② 商店直达——`open_external_page` 白名单追加
  `ms-windows-store://pdp/?ProductId=9N4WGH0Z6VHQ`（一行配置）。
- 用户可见文案同步 `zh-CN.json` / `en-US.json`。

**常见问题文档（新增 `docs/faq.md`，用户视角措辞，发布页/README 引用）**

- 缺 HEVC 扩展（有声音没画面）：应用内一键修复 / 手动下载双击安装（附直链 + SHA-256
  - 装好重启）/ 商店购买备选（付费说明）；系统要求 Win10 1709+，macOS 无此问题。
- 缺 WebView2（应用起不来）：安装器会自动装；自动失败时用官方在线安装器或上面的
  离线备用链接手动装，装好重启。
- 附带：安全软件误报说明、本地回环（127.0.0.1）防火墙提示的解释、Win10 1803 之前
  不支持的声明。

### 3.3 ③ 兜底触发补齐（小修）

- `handleVideoError` 增加判别（抽纯函数 `classifyMediaError` 进 vitest）：`code === 3/4`
  为解码类——在线流且未兜底过时直接 `startCompat()`，**跳过 retryOnline**（确定性解码
  失败重试必然复现）；`code === 2` 维持现有「重试一次 → 错误页」。
- 现有 `videoWidth === 0` 轮询保留，共用 `compatStarted` 防重复。
- 兜底 overlay 文案补预期（纯软解较慢属正常）。

## 四、验证计划

1. **开发机全链模拟**（`HONGGUO_NO_HEVC=1 make dev`）：
   - 选流日志证实选中 h264；清晰度菜单不含 HEVC-only 档；pin 该档静默回退正确。
   - 引导卡出现；一键安装真跑（下载 → 哈希校验 → Add-AppxPackage → 临时文件清理；
     已装机器命中「已安装」类错误恰好覆盖失败文案；完整成功路径在干净虚拟机验；
     哈希不匹配路径用截断文件验拒装）。
   - FAQ 里的两条手动直链可用性复核。
   - 不开 env 行为与现状完全一致（Full 回归基线）。
2. **单测**：stream_pick 行为矩阵全表（Full/NoHevc × codec 集 × target/None × 回退，
   语义对齐官方 `CodecStrategy`：优先序列 + 恒真底座）；`hevc_playable()` 缓存与 env；
   `classifyMediaError` 判别表；安装命令错误映射。
3. **抓包核实（建议不阻塞）**：model 接口 codec × definition 分布，量化 h264 档位
   覆盖率。官方 FALLBACK_LIST 以 h264 为底座，佐证其存在性可靠；抓包只为量化「无扩展
   机器的画质损失面」，结论回写 `hongguo-api-endpoints.md`。
4. **真机验收**（反馈来源的旧 Win10）：不装扩展起播有画面（可能降档）；装扩展后全档位
   可播；拒装路径 compat 兜底出画面。
5. **门禁**：`make lint` / `make test` / `make test-rust` 裸跑确认退出码；提交前 dev 冒烟。

## 五、提交拆分（conventional commits + 中文，均为行为改动故各自成提交）

1. `feat(play): HEVC 解码能力探测`（后端 probe + `playback_capability` 命令全链）
2. `feat(play): 无 HEVC 扩展机器选流翻转到 H.264`（stream_pick 参数化 + 单测对齐官方语义）
3. `feat(play): HEVC 扩展按需下载一键安装`（下载+校验+安装命令 + 播放页引导卡 + i18n）
4. `docs(faq): 常见问题——缺 HEVC 扩展与 WebView2 的处理方式`
5. `fix(player): 解码类 MediaError 直接触发转码兜底`

## 六、风险与开放问题

| #      | 事项                                                                       | 处置                                                                                                                                                                                                                                                         |
| ------ | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 风险 1 | 组策略禁侧载 / 低于 1709 的系统装不上 appx                                 | 手动指引 + ③ 兜底接住；且 ① 已保证这类机器可播（降档）                                                                                                                                                                                                       |
| 风险 2 | pixeldrain 为第三方文件床，可能限流/失效                                   | 下载 URL 与 SHA-256 做成常量便于随时替换；引导卡保留商店直达手动指引                                                                                                                                                                                         |
| 风险 3 | MF「MFT 在位」与 WebView2 实际行为的边缘不一致                             | ③ 兜底接住；反馈集中再引入前端 `mediaCapabilities` 复核                                                                                                                                                                                                      |
| 风险 4 | NoHevc 下未知编码（无 codec_type）仍可能黑屏                               | 维持「整集播不了」底线，次优先 + ③ 兜底（官方对未知同样不赌但也不排除）                                                                                                                                                                                      |
| 风险 5 | 微软付费包再分发的合规风险                                                 | 不进安装包、缺时按需下载 + 哈希校验；分发链接的可见性由维护者自决                                                                                                                                                                                            |
| 开放 1 | 下载文件在该类机器上的播放（下载选流是否翻转）                             | 本期不动：牵动合并/转码管线与体积，单独评估                                                                                                                                                                                                                  |
| 开放 2 | 纯 Rust 软解兜底的等待时长                                                 | overlay 文案说清预期；提速属转码链议题                                                                                                                                                                                                                       |
| 开放 3 | 官方运行期性能监测降级（丢帧→降级）是否值得抄                              | WebView2 可用 `requestVideoFrameCallback` 数丢帧近似实现；本期不做，观察反馈                                                                                                                                                                                 |
| 开放 4 | bytevc2 能否救回：bv2C→hvcC 转封装（不解码，只把参数集从私有盒搬进标准盒） | 官方靠 APK 内置软解库 `ByteVC1_dec`/`byteVC2dec` 直解（`TTPlayerLibLoader.java`，与 ttffmpeg 同载），不依赖系统解码器；WebView2 注入不了解码器，转封装只对下载件可行且需逆向未公开的 bv2C 格式——收益（有扩展机器多救个别 bytevc2-only 档位）不抵成本，暂不做 |

## 七、迭代调整（2026-10-11）：失败先问 + 连播预转

实测《逆江湖之苍生为局》（用户报告播不了的剧）后追加：全剧无 h264
rendition，选流翻转帮不上它，无扩展机器只能靠兜底转码；而旧机纯软解
分钟级等待「慢得吓人」。两项调整：

**① 解码失败先问，转码改为拒装后的自救**

- 解码失败（MediaError 3/4 或 `videoWidth==0` 轮询）且 `hevcPlayable === false`
  时，不再默默起转码，先出**居中主卡**二选一：一键安装（10–30 秒，之后
  全剧原生秒开）/ 暂不安装转兼容格式（本集分钟级）。选「转兼容」本会话
  记授权，连播后续集径直兜底不再重复问；floating 卡保留为进播放页时的
  proactive 提示（降档播放时的画质升级入口）。
- `hevcPlayable !== false`（有扩展/未落定）的解码失败维持直接兜底。

**② 连播预转下一集（`compat_prefetch`）**

- 当前集在线起播成功后，若本机无 HEVC：NoHevc 视角取下一集流表，
  `definitions` 为空（HEVC-only）才后台预转兼容产物；有可选流的原生直
  播，不浪费转码。
- 会话账本 `{series}:{vid}` 去重（一集至多查/转一次）；进度事件按集发，
  前端只认当前集，无打扰；失败只进日志。连播到预转集时兜底命中缓存，
  等待只剩每部剧的第一集。

**服务端协商调查（否证）**：官方 APK 的 `codec_type=3/4`、`Codec=bytevc1`
参数只存在于 TTVideoEngine 的 vid-token 取流路径；红果 novel model 接口
的请求（业务 dex）无任何 codec 字段——官方同样拿到 HEVC-only，靠内置
软解库硬吃。按红线 9 无协商可做。


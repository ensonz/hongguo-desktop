# GPUI 播放引擎对齐主流：平台音频解码 + 时钟校正 + 倍速/音量/seek + 控制区

> 状态：已确认方向（2026-10-12，用户三问三答：症状=音画不同步；解码路线=平台解码器；范围=三步连做）。
> 适用范围：`crates/hongguo-playback/`（引擎）、`crates/hongguo-media-platform-macos/src/audio.rs`（新增平台解码；架构二期前称 `media-platform/src/platform/audio/`）、`crates/hongguo-video/`（elst 解析）、`crates/hongguo-gpui/`（控制区 UI）。

## 1. 背景与症状

GPUI 壳的播放引擎（`hongguo-playback`）已经做到「画面 + 声音都能出来」，但：

- **音画不同步**（用户报告的现行症状；前几轮已修掉无声与大幅偏差，残余为系统性小偏差）；
- 引擎只有 `pause/resume`，无 `set_rate / set_volume / seek`——Tauri 版控制面（倍速 0.75–3x、音量/静音、进度条）没有引擎支撑；
- 缓冲模型不完整：设备空转放静音，无 rebuffer 状态机（主流是暂停 + loading，绝不出静音）。

## 2. 对照主流的审计结论

参照系：bilibili（桌面=Chromium 媒体栈，移动=ijkplayer/ffplay 血统）、抖音（TTVideoEngine）、ExoPlayer（Android 主流基线）、AVPlayer。共同机制：

| 机制 | 主流做法 | 引擎现状 | 判定 |
| --- | --- | --- | --- |
| 音主时钟 | 音频设备消费位置推进（ffplay 字节法 / ExoPlayer AudioTrack clock / mpv） | 一致（cpal 回调消费计数） | 对齐 |
| 时钟基准 | **可闻位置**：回调时间戳（playback 时刻）扣除设备引线；Chromium/ExoPlayer 均如此 | 回调拉取即计时，**未扣设备引线**（约 10–40ms，蓝牙更大） | **偏差，根因之一** |
| AAC priming | 按 elst / 编码器延迟修剪开头样本 | `hongguo-video` 不解析 elst（约 40–50ms 恒定偏移，方向为声音偏晚） | **偏差，根因之二** |
| 解码器 | 平台解码器（Chromium 内建 / AVFoundation / TTVideoEngine），HE-AAC 全覆盖 | symphonia 0.5 仅 AAC-LC，无 SBR/PS | **偏差（能力缺口 + 对齐）** |
| 缓冲模型 | 缓冲不足→暂停渲染 + loading（ExoPlayer BUFFERING / 状态机 + 水位阈值），PCM 冲 ~1s | 环形缓冲 ¼s，无状态机，欠载放静音 | **偏差（断续感的来源）** |
| 倍速 | 音频 WSOLA 变速不变调（soundtouch/Sonic/audioTimePitch），时钟×rate，视频到期时间随媒体时间 | 无 | 缺失 |
| 音量/静音 | 设备回调内乘 PCM（零延迟），静音独立于音量 | 无 | 缺失 |
| seek | 冲解码器与设备缓冲，时钟重置，关键帧起步 | 仅起播 start_secs | 缺失 |
| 丢帧策略 | 视频迟到→丢/跳帧 | 不丢帧、晚显示（用户红线，解码 13–17ms < 33ms 实时线，正常不触发） | 比主流更严，保留 |
| B 帧显示序重排 / 预填起播 / 解码前瞻 | 标准 | 一致 | 对齐 |

音画不同步的残余系统性偏差 = 设备引线（时钟快于耳朵 10–40ms）+ priming 未修剪（声音内容晚 40–50ms）+ 上屏链路 1 帧级延迟，同向叠加可到可感知阈值（±45ms 级）以上。三项都是恒定偏差，逐一机制性消除，不做 NTP 式对表补丁。

## 3. 阶段一：平台音频解码 + 时钟校正（根治音画不同步）

### 3.1 平台 AAC 解码器（替换 symphonia）

- 新模块 `hongguo-media-platform-macos/src/audio.rs`（当时位于 `media-platform/src/platform/audio/`，与 `vt/` 同构）：
  - 手写 AudioToolbox FFI（`AudioConverterOpen / SetProperty(DecompressionMagicCookie) / FillComplexBuffer / Reset / Dispose`，照 `vt/ffi.rs` 的「最小声明 + framework 链接」写法，零新增依赖）；
  - 接口：`AacDecoder::open(asc: &[u8], channels, sample_rate) -> Self`，`decode(packet: &[u8]) -> AppResult<Vec<f32>>`（交错 f32，输出格式=mp4 声明的声道/采样率，HE-AAC 的 SBR/PS 上变频由系统解码器完成）；
  - mpv 的 coreaudio AAC 解码即此路线（AudioConverter + magic cookie），作为参照实现。
- `hongguo-playback` 移除 symphonia 依赖，`audio_feeder` 改用平台解码器。
- 起播日志加一行 ASC 的 audioObjectType（2=LC，5=HEv1，29=HEv2）——诊断留证。
- Windows：引擎现状即 macOS-only（依赖 VideoToolbox 实现）；MF AAC 是 Windows 壳立项时的事，此处不占位不兜底。

### 3.2 时钟扣除设备引线

- 回调内用 cpal `OutputCallbackInfo::timestamp()`：`playback − callback` 的差即本缓冲可闻时刻的引线（coreaudio 后端按 IO 缓冲周期填充）。
- 时钟口径从「已拉取帧数」改为「已拉取帧数 − 引线帧数」，即**可闻位置**。蓝牙传输延迟 CoreAudio 不暴露（主流同样不补），可闻位置已对齐主流能做到的部分。

### 3.3 elst 解析与 priming 修剪

- `hongguo-video` 样本表新增 `edts/elst` 解析：每轨 `media_start`（elst 的 media_time），样本 pts 统一减去它。AAC 编码器 priming 由容器 edit list 表达，这是机制性修剪，不是猜常数。
- AudioToolbox 解码输出的 priming 行为以实测定论：若流无 elst 且解码首包含前导静音，按实测补偿（验证步骤见 §6，对齐参照实现在源头，不拍脑袋）。

## 4. 阶段二：引擎 API 与缓冲状态机

- **缓冲状态机**：环形缓冲 ¼s → 1s；低于低水位（0.25s）→ 暂停设备 + `PlayerEvent::Buffering`；回填到高水位（0.6s）→ 恢复 + `PlayerEvent::Ready`。设备永不空转放静音。
- **set_volume / set_muted**：设备回调内对拉取样本乘增益（零延迟生效，静音独立于音量，语义对齐 Tauri 版 playback-prefs）。
- **set_rate**：`hongguo-playback` 内置纯 Rust WSOLA（变速不变调，ijkplayer/soundtouch 同族算法；不引依赖、Windows 通用）。媒体时间 = base + 可闻帧数 × rate ÷ 采样率，倍速切换时 rebase；视频侧无需改动（媒体时间已含 rate）。
- **seek(secs)**：seek 目标经共享态下达；视频循环冲重排缓冲、重建 VT 会话、从 `start_index`（关键帧）重启；音频 AudioConverterReset、经冲流协议清空环形缓冲（回调丢弃计数 + 喂送端重新预填后放行）；时钟 reset 到目标点。预填复用现有 prime 机制。
- **音频先于视频结束**：喂送结束时若时钟仍是音频时钟，切到锚定当前位的挂钟，避免尾帧冻结（现存边界）。
- `PlayerHandle` 新增：`set_rate / rate / set_volume / volume / set_muted / muted / seek`。

## 5. 阶段三：GPUI 控制区（对齐 Tauri 版控制面）

对齐对象 `src/features/player/components/controls/player-controls.tsx`，本轮做播放器本体核心件：

- 播放/暂停（控制条按钮 + 点击画面切换）；
- 进度条：当前/总时长显示、拖动 seek（引擎 `seek`）；
- 倍速菜单：档位 `[0.75, 1, 1.25, 1.5, 2, 3]`（与 Tauri `RATES` 完全一致）；
- 音量滑杆 + 静音（独立状态）；
- 播放偏好在壳侧持久化（倍速/音量/静音随集与重启保留，语义对齐 Tauri 的 playback-prefs）。

组件用 gpui-kit（button/slider/menu），交互与视觉遵循 gpui-ui-design skill。画质/选集/弹幕/隐身/全屏/下载/小窗不在本轮（依赖 GPUI 首页形态本身，另立计划）。

## 6. 验证与验收

- 单测：WSOLA（时长比 ≈ 1/rate、静音进静音出、确定性）、时钟数学（引线扣除、rate 换算、rebase）、elst 解析、冲流协议状态、倍速档位表。
- `make test-rust`、`make lint` 裸跑全绿。
- 用户对照实验（同集对播 Tauri 版）：① 口型对齐（起播 10s / 1min / 5min 三点）；② 倍速 2x 听感（不变调）；③ 进度条拖动后口型仍对；④ 拖网速/切设备验证不再出静音断续。

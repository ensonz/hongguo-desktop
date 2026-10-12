# Windows 接入 GPUI 播放：上机 checklist

> 状态：引擎层（`hongguo-media-platform` / `hongguo-playback`）已在 macOS 上对
> `x86_64-pc-windows-msvc` 完成**类型检查**；gpui 壳的 Windows 编译只能在真机做
> （`aws-lc-sys` 的 C 构建需要 MSVC 工具链）。本文是 Windows 设备上的接入步骤
> 与已知风险清单。

## 平台布局（本轮落定的结构）

```text
platform::video   平台中立视频解码门面：macOS=VT（CVPixelBuffer 零拷贝）/
                   Windows=MF（NV12 系统内存帧），引擎与壳只认 VideoFrame
platform::audio   平台中立 AAC 解码：macOS=AudioToolbox / Windows=MF 解码 MFT，
                   同一接口（ASC open / 逐包 decode / seek reset）
mf::playback      播放专用解码 MFT（HEVC + AAC），专用线程 + 请求-应答；
                   HEVC 序列头走 hvcC（MF_MT_MPEG_SEQUENCE_HEADER），被拒时
                   首样本前插带内参数集（按 length_size 前缀）兜底
渲染（home.rs）   macOS=surface()（gpui 仅 Apple 目标有视频 surface）；
                   Windows=事件任务里 NV12→RGBA（BT.709 limited，hongguo-media
                   的纯函数转换）后按普通图像 img() 上传
```

## 上机步骤

1. 环境确认（一次性）：

   ```powershell
   rustup target list --installed          # 应含 x86_64-pc-windows-msvc（默认宿主目标）
   rustc --version                          # >=1.99
   # HEVC 解码 MFT 在位性（缺了视频路径起不来）：
   #   设置 → 应用 → 搜「HEVC 视频扩展」；或直接看起播日志（下一步）
   ```

2. 逐层编译（顺序即依赖序，哪层红修哪层）：

   ```powershell
   cargo check -p hongguo-media-platform    # 已在 mac 交叉验证过，应当绿
   cargo check -p hongguo-playback          # 同上
   cargo check -p hongguo-gpui              # 真机首编：aws-lc-sys 需要 MSVC
                                             #（Build Tools 的 C++ 工具链 + CMake）
   ```

3. 运行与验收：

   ```powershell
   cargo run -p hongguo-gpui
   # 观察日志（RUST_LOG=debug 更细）：
   #   [Platform/mf] HEVC 解码会话就绪（WxH，stride N）   ← MFT 建成
   #   [Playback] 音频 profile：AOT …                     ← AAC 走的哪族
   #   [Playback] 首帧 WxH（显示时间 …）                   ← 出画
   #   [Playback][sync] 偏差 ±0.15s 内                    ← 音画对齐
   #   [Home][stage] 封面层/视频层 …                       ← 布局哨兵
   # 验收项与 mac 相同：口型、2x 不变调、进度条拖动后对齐、弱网 Buffering/Ready。
   ```

## 已知风险（真机首发重点盯）

- **HEVC 解码 MFT**：来自系统「HEVC 视频扩展」，个别精简系统没有——
  起播会报 `没有 HEVC 解码 MFT`，装扩展即好（Tauri 壳的 WebView2 同一依赖）。
- **hvcC 序列头形态**：`MF_MT_MPEG_SEQUENCE_HEADER` 喂 hvcC 原始载荷是文档
  口径；个别解码器实现若拒收，代码已自动回落带内参数集（看
  `hvcC 序列头被拒` 日志确认走了哪条）。
- **AAC 解码器首包延迟/priming**：MF 的 AAC MFT 与 AudioToolbox 对编码器
  延迟的处理未必一致——若 Windows 上口型恒定偏 20–50ms，先对比两平台
  `[sync]` 偏差读数，再定按平台补偿。
- **WASAPI 设备引线**：cpal 的 WASAPI 后端同样填 `playback` 时间戳；若
  Windows 上整体偏快，检查 `[cb]` 日志里的「引线」读数是否为 0（为 0 说明
  该后端没填，需要按设备周期补）。
- **gpui-kit Windows 构建**：`hongguo-gpui` 的 Windows 编译在本仓从未跑过
  （Makefile 的交叉 clippy 一直排除它）。首编的报错大概率在依赖树
  （bindgen/Windows SDK），不在业务代码。

## 门禁口径

macOS 侧 `make lint`/`make test-rust` 全绿（引擎三平台纪律不变）。Windows
宿主的 clippy 由 CI 的 Windows job 覆盖；本地 PowerShell 可裸跑：

```powershell
cargo clippy --workspace --all-targets -- -D warnings -D clippy::allow_attributes
cargo test --workspace
```

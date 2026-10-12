//! 播放引擎（hongguo-playback crate）——§5.3 帧流的正式落地。
//!
//! 输入 `(vid, 档位)`（流已由调用方经 `play_service` 注册进 hongguo-stream
//! 缓存），输出两条：**NV12 CVPixelBuffer 帧**（`latest_frame` 槽，渲染侧
//! 零拷贝上屏）与**扬声器声音**（cpal 输出）。音频硬件的可闻消费位置是主
//! 时钟（扣设备引线、含倍速换算），视频按源 PTS 对表上屏——与 Tauri 侧
//! `<video>` 同一「媒体自己走时间轴」的语义，同步逻辑不进宿主。
//!
//! ```text
//! serve() 按样本表逐段取明文 ─┬─ 视频样本 → VT 解码(420f NV12) → 帧槽 ─→ surface()
//!                             └─ 音频样本 → AudioToolbox 解码 → WSOLA 倍速
//!                                → 环形缓冲（水位状态机）→ cpal → 时钟
//! ```
//!
//! 取流编排（prepare/prefetch/预算升格）留在壳侧的 `play_service`：引擎与
//! 具体壳、具体编排入口解耦，只认「serve 能读到的流」。
//!
//! 生命周期：一次 [`open`](player::open) 对应一集（对齐 Tauri 切集重挂载的
//! 语义——连播/换剧就是换一个引擎实例）；句柄 Drop 即停（任务 abort、音频
//! 设备释放）。播放中 seek/倍速/音量/静音走 [`PlayerHandle`](player::PlayerHandle)。

mod audio;
pub mod player;
mod source;
mod video;
mod wsola;

pub use player::{OpenOptions, PlayerEvent, PlayerHandle, open};

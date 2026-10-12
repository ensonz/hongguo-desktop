//! PCM 喂送侧的设备适配：采样率重采样 + 声道映射。
//!
//! Windows 的 WASAPI 共享模式对任意采样率**不做转换**（cpal 也不开
//! `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`）：解码侧 44100Hz 打到只收 48000Hz
//! 的设备（USB 耳机/HDMI 常见）直接建流失败，播放器退化为无声。macOS 的
//! CoreAudio 设备侧自带 SRC，所以这层问题只在 Windows 暴露。这里在**喂送侧**
//! 把解码 PCM 适配成设备混音格式：环形缓冲、回调、时钟、音量全部统一在
//! 设备域（设备帧数 ÷ 设备采样率 = 媒体秒，重采样保持时长不变）。
//!
//! 插值用立方 Hermite（四点、精确到三次多项式）：对语音/剧集内容足够，
//! 无新增依赖；`step` 与相位用 f64 累计，长播放不漂。

/// 喂送侧 PCM 适配器（交错 f32）。解码格式 → 设备混音格式。
pub(crate) struct PcmAdapter {
    src_ch: usize,
    dst_ch: usize,
    dst_rate: u32,
    /// 每产出一帧消耗的源帧数（>1 下采样，<1 上采样）。
    step: f64,
    /// 下一输出帧在 `tail` 坐标系里的源侧位置（小数）。
    pos: f64,
    /// 跨次调用保留的源帧尾巴（cubic 窗口 [pos-1, pos+2) 的左沿）。
    tail: Vec<f32>,
    /// 输出复用缓冲（避免每次 push 分配）。
    out: Vec<f32>,
    passthrough: bool,
}

impl PcmAdapter {
    pub(crate) fn new(src_ch: u16, src_rate: u32, dst_ch: u16, dst_rate: u32) -> Self {
        let passthrough = src_ch == dst_ch && src_rate == dst_rate;
        Self {
            src_ch: src_ch as usize,
            dst_ch: dst_ch as usize,
            dst_rate,
            step: f64::from(src_rate) / f64::from(dst_rate),
            pos: 1.0,
            tail: Vec::new(),
            out: Vec::new(),
            passthrough,
        }
    }

    /// 喂一段解码 PCM（源域交错），返回适配后的设备域交错切片。
    /// 返回值只在下一次调用前有效（内部复用缓冲）。尾部的亚毫秒残量
    /// （不足一个 cubic 窗口的最后几帧）留给下一次调用续上；整集播完
    /// 丢弃亦不可闻。
    pub(crate) fn process(&mut self, input: &[f32]) -> &[f32] {
        if self.passthrough {
            self.out.clear();
            self.out.extend_from_slice(input);
            return &self.out;
        }
        debug_assert!(input.len().is_multiple_of(self.src_ch));
        self.tail.extend_from_slice(input);
        let ch = self.src_ch;
        let dst_ch = self.dst_ch;
        let frames = self.tail.len() / ch;
        self.out.clear();
        // 目的声道 c 在第 frame 帧源数据上的取样点（声道映射）
        let sample = |tail: &[f32], frame: usize, c: usize| -> f32 {
            if ch == 1 {
                tail[frame]
            } else if dst_ch == 1 {
                tail[frame * ch..frame * ch + ch].iter().sum::<f32>() / ch as f32
            } else {
                let idx = c.min(ch - 1);
                tail[frame * ch + idx]
            }
        };
        // cubic 窗口 [pos-1, pos+2)：i = floor(pos)，需要帧 i-1..=i+2 在位
        while self.pos + 2.0 < frames as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..self.dst_ch {
                let s0 = sample(&self.tail, i - 1, c);
                let s1 = sample(&self.tail, i, c);
                let s2 = sample(&self.tail, i + 1, c);
                let s3 = sample(&self.tail, i + 2, c);
                self.out.push(hermite(s0, s1, s2, s3, t));
            }
            self.pos += self.step;
        }
        // 丢弃不再需要的帧（保留 floor(pos)-1 起），坐标原点随之平移
        let drop = (self.pos.floor() as usize).saturating_sub(1);
        if drop > 0 {
            self.tail.drain(..drop * ch);
            self.pos -= drop as f64;
        }
        &self.out
    }

    /// seek 冲流时清相位与尾巴（新旧位置不连续，续插值只会造一次毛刺）。
    pub(crate) fn reset(&mut self) {
        if self.passthrough {
            return;
        }
        self.pos = 1.0;
        self.tail.clear();
    }

    /// 设备域声道数/采样率（喂送侧把设备样本数换算成秒用）。
    pub(crate) fn dst_format(&self) -> (u16, u32) {
        (self.dst_ch as u16, self.dst_rate)
    }
}

/// 四点立方 Hermite 插值（t ∈ [0,1)）。线性 ramp 输入下输出精确等于输入
/// （三次多项式可被精确重构）——这是单测的锚点。
fn hermite(s0: f32, s1: f32, s2: f32, s3: f32, t: f32) -> f32 {
    let diff = s2 - s0;
    let a = 2.0 * s0 - 5.0 * s1 + 4.0 * s2 - s3;
    let b = 3.0 * (s1 - s2) + s3 - s0;
    s1 + 0.5 * t * (diff + t * (a + t * b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同率同声道：内容原样直通（采样不增不减不变值）。
    #[test]
    fn passthrough_when_formats_match() {
        let mut a = PcmAdapter::new(2, 48_000, 2, 48_000);
        let input = [0.25f32, -0.25, 0.5, -0.5];
        assert_eq!(a.process(&input), input);
    }

    /// 时长守恒：N 帧 44100Hz 喂进 48000Hz，产出 ≈ N×48000/44100
    /// （尾部不足一个 cubic 窗口的残量 < 4 帧）。
    #[test]
    fn rate_change_preserves_duration() {
        let mut a = PcmAdapter::new(2, 44_100, 2, 48_000);
        let mut produced = 0usize;
        let mut fed = 0usize;
        // AAC 解码粒度（~1024 帧）分多次喂入
        for _ in 0..43 {
            let chunk = vec![0.0f32; 1024 * 2];
            fed += 1024;
            produced += a.process(&chunk).len() / 2;
        }
        let expect = fed as f64 * 48_000.0 / 44_100.0;
        assert!(
            (produced as f64 - expect).abs() < 8.0,
            "产出 {produced} 帧，应 ≈{expect:.1}"
        );
    }

    /// 立方 Hermite 精确重构三次以内多项式：线性 ramp 进 → 线性 ramp 出。
    /// （斜率受 rate 缩放：输出时间轴 t' 对应源侧 t = t'×step。）
    #[test]
    fn linear_ramp_interpolates_exactly() {
        let mut a = PcmAdapter::new(1, 48_000, 2, 96_000); // 上采样 2 倍
        let input: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
        let out = a.process(&input).to_vec();
        // 输出帧 k 的源侧位置 = 1 + k×0.5 → 期望值 = (1 + k/2)×0.01
        for (k, frame) in out.chunks(2).enumerate() {
            let expect = (1.0 + k as f32 * 0.5) * 0.01;
            for s in frame {
                assert!((s - expect).abs() < 1e-4, "帧 {k}: {s} ≠ {expect}");
            }
        }
    }

    /// mono → stereo：两道复制同一值；幅度不因声道数漂移。
    #[test]
    fn mono_upmix_duplicates_channel() {
        let mut a = PcmAdapter::new(1, 48_000, 2, 48_000);
        let input = [0.5f32, 0.6, 0.7, 0.8, 0.9];
        let out = a.process(&input).to_vec();
        for frame in out.chunks(2) {
            assert_eq!(frame[0], frame[1]);
        }
    }

    /// reset 清状态：ramp 适配器 reset 后再喂常数，输出不含旧尾巴的斜坡残值。
    #[test]
    fn reset_drops_history() {
        let mut a = PcmAdapter::new(1, 44_100, 1, 48_000);
        let ramp: Vec<f32> = (0..32).map(|i| i as f32).collect();
        let _ = a.process(&ramp);
        a.reset();
        let flat = [0.5f32; 64];
        let out = a.process(&flat).to_vec();
        // 首帧就在窗口内（pos=1 有 i-1=0 在位），值应立即贴住 0.5
        assert!((out[0] - 0.5).abs() < 1e-5, "reset 后首帧 {} ≠ 0.5", out[0]);
    }
}

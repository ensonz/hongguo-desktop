//! WSOLA 变速不变调（Waveform Similarity Overlap-Add）。
//!
//! 倍速的正确做法（主流一致：ijkplayer 用 soundtouch、ExoPlayer 用 Sonic、
//! AVPlayer 用 audioTimePitch）：音频时长伸缩 + 音高保持。绝不能靠丢音频帧
//! （断续）或改采样率（变调）。本实现是 WSOLA 的最小完整版：
//! 固定输出步长（hop），**名义读取指针独立按 hop × rate 推进**（对齐只在
//! ±search 内修相位——指针跟着对齐点走会在恒定素材上定点死循环），每个新窗
//! 在搜索窗内找与「自然延续」最相似的相位对齐后 Hann 重叠加窗。归一按
//! 逐帧累计权重相除（窗长固定为最大倍速下的最长滑距 + 余量，任意倍速下
//! 叠加和都精确归一）。
//!
//! 输入输出都是交错 f32，帧 = 每声道样本组。纯函数逻辑，无平台依赖。

/// 输出步长（帧）：10ms。
const HOP_MS: f64 = 0.010;
/// 相位搜索半径（帧）：5ms。
const SEARCH_MS: f64 = 0.005;
/// 窗长 = 步长 × 本系数。须保证最大倍速（4x）下单步滑距 4·hop 之后仍有
/// ≥ 3·hop 的重叠可交叉淡化，否则高倍速出「缝」。
const WIN_HOP_RATIO: usize = 7;

pub(crate) struct Wsola {
    channels: usize,
    /// 输出步长与窗长（帧）。
    hop: usize,
    win: usize,
    search: usize,
    /// 未消费的输入（交错样本），队列头 = 名义消费基点。
    queue: Vec<f32>,
    /// 下一个窗的名义读取位置（帧，相对队列头）。按 hop × rate 推进，
    /// 与对齐结果解耦。
    next_in: f64,
    /// 上一个对齐窗的自然延续（hop 帧）：下一个窗的相位搜索目标。首窗无。
    natural: Option<Vec<f32>>,
    /// 叠加尾（窗长 − 步长帧）：加权和与权重和各一份，发射时相除归一。
    tail: Vec<f32>,
    tail_w: Vec<f32>,
    /// Hann 窗（长 win，按帧）。
    envelope: Vec<f32>,
}

impl Wsola {
    pub(crate) fn new(sample_rate: u32, channels: u16) -> Self {
        let hop = (sample_rate as f64 * HOP_MS).round().max(1.0) as usize;
        let win = hop * WIN_HOP_RATIO;
        let search = (sample_rate as f64 * SEARCH_MS).round().max(1.0) as usize;
        let envelope: Vec<f32> = (0..win)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / win as f32).cos()))
            .collect();
        Self {
            channels: usize::from(channels),
            hop,
            win,
            search,
            queue: Vec::new(),
            next_in: 0.0,
            natural: None,
            tail: vec![0.0; (win - hop) * usize::from(channels)],
            tail_w: vec![0.0; (win - hop) * usize::from(channels)],
            envelope,
        }
    }

    /// 喂入一段解码 PCM（交错），按当前倍速吐出伸缩后的样本到 `out`。
    /// rate 变化逐窗生效（next_in 推进量跟着变），无需专门冲状态。
    pub(crate) fn process(&mut self, input: &[f32], rate: f64, out: &mut Vec<f32>) {
        debug_assert_eq!(input.len() % self.channels, 0);
        self.queue.extend_from_slice(input);
        // 推进到「名义位置 + 搜索 + 窗 + 一个自然延续」都拿得齐才开窗
        let need = |next_in: f64, this: &Self| {
            (next_in as usize + this.search + this.win + this.hop) * this.channels
        };
        while self.queue.len() > need(self.next_in, self) {
            self.step(rate, out);
        }
        // 已消费区收缩：对齐点之前 search+hop 帧以外不再会被引用
        let keep_from = self.next_in as usize;
        let drop_frames = keep_from.saturating_sub(self.search + self.hop);
        if drop_frames > 0 {
            let at = drop_frames * self.channels;
            self.queue.drain(..at);
            self.next_in -= drop_frames as f64;
        }
    }

    /// 发射一个 hop 的输出帧。
    fn step(&mut self, rate: f64, out: &mut Vec<f32>) {
        let (hop, win, search, ch) = (self.hop, self.win, self.search, self.channels);
        // 相位对齐：在名义位置 ± search 内找与自然延续最相似的起点
        let p = match &self.natural {
            None => self.next_in as usize,
            Some(natural) => {
                let lo = (self.next_in as usize).saturating_sub(search);
                let hi = (self.next_in as usize + search).min(self.queue.len() / ch - win - hop);
                let mut best = lo;
                let mut best_score = f64::INFINITY;
                let mut cand = lo;
                while cand <= hi {
                    let mut score = 0f64;
                    for j in 0..hop {
                        let a = (cand + j) * ch;
                        let b = j * ch;
                        for c in 0..ch {
                            let d = self.queue[a + c] as f64 - natural[b + c] as f64;
                            score += d * d;
                        }
                        if score >= best_score {
                            break; // 早退：已不可能更优
                        }
                    }
                    if score < best_score {
                        best_score = score;
                        best = cand;
                    }
                    cand += 1;
                }
                best
            }
        };

        // 重叠相加：本窗前 hop 并进头部（加权和 + 权重和），相除归一后发射
        let window = &self.queue[p * ch..(p + win) * ch];
        for j in 0..hop {
            for c in 0..ch {
                let at = j * ch + c;
                self.tail[at] += window[at] * self.envelope[j];
                self.tail_w[at] += self.envelope[j];
                let w = self.tail_w[at];
                out.push(if w > 1e-4 {
                    self.tail[at] / w
                } else {
                    self.tail[at]
                });
            }
        }
        shift_tail(
            &mut self.tail,
            &mut self.tail_w,
            window,
            &self.envelope,
            hop,
            ch,
        );

        // 自然延续 = 对齐点后第一个 hop（下一个窗该无缝接上的内容）。
        // 名义指针独立推进（对齐只在 ±search 内修相位）——若跟着对齐点走，
        // 恒定/周期素材会让搜索退化到同一边界值，名义位置定点不动，循环
        // 就转不出来了。
        self.natural = Some(self.queue[(p + hop) * ch..(p + hop * 2) * ch].to_vec());
        self.next_in += hop as f64 * rate;
    }
}

/// tail ← tail[hop..]（补零）+ window[hop..]，加权和与权重和同步滑动。
/// 独立成自由函数：window 借着 queue 的同时要可变借 tail，方法调用做不了
/// 这种字段级拆借。
fn shift_tail(
    tail: &mut Vec<f32>,
    tail_w: &mut Vec<f32>,
    window: &[f32],
    envelope: &[f32],
    hop: usize,
    ch: usize,
) {
    let overlap = window.len() / ch - hop;
    for v in [&mut *tail, &mut *tail_w] {
        v.drain(..hop * ch);
        v.resize(overlap * ch, 0.0);
    }
    for k in 0..overlap {
        let w_at = (k + hop) * ch;
        let env = envelope[k + hop];
        for c in 0..ch {
            tail[k * ch + c] += window[w_at + c] * env;
            tail_w[k * ch + c] += env;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(frames: usize, rate: u32) -> f64 {
        frames as f64 / f64::from(rate)
    }

    /// 静音进静音出：任意倍速下输出全是 0（相位搜索在静音上退化但不出杂波）。
    /// 素材刻意小：WSOLA 相关搜索在 debug 构建下慢，测试只验算法不变量。
    #[test]
    fn silence_in_silence_out() {
        let mut wsola = Wsola::new(48_000, 2);
        let mut out = Vec::new();
        let chunk = vec![0f32; 1024 * 2 * 4]; // 4 包立体声静音
        for _ in 0..8 {
            wsola.process(&chunk, 2.0, &mut out);
        }
        assert!(!out.is_empty());
        assert!(out.iter().all(|&s| s == 0.0), "静音输入不得产生非零输出");
    }

    /// 时长比 ≈ 1/rate（± 一个窗的流水线余量）。
    #[test]
    fn output_length_scales_with_rate() {
        let sr = 16_000u32; // 低采样率省算力，算法参数随采样率自适应
        let input_frames = (sr * 3) as usize; // 3s
        let input = vec![0.1f32; input_frames];
        for rate in [0.5f64, 1.0, 2.0] {
            let mut wsola = Wsola::new(sr, 1);
            let mut out = Vec::new();
            // 分包喂（贴近真实：每包 1024 帧）
            for chunk in input.chunks(1024) {
                wsola.process(chunk, rate, &mut out);
            }
            let got = secs(out.len(), sr);
            let want = 3.0 / rate;
            assert!(
                (got - want).abs() < 0.2,
                "rate={rate}: 输出 {got:.2}s，期望 {want:.2}s"
            );
        }
    }

    /// 确定性：同样输入输出逐位一致。
    #[test]
    fn deterministic() {
        let sr = 16_000u32;
        let input: Vec<f32> = (0..sr / 2).map(|i| (i as f32 * 0.01).sin()).collect();
        let run = || {
            let mut w = Wsola::new(sr, 1);
            let mut out = Vec::new();
            for chunk in input.chunks(1024) {
                w.process(chunk, 1.5, &mut out);
            }
            out
        };
        assert_eq!(run(), run());
    }
}

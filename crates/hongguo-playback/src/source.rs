//! 流的样本级读取：按样本表从 `serve()` 取明文字节。
//!
//! serve 是同步阻塞入口（未就绪的区间在条件变量上等，读者需求会驱动填充
//! 优先供这段），一律放 tokio 阻塞池。顺序读是常态，用「1MB 窗口」缓存摊薄
//! 调用次数：视频 1MB 够 1-2s，音频 1MB 够一分钟量级。

use std::sync::Arc;

use hongguo_common::{AppError, AppResult};
use hongguo_mp4::sample_table::TrackInfo;

/// 单次 serve 拉取的窗口大小（对齐填充的分块口径）。
const WINDOW: u64 = 1024 * 1024;

/// 流的静态信息（moov 解析产物）。
pub(crate) struct StreamMeta {
    pub video: TrackInfo,
    pub audio: Option<TrackInfo>,
    pub video_parameter_sets: Vec<Vec<u8>>,
    /// hvcC 原始载荷（Windows MF 的序列头形态；macOS 不用）。
    pub video_hvcc: Option<Vec<u8>>,
    /// 样本 NAL 长度前缀字节数（带内参数集前插用）。
    pub video_length_size: usize,
}

/// 从缓存流里读头部直到样本表完整（moov 落在头部，渐进注册已保证就绪）。
pub(crate) async fn open_stream(vid: &str, definition: u32) -> AppResult<StreamMeta> {
    let total = probe_total(vid, definition).await?;
    let mut head: Vec<u8> = Vec::new();
    let mut cursor = 0u64;
    for _ in 0..16 {
        // 16MB 封顶：moov 不可能这么大，超了说明流不对
        if cursor >= total {
            break;
        }
        let want = WINDOW.min(total - cursor);
        let chunk = read_range(
            vid.to_string(),
            definition,
            cursor,
            cursor + want - 1,
            || false,
        )
        .await?;
        if chunk.is_empty() {
            break;
        }
        cursor += chunk.len() as u64;
        head.extend_from_slice(&chunk);
        if let Ok(demuxed) = hongguo_media::demux::demux_bytes(&head)
            && let Some(video) = demuxed.video_track()
        {
            if hongguo_media::hevc::is_hevc(&video.info.codec) {
                let (nalus, length_size) =
                    hongguo_media::hevc::param_nalus_from_head(&head, &video.info)?;
                // hvcC 原始载荷尽力而为：Windows MF 走它做序列头，取不到
                // 时解码端有带内参数集兜底
                let hvcc = hongguo_media::hevc::hvcc_payload_from_head(&head, &video.info)
                    .map_err(|e| {
                        log::debug!("[Playback] hvcC 载荷取不到（走带内兜底）: {e}");
                        e
                    })
                    .ok();
                return Ok(StreamMeta {
                    audio: demuxed.audio_track().map(|t| t.info.clone()),
                    video_parameter_sets: nalus,
                    video_hvcc: hvcc,
                    video_length_size: length_size,
                    video: video.info.clone(),
                });
            }
            return Err(AppError::Media(format!(
                "播放引擎只接 HEVC 轨（codec = {}）",
                video.info.codec
            )));
        }
    }
    Err(AppError::Media("头部拿不到完整样本表（moov）".into()))
}

/// 文件总长：`bytes=0-0` 的 Content-Range 尾段。
async fn probe_total(vid: &str, definition: u32) -> AppResult<u64> {
    let (_, headers, _) = read_range_raw(vid.to_string(), definition, 0, 0, || false).await?;
    let cr = headers
        .iter()
        .find(|(k, _)| k == "Content-Range")
        .map(|(_, v)| v.clone())
        .ok_or_else(|| AppError::Media("取流响应没有 Content-Range".into()))?;
    cr.rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| AppError::Media(format!("Content-Range 无法解析: {cr}")))
}

/// 读样本 idx 的字节（窗口缓存：顺序读命中，乱序读自动重灌）。
pub(crate) struct SampleSource {
    vid: String,
    definition: u32,
    samples: Arc<Vec<(u64, u64)>>,
    buf: Vec<u8>,
    buf_at: u64,
}

impl SampleSource {
    pub(crate) fn new(vid: &str, definition: u32, samples: Arc<Vec<(u64, u64)>>) -> Self {
        Self {
            vid: vid.to_string(),
            definition,
            samples,
            buf: Vec::new(),
            buf_at: u64::MAX,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.samples.len()
    }

    /// 读样本 idx 的字节；`cancelled` 命中时，等待中的 serve 会被尽快唤醒
    /// 退场（注销读者登记），本调用以错误返回。
    ///
    /// seek 之后还堵在旧位置上的读取必须主动让路——干等旧数据的话，填充
    /// 调度器会先满足旧需求（FIFO），新位置的 seek 就绪被白白拖后一个区间。
    pub(crate) async fn read_cancellable(
        &mut self,
        idx: usize,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> AppResult<Vec<u8>> {
        self.read_inner(idx, cancelled).await
    }

    async fn read_inner(
        &mut self,
        idx: usize,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> AppResult<Vec<u8>> {
        let (offset, size) = *self
            .samples
            .get(idx)
            .ok_or_else(|| AppError::Media(format!("样本号越界: {idx}")))?;
        if size == 0 {
            return Ok(Vec::new());
        }
        let end = offset + size; // 开区间
        if !(offset >= self.buf_at && end <= self.buf_at + self.buf.len() as u64) {
            self.buf_at = offset;
            self.buf = read_range(
                self.vid.clone(),
                self.definition,
                offset,
                offset + WINDOW - 1,
                cancelled,
            )
            .await?;
        }
        let start = (offset - self.buf_at) as usize;
        self.buf
            .get(start..start + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| AppError::Media(format!("样本 {idx} 越出读窗口")))
    }
}

/// 取一段明文字节（serve 阻塞，放阻塞池；`cancelled` 透传给 serve，
/// seek 重定位时等待中的请求能尽快退场并注销读者登记）。
async fn read_range(
    vid: String,
    definition: u32,
    start: u64,
    end: u64,
    cancelled: impl Fn() -> bool + Send + Sync + 'static,
) -> AppResult<Vec<u8>> {
    let (status, _, body) = read_range_raw(vid, definition, start, end, cancelled).await?;
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(AppError::Media(format!(
            "取流区间 {start}-{end} 失败（HTTP {status}）"
        )))
    }
}

async fn read_range_raw(
    vid: String,
    definition: u32,
    start: u64,
    end: u64,
    cancelled: impl Fn() -> bool + Send + Sync + 'static,
) -> AppResult<hongguo_stream::range::ProtocolResponse> {
    let header = format!("bytes={start}-{end}");
    tokio::task::spawn_blocking(move || {
        hongguo_stream::serve_cancellable(&vid, definition, Some(&header), cancelled)
            .map_err(|e| AppError::Media(format!("取流失败: {e}")))
    })
    .await
    .map_err(|e| AppError::Internal(format!("阻塞池 join: {e}")))?
}

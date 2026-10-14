//! 弹幕表情：`[名字]` 代码 → 打包 webp 的解码与渲染分段。
//!
//! 上游文本里表情以 `[名字]` 代码出现（hgplayer 1.1.6 同款，1–8 个非空白
//! 字符），渲染时按映射表换打包的 webp 图——弹幕/评论同源。webp 解码用
//! image crate（feature "webp" 已启用），首次用到才解码并缓存
//! （`RenderImage` 构造走 [`crate::image_upload::render_image`] 唯一入口）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::RenderImage;

pub(crate) use crate::emoji_files::EMOJI_FILES;

/// 一段解析结果：纯文本或一张表情图。
pub(crate) enum EmojiSegment {
    Text(String),
    Emoji(Arc<RenderImage>),
}

/// 名字 → 解码后的 RenderImage 缓存（首次用到才解码；毒锁恢复为空表——
/// 解码失败重试代价只是一次 webp 解码）。名字先归一到静态表的 `'static`
/// 条目，缓存键才不借运行时字符串。表情选择器网格同源复用。
pub(crate) fn emoji_by_name(name: &str) -> Option<Arc<RenderImage>> {
    type EmojiCache = HashMap<&'static str, Option<Arc<RenderImage>>>;
    static CACHE: OnceLock<Mutex<EmojiCache>> = OnceLock::new();
    let static_name = EMOJI_FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(n, _)| *n)?;
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(hit) = guard.get(static_name) {
        return hit.clone();
    }
    let decoded = EMOJI_FILES
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, bytes)| image::load_from_memory(bytes).ok())
        .map(image::DynamicImage::into_rgba8)
        .map(crate::image_upload::render_image)
        .map(Arc::new);
    guard.insert(static_name, decoded.clone());
    decoded
}

/// 文本 → 段序列：认识的 `[名字]` 换表情，不认识的原样保留为文本。
/// 手写扫描（不引 regex 依赖）：`[` 起始、1–8 个非空白且不含 `]`/`[`、
/// `]` 收尾，且整个代码在映射表里才认。
pub(crate) fn parse_emoji_segments(text: &str) -> Vec<EmojiSegment> {
    let bytes = text.as_bytes();
    let mut segments = Vec::new();
    let mut cursor = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        // 找收尾 `]`：中间 1–8 个非空白且不含 `[`/`]`
        let mut end = None;
        for (j, b) in bytes
            .iter()
            .enumerate()
            .take(bytes.len().min(i + 10))
            .skip(i + 1)
        {
            match b {
                b']' => {
                    end = Some(j);
                    break;
                }
                b'[' | b' ' | b'\t' => break,
                _ => {}
            }
        }
        let Some(close) = end else {
            i += 1;
            continue;
        };
        let name = &text[i..=close];
        let Some(img) = emoji_by_name(name) else {
            i += 1;
            continue;
        };
        if i > cursor {
            segments.push(EmojiSegment::Text(text[cursor..i].to_string()));
        }
        segments.push(EmojiSegment::Emoji(img));
        i = close + 1;
        cursor = i;
    }
    if cursor < text.len() {
        segments.push(EmojiSegment::Text(text[cursor..].to_string()));
    }
    segments
}

/// 段宽度估算：文本按 CJK/非 CJK 估宽，表情按 1.3 字号（hgplayer 同款）。
pub(crate) fn segment_widths(text: &str, font_px: f32) -> f32 {
    parse_emoji_segments(text)
        .iter()
        .map(|seg| match seg {
            EmojiSegment::Text(t) => {
                t.chars()
                    .map(|c| if is_wide_char(c) { 1.0 } else { 0.55 })
                    .sum::<f32>()
                    * font_px
            }
            EmojiSegment::Emoji(..) => font_px * 1.3,
        })
        .sum()
}

/// 宽字符判定（CJK/谚文/全形；估宽够用，不引 unicode-width 依赖——
/// gpui 依赖树里它与 unicode-properties 的版本钉子有过冲突史）。
pub(crate) fn is_wide_char(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x2FFFD
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 认识的 `[名字]` 换表情段，文本段保留；不认识的代码原样留在文本里。
    #[test]
    fn parse_segments_splits_known_emoji() {
        let segs = parse_emoji_segments("来了[微笑]");
        assert_eq!(segs.len(), 2, "文本段 + 表情段");
        assert!(matches!(&segs[0], EmojiSegment::Text(t) if t == "来了"));
        assert!(matches!(&segs[1], EmojiSegment::Emoji(_)));

        // 映射表里没有的代码：原样保留为文本（不产表情段）
        let segs = parse_emoji_segments("[不存在的表情]");
        assert!(
            segs.iter()
                .all(|s| matches!(s, EmojiSegment::Text(t) if t.contains("[不存在的表情]"))),
            "未知代码应留在文本段"
        );
    }

    /// webp 资产可解码成 RGBA 位图（首项 [微笑] 即 e01.webp）。
    #[test]
    fn emoji_webp_decodes() {
        assert!(emoji_by_name("[微笑]").is_some(), "映射表首项应可解码");
    }

    /// 超长/含空白的伪代码不认（hgplayer 同款 1–8 非空白字符约束）。
    #[test]
    fn scanner_ignores_non_code_brackets() {
        for text in [
            "[这是 一段很长 很长 很长很长 很长很长的 文本内容]",
            "a [b c] d",
        ] {
            assert!(
                parse_emoji_segments(text)
                    .iter()
                    .all(|s| matches!(s, EmojiSegment::Text(_))),
                "{text} 不应产生表情段"
            );
        }
    }
}

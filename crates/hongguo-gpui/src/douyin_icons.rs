//! 抖音系实心互动图标（收藏星/评论气泡/点赞心/分享箭头）。
//!
//! Tauri `interaction-rail.tsx` 的自绘 SVG 1:1 移植：lucide 线框形态不像
//! 抖音那套实心胖图标。`fill/stroke` 全走 `currentColor`，激活态由外层
//! `text_color` 接管（红心/黄星）。渲染走 `Icon::default().data()`——
//! 内嵌字节不经 AssetSource，与 assets 目录解耦。
//!
//! 红色激活是 hgplayer 皮肤（硬红线 11 的播放器豁免范围）。

use gpui_kit::component::Icon;

const DOUYIN_STAR: &[u8] = br#"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
  <path d="M12 17.27L18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z" fill="currentColor" stroke="currentColor" stroke-width="2.2" stroke-linejoin="round"/>
</svg>"#;

/// 圆气泡撑满画幅 + 左下甩尾，三个圆点 evenodd 挖空（露出画面）。
const DOUYIN_COMMENT: &[u8] = br#"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
  <path fill-rule="evenodd" clip-rule="evenodd" d="M12 2.4C17.7 2.4 22.2 6.1 22.2 10.8C22.2 15.5 17.7 19.2 12 19.2C11.3 19.2 10.62 19.15 9.96 19.04C8.7 20 6.7 20.95 4.3 21.17C3.83 21.21 3.6 20.62 3.95 20.29C4.85 19.44 5.4 18.32 5.55 17.26C3.2 15.84 1.8 13.6 1.8 10.8C1.8 6.1 6.3 2.4 12 2.4ZM7.2 9.3A1.5 1.5 0 1 0 7.2 12.3A1.5 1.5 0 0 0 7.2 9.3ZM12 9.3A1.5 1.5 0 1 0 12 12.3A1.5 1.5 0 0 0 12 9.3ZM16.8 9.3A1.5 1.5 0 1 0 16.8 12.3A1.5 1.5 0 0 0 16.8 9.3Z" fill="currentColor"/>
</svg>"#;

const DOUYIN_HEART: &[u8] = br#"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
  <path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z" fill="currentColor"/>
</svg>"#;

/// 实心前转箭头（转发形态），描边圆角软化箭头尖角。
const DOUYIN_SHARE: &[u8] = br#"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
  <path d="M14 9V5l7 7-7 7v-4.1c-5 0-8.5 1.6-11 5.1 1-5 4-10 11-11z" fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>
</svg>"#;

/// 收藏（星）。
pub(crate) fn star() -> Icon {
    Icon::default().data(DOUYIN_STAR)
}

/// 评论（气泡）。
pub(crate) fn comment() -> Icon {
    Icon::default().data(DOUYIN_COMMENT)
}

/// 点赞（心）。
pub(crate) fn heart() -> Icon {
    Icon::default().data(DOUYIN_HEART)
}

/// 分享（前转箭头）。
pub(crate) fn share() -> Icon {
    Icon::default().data(DOUYIN_SHARE)
}

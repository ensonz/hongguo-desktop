//! `image` crate 的 RGBA 位图 → gpui [`RenderImage`] 的唯一入口（**必须在此转
//! BGRA**）。
//!
//! gpui 的 `RenderImage` 约定帧缓冲是 **BGRA**：gpui-pre `assets.rs` 的
//! `RenderImage` 头注释写着「A cached and processed image, in BGRA format」，
//! 它自带的图片加载路径（`elements/img.rs`）解码后也都显式把 RGBA 逐像素
//! 交换成 BGRA 才交给 `RenderImage::new`（`RenderImage::new` 自身不做转换）。
//!
//! 本壳手工造帧的地方有三处——播放的 NV12→RGBA 帧、首页占位封面、排行榜
//! 封面——都走这一个函数：同一段转换只有一个归属。别在调用点各自交换，
//! 也别把交换下移到解码/转换层（那是同一个错换个位置）。
//!
//! 两层护栏盯着这两条：交换本身是 [`hongguo_media::bgra::swap_red_blue_in_place`]
//! （有单测），「只有这里能造 `RenderImage`」由 `tests/render_image_guard.rs`
//! 扫源码盯着。

use gpui_kit::RenderImage;

/// RGBA 位图 → 可上屏的 `RenderImage`（就地交换成 BGRA）。
pub(crate) fn render_image(mut rgba: image::RgbaImage) -> RenderImage {
    let bytes: &mut [u8] = rgba.as_mut();
    hongguo_media::bgra::swap_red_blue_in_place(bytes);
    RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])
}

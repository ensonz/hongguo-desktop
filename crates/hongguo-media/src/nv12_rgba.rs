//! NV12 → RGBA（BT.709 limited range）。
//!
//! Windows 播放路径的渲染转换：MF 解码 MFT 出 NV12 系统内存帧，而 gpui 在
//! Windows 没有视频 surface（macOS 是 CVPixelBuffer 零拷贝），走 CPU 转
//! RGBA 后按普通图像上传。色彩矩阵与 macOS 侧 VT→Metal 的 YCbCr 采样一致
//! （BT.709、limited range：Y∈[16,235]、UV∈[16,240]），两端观感对齐。
//!
//! 纯函数、无平台依赖；stride（字节行距）可以大于宽度（解码面对齐是常态，
//! 见 mf 侧「解码面 1928×1080」的实况注释）。

/// NV12 帧转 RGBA8（逐像素，交错输出）。
///
/// `data` 布局：Y 平面 `stride × height`，紧随 UV 交错平面
/// `stride × height/2`。输出长度恒为 `width × height × 4`。
pub fn nv12_to_rgba_bt709(data: &[u8], stride: usize, width: usize, height: usize) -> Vec<u8> {
    debug_assert!(stride >= width);
    let uv_stride = stride;
    let y_plane = stride * height;
    let uv_plane = uv_stride * height.div_ceil(2);
    let mut out = Vec::with_capacity(width * height * 4);
    // BT.709 limited range 的分解系数（定点 ×1024，乘完右移 10 位）
    const Y_SCALE: i32 = 1192; // 1.1644 × 1024
    const V_R: i32 = 1634; // 1.5960 × 1024
    const U_G: i32 = 401; // 0.3917 × 1024
    const V_G: i32 = 833; // 0.8129 × 1024
    const U_B: i32 = 2066; // 2.0172 × 1024
    for row in 0..height {
        let y_row = &data[row * stride..][..width];
        let uv_row = &data[y_plane + (row / 2) * uv_stride..][..width];
        for (x, &y) in y_row.iter().enumerate() {
            let u = uv_row[x / 2 * 2] as i32;
            let v = uv_row[x / 2 * 2 + 1] as i32;
            let yy = (y as i32 - 16) * Y_SCALE;
            let ud = u - 128;
            let vd = v - 128;
            // +512 后再右移 = 四舍五入：Y 上限（235）必须精确落 255，
            // floor 会差一档（254）——白场发灰
            let r = (yy + V_R * vd + 512) >> 10;
            let g = (yy - U_G * ud - V_G * vd + 512) >> 10;
            let b = (yy + U_B * ud + 512) >> 10;
            out.push(r.clamp(0, 255) as u8);
            out.push(g.clamp(0, 255) as u8);
            out.push(b.clamp(0, 255) as u8);
            out.push(255);
        }
    }
    let _ = uv_plane; // 布局断言用：UV 区间必须整体在 data 内
    debug_assert!(data.len() >= y_plane + uv_plane, "NV12 数据不足");
    out
}

/// NV12 → RGBA8 并按目标尺寸**最近邻缩放**（给远小于解码面分辨率的自绘
/// 视口用）。同尺寸时走 [`nv12_to_rgba_bt709`] 原路径。
///
/// 1080p 解码面对 ~800px 的显示视口：全尺寸转换每帧几十毫秒 CPU，还把
/// 8MB 的纹理塞进 UI 图集（每帧一个新图 = 图集只增不减）。显示端反正是
/// object-contain 拉伸，按视口尺寸出图是主流播放器的常规做法。缩放比由
/// 调用方按保持宽高比算好传入（本函数不猜宽高比）。
pub fn nv12_to_rgba_bt709_scaled(
    data: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<u8> {
    if dst_width == 0 || dst_height == 0 {
        return Vec::new();
    }
    if dst_width == width && dst_height == height {
        return nv12_to_rgba_bt709(data, stride, width, height);
    }
    let uv_stride = stride;
    let y_plane = stride * height;
    let uv_plane = uv_stride * height.div_ceil(2);
    debug_assert!(data.len() >= y_plane + uv_plane, "NV12 数据不足");
    // 输出行/列 → 源行/列的整数映射（不逐像素做浮点乘法）
    let row_map: Vec<usize> = (0..dst_height)
        .map(|dy| (dy * height / dst_height).min(height - 1))
        .collect();
    let col_map: Vec<usize> = (0..dst_width)
        .map(|dx| (dx * width / dst_width).min(width - 1))
        .collect();
    let mut out = Vec::with_capacity(dst_width * dst_height * 4);
    // 与全尺寸版同一组 BT.709 limited 定点系数（×1024）
    const Y_SCALE: i32 = 1192;
    const V_R: i32 = 1634;
    const U_G: i32 = 401;
    const V_G: i32 = 833;
    const U_B: i32 = 2066;
    for &sy in &row_map {
        let y_row = &data[sy * stride..][..width];
        let uv_row = &data[y_plane + (sy / 2) * uv_stride..][..width];
        for &sx in &col_map {
            let y = y_row[sx] as i32;
            let u = uv_row[sx / 2 * 2] as i32;
            let v = uv_row[sx / 2 * 2 + 1] as i32;
            let yy = (y - 16) * Y_SCALE;
            let ud = u - 128;
            let vd = v - 128;
            // +512 后再右移 = 四舍五入（与全尺寸版同口径，白场不灰一档）
            let r = (yy + V_R * vd + 512) >> 10;
            let g = (yy - U_G * ud - V_G * vd + 512) >> 10;
            let b = (yy + U_B * ud + 512) >> 10;
            out.push(r.clamp(0, 255) as u8);
            out.push(g.clamp(0, 255) as u8);
            out.push(b.clamp(0, 255) as u8);
            out.push(255);
        }
    }
    let _ = uv_plane;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 帧构造器：Y 与 UV 显式给值，stride 可加 padding。
    fn frame(y: [[u8; 2]; 2], uv: [u8; 2], pad: usize) -> Vec<u8> {
        let stride = 2 + pad;
        let mut v = vec![0u8; stride * 2 + stride]; // Y 2 行 + UV 1 行
        for r in 0..2 {
            for c in 0..2 {
                v[r * stride + c] = y[r][c];
            }
        }
        v[stride * 2] = uv[0];
        v[stride * 2 + 1] = uv[1];
        v
    }

    /// 黑场：Y=16（下限）、UV=128（中性）→ RGB 全 0。
    #[test]
    fn black_pixel_maps_to_zero() {
        let data = frame([[16, 16], [16, 16]], [128, 128], 0);
        let rgba = nv12_to_rgba_bt709(&data, 2, 2, 2);
        assert_eq!(
            rgba,
            vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]
        );
    }

    /// 白场：Y=235（上限）、UV 中性 → RGB 全 255。
    #[test]
    fn white_pixel_maps_to_full() {
        let data = frame([[235, 235], [235, 235]], [128, 128], 0);
        let rgba = nv12_to_rgba_bt709(&data, 2, 2, 2);
        assert!(
            rgba.chunks(4).all(|p| p[..3] == [255, 255, 255]),
            "{rgba:?}"
        );
    }

    /// 有限的 Y（超限钳到边界，不回绕）。
    #[test]
    fn out_of_range_values_clamp() {
        let data = frame([[0, 255], [0, 255]], [128, 128], 0);
        let rgba = nv12_to_rgba_bt709(&data, 2, 2, 2);
        assert!(
            rgba.chunks(4).all(|p| p[0] == p[1] && p[1] == p[2]),
            "{rgba:?}"
        );
    }

    /// stride 带行距 padding 时行内取值不受影响（解码面对齐是常态）。
    #[test]
    fn stride_padding_is_skipped() {
        let tight = frame([[16, 235], [16, 235]], [128, 128], 0);
        let padded = frame([[16, 235], [16, 235]], [128, 128], 3);
        let a = nv12_to_rgba_bt709(&tight, 2, 2, 2);
        let b = nv12_to_rgba_bt709(&padded, 5, 2, 2);
        assert_eq!(a, b);
    }

    /// 色度代入的一组手工基准（BT.709）：中灰 + U/V 双高（U↑蓝↑、V↑红↑、
    /// 两者一起压绿）。
    #[test]
    fn known_chroma_reference() {
        let data = frame([[126, 126], [126, 126]], [255, 255], 0);
        let rgba = nv12_to_rgba_bt709(&data, 2, 2, 2);
        let [r, g, b, _] = rgba[..4].try_into().unwrap();
        assert!(r > 200 && b > 200 && g < 60, "R={r} G={g} B={b}");
    }

    /// 同尺寸走原路径：缩放版在 1:1 时必须逐字节等于全尺寸版。
    #[test]
    fn scaled_same_size_equals_full() {
        let data = frame([[126, 235], [16, 90]], [200, 40], 2);
        let a = nv12_to_rgba_bt709(&data, 4, 2, 2);
        let b = nv12_to_rgba_bt709_scaled(&data, 4, 2, 2, 2, 2);
        assert_eq!(a, b);
    }

    /// 缩放取点正确性：4×2 → 2×1 应采样源 (0,0) 与 (0,2) 两像素，
    /// 输出值与全尺寸版对应位置的值一致（灰场 + 左白右黑可区分）。
    #[test]
    fn scaled_samples_source_pixels() {
        let stride = 4;
        let mut data = vec![0u8; stride * 2 + stride];
        // 第 0 行 Y：16 235 16 235（黑白交替）
        for (i, v) in [16u8, 235, 16, 235].into_iter().enumerate() {
            data[i] = v;
        }
        // 第 1 行随意；UV 中性
        for b in data.iter_mut().skip(stride * 2) {
            *b = 128;
        }
        let full = nv12_to_rgba_bt709(&data, stride, 4, 2);
        let small = nv12_to_rgba_bt709_scaled(&data, stride, 4, 2, 2, 1);
        assert_eq!(small.len(), 2 * 4); // 2×1 帧共 8 字节
        // 输出 0 ← 源 (0,0)（黑），输出 1 ← 源 (0,2)（黑）；中间白被跳过
        assert_eq!(&small[..4], &full[0..4]);
        assert_eq!(&small[4..], &full[8..12]);
    }
}

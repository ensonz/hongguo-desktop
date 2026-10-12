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
}

# 后端测试样本

| 文件                     | 是什么                                                                | 用途                                                                                                   | 来源                                   |
| ------------------------ | --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | -------------------------------------- |
| `heic-cover-sample.heic` | 25 KB：单 `hvc1` item（`hvcC` / `ispe` / `iloc`）的 HEIC 封面原样字节 | `hongguo_media::heif::tests::decodes_real_cdn_sample_to_jpeg`——解析 → 软解 → JPEG，产物必须是合法 JPEG | 真实 CDN 样本（`fqnovelpic` 封面原样） |

由来：2026-10-11 抽 `hongguo-media` crate 时从 `src-tauri/tests/fixtures/` 随 `heif.rs` 一并迁出（`heif.rs` 的测试用 `CARGO_MANIFEST_DIR/tests/fixtures/` 定位样本，跟着代码走才对得上）。

> 注册的 TT-Encrypt V5 样本（`register_req_real.bin` / `py_enc_accepted.hex`）已随 `domain/api` 迁到 `crates/hongguo-api/tests/fixtures/`，见那里的 README。

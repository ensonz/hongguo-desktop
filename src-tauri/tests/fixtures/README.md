# 后端测试样本

| 文件                     | 是什么                                                                | 用途                                                                                           | 来源                                   |
| ------------------------ | --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | -------------------------------------- |
| `heic-cover-sample.heic` | 25 KB：单 `hvc1` item（`hvcC` / `ispe` / `iloc`）的 HEIC 封面原样字节 | `media::heif::tests::decodes_real_cdn_sample_to_jpeg`——解析 → 软解 → JPEG，产物必须是合法 JPEG | 真实 CDN 样本（`fqnovelpic` 封面原样） |

## 迁移提示

`heic-cover-sample.heic` 属 `media` 域，抽 `hongguo-media` crate 时随其迁出，届时本目录及本表一并处理。

> 注册的 TT-Encrypt V5 样本（`register_req_real.bin` / `py_enc_accepted.hex`）已随 `domain/api` 迁到 `crates/hongguo-api/tests/fixtures/`，见那里的 README。

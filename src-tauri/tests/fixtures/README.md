# 后端测试样本

| 文件                     | 是什么                                                                | 用途                                                                                                                                             | 来源                                                  |
| ------------------------ | --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------- |
| `register_req_real.bin`  | 1046 字节：真机注册请求的完整 body（TT-Encrypt V5 密文）              | `register::tests::tt_v5_real_capture_roundtrip`——解密 → 同盐重加密 → 结构性复现，锁死派生链 / AES-CBC / PKCS7 / gzip 字节流                      | 2026-10-03 真机抓包                                   |
| `py_enc_accepted.hex`    | 2060 字符 hex（= 1030 字节）：**服务端发号成功**的注册 body           | `register::tests::tt_v5_reproduces_accepted_body`——同 (gz, salt, iv) 重加密必须逐字节相等，锁死 hash 取窗（`digest[16..64]`）/ 拼接 / 填充全链路 | 2026-10-03 服务端接受的形态，由 Python 逆向实现预生成 |
| `heic-cover-sample.heic` | 25 KB：单 `hvc1` item（`hvcC` / `ispe` / `iloc`）的 HEIC 封面原样字节 | `media::heif::tests::decodes_real_cdn_sample_to_jpeg`——解析 → 软解 → JPEG，产物必须是合法 JPEG                                                   | 真实 CDN 样本（`fqnovelpic` 封面原样）                |

## 迁移提示

`heic-cover-sample.heic` 属 `media` 域，抽 `hongguo-media` crate 时随其迁出，届时本表需同步（`register_*` / `py_enc_accepted` 属 `domain/api` 域，随 `hongguo-api` 走）。

## 相关

- `register` 两个样本保护的算法（TTEncrypt V5）与 signer 的 `tt_hash.rs` 同源，见 `crates/hongguo-signer/tests/fixtures/README.md`。

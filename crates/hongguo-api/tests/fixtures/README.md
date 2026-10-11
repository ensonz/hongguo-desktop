# hongguo-api 测试样本

| 文件                    | 是什么                                                      | 用途                                                                                                                                             | 来源                                                  |
| ----------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------- |
| `register_req_real.bin` | 1046 字节：真机注册请求的完整 body（TT-Encrypt V5 密文）    | `register::tests::tt_v5_real_capture_roundtrip`——解密 → 同盐重加密 → 结构性复现，锁死派生链 / AES-CBC / PKCS7 / gzip 字节流                      | 2026-10-03 真机抓包                                   |
| `py_enc_accepted.hex`   | 2060 字符 hex（= 1030 字节）：**服务端发号成功**的注册 body | `register::tests::tt_v5_reproduces_accepted_body`——同 (gz, salt, iv) 重加密必须逐字节相等，锁死 hash 取窗（`digest[16..64]`）/ 拼接 / 填充全链路 | 2026-10-03 服务端接受的形态，由 Python 逆向实现预生成 |

## 相关

- 这两个样本保护的算法（TTEncrypt V5）与 `hongguo-signer` 的 `tt_hash.rs` 同源，见 `crates/hongguo-signer/tests/fixtures/README.md`。
- `interact` 的运行时探测用例要读 `.hg-runtime-settings.json`，约定放本 crate 根（`crates/hongguo-api/`）——见 `src/interact/mod.rs` 的 `runtime_env` 注释。

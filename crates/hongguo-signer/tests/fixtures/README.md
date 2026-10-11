# signer 测试样本

| 文件        | 是什么                                        | 用途                                               |
| ----------- | --------------------------------------------- | -------------------------------------------------- |
| `py_gz.hex` | 924 字节 gzip 数据的 hex 文本（`1f8b…` 开头） | `tt_hash::tests::calculate_multi_block_924` 的输入 |

## `py_gz.hex` 的细节

- **格式**：单行 hex，无空白；`1f8b` 即 gzip 魔数。
- **为什么是 924 字节**：`TtHashCore::calculate` 按 128 字节分块，924 字节 = 7 块整 + 余量，**专门压在跨块状态链接这条路径上**——单块向量测不到它。
- **它保护什么**：`tt_hash.rs` 是 TTEncrypt V5（设备注册 body 加密）的核心算法。算错一个 bit 的后果是服务端静默拒绝（HTTP 200 + 0 字节 / `device_id=0`），所以这条路径必须有真实输入的端到端对拍。
- **别删**：真实 gz 输入的 golden 对拍物证（期望值已随向量测试固化）。

## `tt_hash.rs` 的事实源与正确性锚

`tt_hash.rs` 历史上由一次性脚本从 Python 参考实现机械翻译而来；参考实现（临时文件）已遗失、生成脚本已移除，**`tt_hash.rs` 本身就是唯一事实源**，按普通 Rust 代码维护（没有「重新生成」一说），改动由以下锚兜底：

- `tt_hash.rs` 内的 `calculate_*` 向量测试（含 len=127 的魔改填充分支 golden 与共享/独立核等价测试）；
- 本目录 `py_gz.hex` 的跨块对拍；
- register 域两条真实抓包字节级回归（`crates/hongguo-api/tests/fixtures/` 的 `register_req_real.bin` 与 `py_enc_accepted.hex`，后者是服务端发号成功样本的逐字节复现锁）。

算法考证（2026-10-11，详见 `tt_hash.rs` 模块文档）：K/IV 与标准 SHA-512 常量逐值一致；`len % 128 <= 110` 时 `calculate` 与 SHA-512 逐字节相同，`>= 111` 的填充分支与标准分叉（len=127 golden 锁定）；`TT_ORD_LIST` 在官方 APK 的 `lib/arm64-v8a/libvideodec.so` 偏移 0x5220 原样存在（全 APK 唯一命中）。

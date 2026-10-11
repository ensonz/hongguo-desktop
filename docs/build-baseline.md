# 编译基线（拆分前后对照）

> 采集于 **2026-10-11**，commit `5bf5697`，即「阶段 2 第 2 条（hongguo-store）」合并之后、
> **第 3 条（hongguo-stream / hongguo-media / hongguo-media-platform）动手之前**。
>
> 存在意义：`docs/architecture-refactor.md` §11 把「拆分后编译未必变快」列为头号风险，
> 要求先测基线。没有本表，后面任何「更快了 / 更慢了」都是无据之谈。

## 环境

| 项 | 值 |
| --- | --- |
| OS / 工具链 | Windows (win32) / rustc 1.99.0（stable-x86_64-pc-windows-msvc） |
| CPU | 13th Gen Intel Core i5-13400（16 逻辑核） |
| profile | `dev`（`unoptimized + debuginfo`） |
| 后端 crate 数 | 6（common / signer / mp4 / api / store / desktop） |
| 后端代码量 | `src-tauri/src` 19,758 行 + `crates/` 8 crate |

## 1. 整包 clean build（dev）

```
cargo clean && cargo build --workspace
```

| 指标 | 值 |
| --- | --- |
| 总耗时（wall clock） | **208.8 s（3 分 29 秒）** |
| cargo 自报 | `Finished dev profile in 3m 28s` |
| `cargo clean` 释放 | 37,726 文件 / 50.0 GiB |
| 构建后 `target/` 体积 | 15.06 GB |

## 2. 单文件增量（改一个文件 → 重建）

预热后，`touch` 单个源文件，分别跑 `cargo check --workspace` 与 `cargo build --workspace`。

| 探针 | 改哪个文件 | check | build | 被重建的 hongguo crate |
| --- | --- | --- | --- | --- |
| B1 | `src-tauri/src/media/transcode.rs` | 1.4 s | **18.6 s** | `hongguo-desktop` |
| B2 | `crates/hongguo-store/src/paths.rs` | 1.7 s | 18.4 s | `hongguo-store`, `hongguo-desktop` |
| B3 | `crates/hongguo-mp4/src/lib.rs` | 1.5 s | 17.8 s | `hongguo-mp4`, `hongguo-desktop` |

### 读数结论（重要，别误读）

1. **增量成本的绝对大头是 `src-tauri` 二进制的链接**（≈18 s）。改哪个 crate 的文件都要重链
   `hongguo-desktop`，所以**拆分不会让「改一行代码」的等待变短**——这正是 §11 预判的风险，
   基线把这件事先钉死，省得后面拿它当拆分的功劳或罪状。
2. `cargo check` 只有 1.4~1.7 s：类型检查本身很便宜，被改动的 trait/签名越多才越贵。
3. 拆分的真实收益在三处，后续对照只看这三处：
   - `cargo check -p <crate>`：单 crate 焦点工作的反馈时延；
   - 一次改动的**检查范围**（同一个文件改动波及多少 crate）；
   - 独立 crate 之间不再互相失效（改 `media` 不该让 `store` 重编）。

## 3. 复测口径（每拆一个 crate 后重跑同样三项）

```powershell
# 环境（本机 rustup home 与默认 toolchain 不在默认位置）
$env:RUSTUP_HOME='C:\Users\liu13\.rustup'; $env:CARGO_HOME='C:\Users\liu13\.cargo'

# 1) 整包 clean build
cargo clean; cargo build --workspace

# 2) 单文件增量（换 $file 为对应探针）
(Get-Item $file).LastWriteTime = Get-Date
cargo check --workspace
cargo build --workspace
```

**对照原则**：条目只与本表同口径数字比，且必须注明当时的 crate 数与探针文件——
探针文件在拆分后会换到新 crate 里的同一逻辑位置（例如 media 拆出后，B1 探针改用
`crates/hongguo-media/src/transcode.rs`）。

## 4. 验收指标（拆分是否「值」）

| 指标 | 基线 | 期望 |
| --- | --- | --- |
| B1 check（改 media 一个文件） | 1.4 s | 不劣于基线（拆出后不应变慢） |
| B1 build | 18.6 s | 由链接主导，预期持平 |
| clean build | 208.8 s | 允许变差 ≤10%，超出需给出原因 |
| 跨 crate 误伤 | media 改动会重建整个 `hongguo-desktop`（无法避免） | 不再额外失效无关的叶子 crate |

## 5. 复测记录

### 3a `hongguo-stream` 之后（2026-10-11）

| 探针 | check | build | 重建的 hongguo crate |
| --- | --- | --- | --- |
| A1 `crates/hongguo-stream/src/cache.rs` | 1.4 s | 18.7 s | `hongguo-stream`, `hongguo-desktop` |
| A2 `src-tauri/src/service/play_service/online.rs` | 1.4 s | 17.0 s | `hongguo-desktop` |
| A3 `crates/hongguo-stream/src/range.rs` | 1.4 s | 18.4 s | `hongguo-stream`, `hongguo-desktop` |

**结论：与基线持平。** check 稳定在 1.4 s，build 17~18.7 s（基线 17.8~18.6 s）——
证实了「增量成本由 `hongguo-desktop` 的链接主导、拆分不会让单文件改动变快」。
拆分拿到的不是秒数，而是**职责边界**：`serve()` 不再反向依赖 service，
`hongguo-stream` 可以脱离 tauri 单独 `cargo check -p hongguo-stream`。
⚠️ 注意：目标 crate 首次参与构建那一次会显著更慢（A1 首轮 52.7 s，含新 crate
全量 codegen + 重链），复测要等预热后再读数。

### 3b `hongguo-media` 之后（2026-10-11）

| 探针 | check | build | 重建的 hongguo crate |
| --- | --- | --- | --- |
| B1 `crates/hongguo-media/src/transcode.rs` | 1.5 s | **26.0 s** | `hongguo-media`, `hongguo-desktop` |
| B2 `src-tauri/src/service/merge_service/compat.rs` | 1.4 s | 19.1 s | `hongguo-desktop` |

**结论：改 media 文件的增量 build 变慢了（18.6 s → 26 s），这是拆分的真实代价。**

原因很直白：改动落在 media 上时，现在要多付一次 `hongguo-media` 的 codegen；而下游
`hongguo-desktop` 无论改哪边都要整体重编 + 重链——Cargo 的跨 crate 泛型实例化不会因为
「代码搬进另一个 crate」就少做一遍。check 不受影响（1.4 s 持平），改壳文件（B2）也持平。

**换来的是边界，不是秒数**：

- 改 media 与改壳不再扩大对方的检查范围；`cargo check -p hongguo-media` 可脱离 tauri 单跑；
- 壳的依赖图变薄——`rusty_h265` / `rusty_h264` / `muxide` / `sysinfo` 四个重型依赖
  随编解码核心移出，只有 media 需要它们；
- `hongguo-media` 的 76 个用例可以在不编 tauri 的前提下跑完（1.1 s）。

> 教训：**如果哪天要拿「编译更快」当拆分的理由，这张表就是反例。** 拆分的正当理由
> 始终是职责边界与可复用性（GPUI 壳要单独用 media），不是构建时间。

### 3c `hongguo-media-platform` 之后（2026-10-12）

| 探针 | check | build | 重建的 hongguo crate |
| --- | --- | --- | --- |
| P1 `crates/hongguo-media-platform/src/capability/probe.rs` | 1.4 s | **21.5 s** | `hongguo-media-platform`, `hongguo-desktop` |
| P2 `src-tauri/src/service/play_service/online.rs` | 1.3 s | 16.0 s | `hongguo-desktop` |

**结论：与 3b 同形态。** 平台层文件改动 18.6 s → 21.5 s（多付一次
`hongguo-media-platform` 的 codegen，涨幅比 3b 的 +7.4 s 小——平台层没有
rusty_h265 那种重型 codegen）；壳文件持平（16 s）。check 稳定 1.3~1.4 s。
重建清单干净：改平台层只波及 `hongguo-media-platform` + `hongguo-desktop`，
不误伤其他叶子 crate。

⚠️ 预热注意（3a 教训的变体）：`cargo test --workspace` 带 shell dev-deps 的
`test-support` feature，`cargo build` 是无 feature 的另一套指纹——测试后第一轮
build 要为新配置重做 codegen（实测 45 s），**复测必须等预热后的第二轮**再读数。

## 6. 附带发现：本机 `make lint` 在 `cargo clean` 后会挂在 `objc2-exception-helper`

两个 macOS 交叉 clippy 用 `CC_...=true` / `AR_...=true` 占位，让
`objc2-exception-helper` 的 C 编译步骤假成功。但该构建脚本会去 PATH 里找
`true` 这个可执行文件——**Windows 上不存在**，只有构建脚本产物被缓存时才不会暴露。
`cargo clean` 之后第一次跑交叉 clippy 就会：

```
error: failed to run custom build command for `objc2-exception-helper v0.1.1`
       failed to find tool "true": program not found
```

绕过：把 Git for Windows 的 `usr\bin`（里面有真的 `true.exe`）**追加**到 PATH 末尾
（追加而非前插，避免它的 `link.exe` 抢到 MSVC 的位）：

```powershell
$env:PATH = $env:PATH + ';C:\Program Files\Git\usr\bin'
```

长期修法（未做，留待评估）：给 `make lint` 的交叉两条命令加一层「确保 `true` 可解析」
的前置检查，或改用 `--config env.CC_...` 指向占位脚本。


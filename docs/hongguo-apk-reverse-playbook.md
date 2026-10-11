# 红果 APK 逆向操作手册（经验总结）

> 来源：sess 会话「逆向分析 APK 并自动补齐环境」（2026-10-10/11），目标是从官方 APK
> `com.phoenix.read` 7.3.9.32（version_code 73932）静态还原抖音扫码登录协议，
> 并顺带产出全量端点清单。本文沉淀**可复现的操作流程与全部踩坑**，协议结论本身
> 见 [`hongguo-login-channels.md`](hongguo-login-channels.md) §3，端点清单见
> [`hongguo-apk-endpoints.md`](hongguo-apk-endpoints.md)。
>
> ⚠️ 本文档记录的是**静态逆向**（不运行 APK、不抓包）。抓包对齐仍按老规矩
> 「请求 + 响应 + UI 效果」三点一线，见 [`hongguo-api-endpoints.md`](hongguo-api-endpoints.md)。

## 0. 一句话流水线

```
APK → 去掉 DebugProbesKt.bin 的副本 → 解出全部 dex
    → 全量字符串表 strings.tsv（grep 定位 URL 在哪个 dex）
    → find-str-refs.py（const-string 反查引用类与方法）
    → jadx 只反编译命中的那一个 dex（秒级~分钟级）
    → 反编译源码里 grep 完整 URL / 参数名 → 拿到 类:行号 → 读混淆类还原协议
```

核心思路：**14.4 万类的 APK 不要整体反编译**。先用字符串表和字节码反查把范围
缩到单个 dex，再对单 dex 跑 jadx——又快又省内存。

## 1. 环境清单（本机已装好，直接用）

| 工具 | 位置 | 说明 |
| --- | --- | --- |
| APK 副本 | `C:\Users\liu13\Downloads\hongguo.apk` | 官方包，SHA-256 以 Evidence JSON 为准，别信终端回显 |
| 去重副本 | `…\analysis\hongguo-apk\hongguo-nodup.apk` | 去掉 `DebugProbesKt.bin`（见坑 3.1） |
| JDK 21 Temurin | `C:/Program Files/Eclipse Adoptium/jdk-21.0.12.101-hotspot` | 已 `setx JAVA_HOME`；命令行里仍建议显式 `export` |
| jadx CLI 1.5.3 | `$HOME/tools/jadx-cli/bin/jadx.bat` | GitHub releases 直链 zip 解压即用 |
| jadx-headless-mcp 0.7.1 | `C:/Users/liu13/tools/rea/jadx-headless-mcp-0.7.1-all.jar` | 已 `setx REA_JADX_MCP_JAR`；**实测不好用，见 §2** |
| Python 3 | 系统 PATH | 只用于自写辅助脚本（dex 解析、字符串导出） |

jadx 安装（离线可复现）：

```bash
curl -sL -o jadx.zip https://github.com/skylot/jadx/releases/download/v1.5.3/jadx-1.5.3.zip
# 解压到 C:\Users\liu13\tools\jadx-cli\
```

## 2. 工具选型：jadx-headless-mcp 弃用，直接用 jadx CLI

两个独立原因，都实测踩过：

1. **环境快照问题**：会话内已启动的 MCP server 看不到 `setx` 写入的新环境变量
   （server 是会话启动时的环境快照）。绕法是每次走 CLI 显式传变量：
   `JAVA_HOME=… REA_JADX_MCP_JAR=… npx -y rea-agents@6.3.0 search-android-classes <apk> <query>`，
   但每次调用要重启 JADX 引擎（约 1 分钟）。
2. **单类反编译极慢**：该引擎的 `get_class_source` 对这个 14.4 万类的包做全量
   类型解析，单类 15 分钟起步，8GB 堆直接 OOM（要 `-Xmx24g` 才勉强）。方法级
   查询一样慢。

结论：**别等它**。改用标准 jadx CLI 批量反编译 + 本地 grep，全程秒级~分钟级。
MCP 引擎仅剩的价值是 `__dump_classes` 全类清单（秒回），而这个用 `dex-classes-*.txt`
静态产物完全替代。

## 3. 踩坑清单（现象 → 解法）

### jadx / Java 层

| # | 现象 | 解法 |
| --- | --- | --- |
| 3.1 | REA 加载 APK 报 fail-closed `unreadable_output / inventory duplicated` | APK 根目录的 `DebugProbesKt.bin`（Kotlin coroutines 探针）被 jadx 记成重复类。zip 重打包去掉该文件即可（产物 `hongguo-nodup.apk`） |
| 3.2 | jadx 退出码 1 | **通常非致命**（"finished with errors, count: 10" 级别）。不要判失败，直接检查 `-d` 目录下 `sources/` 是否产出再用 |
| 3.3 | 后台两个 JVM 并行反编译不同 dex，进程静默挂掉 | 串行执行。用脚本 `run-jadxNN.sh` 逐个跑，日志写 `jadxNN.log`，结束时往 `jadx-status.txt` 追加 `NN exit=?` 便于轮询 |
| 3.4 | jadx INFO 日志混进管道输出 | `grep -av INFO` 过滤 |

### shell / 环境层

| # | 现象 | 解法 |
| --- | --- | --- |
| 3.5 | `grep` 实为 **ugrep**：`\$POST` 之类转义报 `invalid escape`；变量为空时报 `ugrep: warning: : No such file or directory` | 复杂模式改写（如 `grep -aE "\\$POST"`）；先确认变量非空再拼命令 |
| 3.6 | **长/多行内联 Bash 命令被传输层吞字**（表现为莫名 `No such file or directory`、字符丢失） | 用 Write 工具写成 `.sh` 脚本再执行；或 `python - <<'PYEOF'` heredoc。**不要硬调内联长命令** |
| 3.7 | 每次 Bash 调用 cwd 都重置回项目根目录 | 每条命令显式 `cd` 或用绝对路径 |
| 3.8 | `grep -rln` 输出的路径含 `\`，直接拼接路径会失败 | 对输出做替换，或改用 `rg` / Python `Path.rglob` |
| 3.9 | `make` 不在 PATH | 直接用底层命令：`pnpm typecheck`、`pnpm test` 等 |

### 检索层

| # | 现象 | 解法 |
| --- | --- | --- |
| 3.10 | `strings.tsv` 是 **CRLF 行尾**，正则按行尾匹配时 `$` 吃不到 | 脚本处理一律 `rstrip("\r")`；grep 模式行尾锚点前留 `\r?` 余量 |
| 3.11 | dex 二进制 grep 无输出 | 必须带 `-a`（binary 当文本搜） |
| 3.12 | `find-str-refs.py` 传完整 URL 长 needle：0 命中或 needle 本身被吞 | **先用短 needle**（如 `"qrcode"`、`"scan_qr"`）圈出候选类，再到反编译源码里 grep 完整 URL 定位行号 |
| 3.13 | 关键哈希值（SHA-256 等）终端显示可能被截断/转译 | 以 Evidence JSON 文件为准 |

## 4. 标准工作流：端点 URL → 源码行号

以实战案例 `/passport/open/scan_qrcode/` →
`com.dragon.read.component.biz.impl.mine.scan.h.java:119`（POST，body:
`token/scene/qr_source_aid`）为例：

```bash
cd C:/Users/liu13/tools/rea/analysis/hongguo-apk

# ① URL 片段 → 定位 dex（strings.tsv 每行 = classesNN.dex \t 字符串）
grep -aE "\t/passport/open/scan_qrcode/$" strings.tsv
#   → classes16.dex

# ② 字符串 → 引用类与方法（短 needle！见坑 3.12）
python find-str-refs.py dex/classes16.dex "scan_qrcode"
#   → 输出：类 :: 方法 [virtual|direct] -> "字符串" (const-string)

# ③ 只反编译命中的 dex（单 dex 秒级~分钟级）
JAVA_HOME="C:/Program Files/Eclipse Adoptium/jdk-21.0.12.101-hotspot" \
JAVA_OPTS="-Xmx14g" \
$HOME/tools/jadx-cli/bin/jadx.bat --no-res --no-debug-info -j 10 \
  -d jadx-src16 dex/classes16.dex

# ④ 源码里 grep 完整 URL 拿行号，再看上下文提参数组装
grep -rn "scan_qrcode" jadx-src16/sources --include="*.java"
grep -n '\.c("\|\.a = ' jadx-src16/sources/com/dragon/read/.../h.java

# ⑤ 反查调用者（谁调这个类/方法）
grep -rln "TokenHelper" jadx-src20/sources --include="*.java"
```

变体技巧：

- **全量枚举某业务域端点**（不用 strings.tsv，直接扫反编译产物）：
  ```bash
  grep -rhoE '"/passport/[a-z_/]*(auth|login)[a-z_/]*"' jadx-src20/sources \
    --include="*.java" | sort -u
  ```
- **请求头枚举**：`grep -ahoE "\tx-[a-z0-9-]{2,40}$" strings.tsv`
- **模板端点易漏**：Retrofit 路径形如 `$POST` / `{xxx}` 前后缀，裸 grep 路径会漏，
  记得搜 `$POST`（坑 3.5 的转义写法）。
- **native 层排查**：签名五件套（x-gorgon/x-argus/x-ladon/x-khronos）在全部
  Java/dex 字符串中**不存在**，仅在 `libsscronet.so` 二进制命中 → native 层计算，
  Java 侧无可抄逻辑。搜 so 用 `grep -a` 直接打二进制（产物在 `so/`）。

## 5. 分析产物与自写脚本（`C:/Users/liu13/tools/rea/analysis/hongguo-apk/`）

| 产物 | 内容 | 用途 |
| --- | --- | --- |
| `dex/` | APK 内全部 classesN.dex 解出 | jadx / find-str-refs.py 的输入 |
| `strings.tsv` | 全 dex 字符串表，约 136 万条，格式 `classesNN.dex\t<string>`，CRLF | 第一步定位：任何字符串 → 它在哪个 dex |
| `dex-classes-*.txt` | 每 dex 的全类名清单（合计约 14.4 万类） | 类名检索，替代 MCP 的类清单查询 |
| `class-inventory.txt` | 全量类名合集 | 同上 |
| `official-endpoints.txt` | 静态提取的官方端点 216 条 | 端点全量清单的原始中间产物 |
| `jadx-src10/16/20/sources/` | 反编译产物（核心业务在 10/16/20 三个 dex） | 读混淆源码 |
| `models-dump.json` / `models-fields.json` | 数据模型 dump | 按关键词筛字段（本例筛 `qr`/`scan` 无命中，说明该域不在 Java 模型层） |
| `so/` | native 库（.so）副本 | 二进制字符串检索 |
| `hongguo-nodup.apk` | 去 `DebugProbesKt.bin` 副本 | 喂给 REA / MCP 用 |

自写脚本（Python 只写辅助工具，符合仓库红线；改项目代码仍只准 Edit/Write）：

| 脚本 | 作用 |
| --- | --- |
| `find-str-refs.py` | **核心工具**。纯 Python 解析 dex：header（0x38 起 12 个 uint32 offsets）→ string_ids/type_ids/proto_ids/field_ids/method_ids/class_defs → 扫字节码 `const-string` / `const-string-jumbo` 反查引用类与方法。用法 `python find-str-refs.py <dex路径> "<needle>" [needle2 …]` |
| `dexstrings.py` | 导出 dex 字符串表 → `strings.tsv` |
| `gen-endpoints-doc.py` | 从字符串表生成端点清单（`docs/hongguo-apk-endpoints.md` 的生成器） |
| `dump-models.py` / `gen-internals.py` | 模型 dump / 内部结构报告 |
| `run-jadx10.sh` / `run-jadx20.sh` | 单 dex 反编译包装：固定 JAVA_HOME/JAVA_OPTS、`--no-res --no-debug-info -j 10`、日志与状态落盘（坑 3.3/3.4 的解法载体） |
| `jadx-client.mjs` | MCP 引擎的全类清单导出（`__dump_classes`），已被 `dex-classes-*.txt` 替代 |

## 6. 死路与误区（别重走）

1. **`/passport/open/scan_qrcode/` + `/passport/mobile/confirm_qrcode/` 不是桌面扫码登录**——
   这是手机确认侧（创作者中心「扫码确认」），方向反了。桌面取码走
   `open.douyin.com/oauth/get_qrcode/`（抖音开放平台 SDK，`com.bytedance.sdk.open.tt`）。
2. **抖音 app-to-app 授权**（拉起抖音 App 确认）桌面端不可行，放弃，走 QR 码轮询。
3. **`7025948416286921516` 等长数字不是 client_key**——是 PrivilegeManager 权益 ID。
   真正的红果 `client_key=awhrxjuqewhhyckk` 硬编码在 `ub4/j0.java:158`
   （DouYinOpenConfig）；换票 `platform_app_id=7828` 硬编码在 `impl/t.java:233`。
4. **签名五件套别想在 Java 层找到**——只在 `libsscronet.so`（native）里，见 §4。
   项目已有的 `signer/ticket.rs` 方案不受影响。
5. **路径在场 ≠ 接口可用**：静态字符串清单里大量是 H5 路由、废弃接口，
   对接口径仍以抓包三点一线为准（`docs/hongguo-apk-endpoints.md` 顶部的 ⚠️ 同款结论）。

## 7. 本次协议结论速览（详情见链接文档）

- **取码**：`GET open.douyin.com/oauth/get_qrcode/`，关键参数 `aid=1128`、
  `client_key=awhrxjuqewhhyckk`、`customize_params={"source":"pc_auth","not_skip_confirm":"true"}`、
  `signature`（= APK 签名证书 MD5 hex，register.rs 现成 `sig_hash`，离线可算）、
  `app_identity`（= `md5("com.phoenix.read")`，离线可算）。响应含 base64 二维码 + 轮询 token。
- **轮询**：`GET /oauth/check_qrcode/`（+`token`+`timestamp`，约 1s 一次），
  状态机 `new → scanned → confirmed`（返 auth_code）｜`refused`(-2)｜`expired`(-50)。
  源码 `com.bytedance.sdk.open.tt.QRCodeAuthTask`。
- **换票**：`POST /passport/auth/bind_with_mobile/`，`platform_app_id=7828`、
  `platform`、`code` 等；入口实锤 `TokenHelper.java:448`。响应 wire 字段与短信登录
  同构（`data.*` 包装）→ 服务端复用 `extract_cookie_pairs` 落会话。

详见 [`hongguo-login-channels.md`](hongguo-login-channels.md) §3 与
[`hongguo-api-endpoints.md`](hongguo-api-endpoints.md)。

## 8. 换一个 APK 时的 checklist

1. 复制 APK → 检查根目录有无 `DebugProbesKt.bin`，有则做去重副本（3.1）。
2. 解出 dex 到 `dex/`；跑 `dexstrings.py` 生成 `strings.tsv`；导出每 dex 类清单。
3. 已知 URL/关键字 → `grep -a` strings.tsv 定位 dex → `find-str-refs.py` 短 needle
   圈类 → jadx 单 dex 反编译 → 源码 grep 定位行号。
4. 反编译统一参数：`--no-res --no-debug-info -j 10`，`JAVA_OPTS=-Xmx14g`；
   串行跑，日志 + 退出码落盘。
5. 全程记住三条铁律：**exit 1 不一定是失败**；**长命令写成脚本**；**长 needle
   先换短 needle**。

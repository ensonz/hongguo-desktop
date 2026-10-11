# 目录结构改进计划（2026-10-11）

> 分支：dev ｜ 状态：**待确认，未经确认不动代码**
> 依据：`AGENTS.md` + `.agents/skills/hongguo-dev`（含 `dev-habits`）的目录结构规范逐条核对现状
> 原则：纯移动/重命名与行为修改分开提交（硬红线 10、13）；不为目录整齐制造转发文件；
> 新代码单一归属，同一段逻辑只留一个明确归属；`signer/`、`media/platform`、`player/` 渲染结构是禁改区。
> 目标：把「已定型但只落实了一半的约定」补齐，消掉唯一的真实逻辑双实现。
>
> **进度（2026-10-11）**：第一批已落地 —— §3 P2（fixture 迁 `tests/fixtures/`，消 `signer→domain` 反向依赖）、
> §5 P4（`episode-range-picker`）、§7.1（生成器归 `scripts/`）、§7.2（元测试改目录自动发现）。其余（§1 P0 / §2 P1 / §4 P3 / §6 P5 / §8）未动。

## 0. 结论与健康基线

结构大方向是干净的，问题集中在**「约定只落实了一半」**和**一处真实的逻辑双实现**。
另有一项跨层规则硬违反列入 §8：`dev-habits` 第 54 条（禁裸 JSON/map 拼参数与载荷）——后端 25 处事件载荷、22 处第三方请求体、前端 65 处 IPC 入参对象字面量待静态类型化。
不存在死代码、不存在循环依赖、i18n 双语完全对齐——已裸跑核实的基线：

| 检查项 | 命令 | 结果 |
| --- | --- | --- |
| 死导出 / 死文件 / 未用依赖 | `pnpm knip` | 零输出 |
| i18n 键对齐 | zh-CN.json vs en-US.json | 505 : 505，零 only-zh / only-en |
| 分层纪律 | `lib.rs` 通读 | 331 行纯装配，模块全私有，命令注册按注释分组 |
| 元测试守护 | `commands/mod.rs` 扫源码测试 | command 层禁 tokio 约定在守 |

规模基线：前端 197 个 `.ts/.tsx` 共 18,104 行；后端 189 个 `.rs` 共 44,357 行。

---

## 1. P0 —— 评论/剧评组件双实现（唯一的真实逻辑重复）

skill 反面清单原文：「同一业务规则不许两份实现」。这是全仓唯一一处硬违反。

### 1.1 位置与现状

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `src/features/player/components/comment-panel.tsx` | 645 | 沉浸流评论面板（hgplayer 红皮肤） |
| `src/pages/series/components/review-list.tsx` | 579 | 详情页剧评列表（语义令牌皮肤） |

逐行比对结果——不是「相似的两层」，是同一套东西写了两遍：

| comment-panel.tsx | review-list.tsx | 一致性 |
| --- | --- | --- |
| `ReplyRow` L51-128 | `ReviewReplyRow` L64-136 | **props 签名完全相同**（`reply/myUserId/onDigg/onReplyTo/onDelete`）；DOM 结构、`relativeTime`、`EmojiText`、删除按钮、♥ 点赞逐字同构 |
| `ReplySection` L129-216 | `ReviewReplySection` L137-223 | 同为「展开才拉首页 + hasMore 续拉 + 发送后强制展开」 |

两处差异仅在于：

1. **皮肤**：player 侧用 `neutral-*` 硬编码（红线 11 的 hgplayer 例外），series 侧用语义令牌；
2. **i18n key 前缀**：`player.comments.*` vs `detail.*`。

i18n 层已留下指纹——以下 8 个键值在两侧**逐字重复**：

```
reply / replyToComment / replyPlaceholder / replyToPrefix
expandReplies / collapseReplies / moreReplies / repliesLoadFailed
```

另 `detail.like` 重复 `player.interact.like`。`src/service/queries/danmaku.ts`（401 行 / 14 个 hook）
同时管弹幕、评论、剧评三件事，是同一问题在查询层的同源表现。

反证：后端**已经收敛**——`src-tauri/src/domain/api/danmaku/mod.rs:576 fetch_replies(path, body, env)`
就是评论与剧评共用的取数路径。前端没跟上。

### 1.2 改动

1. 新建 `src/components/common/comment/`，落三个共享件：
   - `reply-row.tsx`（合并 `ReplyRow` / `ReviewReplyRow`）
   - `reply-section.tsx`（合并 `ReplySection` / `ReviewReplySection`）
   - `comment-row.tsx`（两层共用的主列表行，若比对后确认同构则合并）
2. 皮肤差异用 props 传入 class（player 侧沿用既有红皮例外，series 侧传语义令牌）；
   或在 player 侧包一层红皮壳，共享件只出结构——按 1.1 比对结果哪种更少改动走哪种。
3. i18n 的 8 个重复键收敛到 `common.reply.*`，两侧改引；`detail.like` 改引 `player.interact.like` 后删除。
4. `src/service/queries/danmaku.ts` 按域拆成 `danmaku.ts` / `comments.ts` / `reviews.ts`，
   `queries/index.ts` 出口不变。

### 1.3 性质与验证

性质：**逻辑搬家不重写**，不动交互语义（player 区 skill 原文约束）。
预期副作用：两个文件自然回落到 300 行线内（`dev-habits #21`）。

验证：`pnpm typecheck` + `pnpm lint` + `pnpm test` + `pnpm knip`；
CDP probe 走 `/player` 评论面板与 `/detail` 剧评 tab 两条链路（回复展开/发送/点赞/删除）。

### 1.4 不作

不借机重构 `player-page.tsx`(683) / `player-controls.tsx`(589) / `home-page.tsx`(441) /
`series-detail-page.tsx`(422) / `browse-page.tsx`(382) / `settings-page.tsx`(363) /
`rich-emoji-input.tsx`(363)。`player/` 是敏感区，只做上面这一项搬家。

---

## 2. P1 —— `domain/api` 的 `mod+model[+parse]` 约定只落实了 2/14

### 2.1 位置与现状

`backend.md` 写明每域目录化 `mod+model[+parse]`，实际只有 2 个域有 `parse.rs`：

| 域 | mod.rs 行数 | fn 数 | parse.rs |
| --- | --- | --- | --- |
| danmaku | 984 | 16 | ❌ |
| rank | 983 | 3 | ❌ |
| register | 975 | 8 | ❌ |
| login | 888 | 15 | ❌ |
| interact | 860 | 19 | ❌ |
| recommend | 726 | 9 | ❌ |
| search | 477 | 6 | ❌ |
| calendar | 472 | 5 | ❌ |
| stream_pick | 421 | 8 | ❌ |
| history | 306 | 6 | ❌ |
| detail | 460 | 3 | ✅ |
| discover | 537 | 2 | ✅ |

`danmaku/mod.rs` 单文件混了 4 类职责：5 个 payload 构造器（`danmaku_payload`/`comments_payload`/
`series_comments_payload`/`comment_replies_payload`/`review_replies_payload`）、6 个 fetcher、
3 个 parser（`check_comment_code`/`parse_comment_page`/`parse_reply_page`）、1 个发送器。

未核实项：`rank/mod.rs` 983 行只有 3 个 fn，疑似内联数据表/常量——拆它之前必须先读，不在本批范围。

### 2.2 改动

以 `danmaku` 为样板，按 `detail`/`discover` 已有的命名对齐：

```
domain/api/danmaku/
├── mod.rs       # 只留对外 pub async fn + 模块声明
├── model.rs     # 已有，不动
├── payload.rs   # 5 个 *_payload 搬入
└── parse.rs     # 3 个 parse/check 函数搬入
```

其余域（`interact`/`login`/`register`/`recommend`/`search`/`calendar`/`stream_pick`/`history`）
按同一形状逐个推进，**一个域一个 commit**。

### 2.3 验证

`cargo test --lib`（该域测试随文件走 `#[path]` 挂回，模式见 `backend.md` 测试规范）+
`make lint`。纯搬迁，无行为变更。

---

## 3. P2 —— 测试数据住在生产源码树，且构成反向依赖 ✅ 已落地

### 3.1 位置与现状

```
src-tauri/src/domain/api/testdata/
├── py_enc_accepted.hex
├── py_gz.hex
├── register_req_real.bin
└── tt_v5_golden.hex
```

引用方只有两处 `#[cfg(test)]`：`domain/api/register/mod.rs:513,545`、
`signer/tt_hash.rs:1127`。

两个问题：① 纯测试专用数据却编译进 `src/` 生产树；② **`signer` 伸手 include
`../domain/api/testdata/`**，形成 `signer → domain` 的反向依赖（规定的单向是 commands→service→domain）。

仓库已有正确落点：`src-tauri/tests/fixtures/`（`heic-cover-sample.heic` 就在那）。

### 3.2 改动

整体移到 `src-tauri/tests/fixtures/`，两处 `include_bytes!`/`include_str!` 路径跟着改。

### 3.3 验证

`cargo test --lib register` + `cargo test --lib signer::tt_hash`。

---

## 4. P3 —— `components/` 两级散装，约定第 5 条只落实了 emoji

### 4.1 位置与现状

`frontend.md` 目录归属第 5 条：「shadcn 件 → `ui/`；布局壳 → `layout/`；跨页展示件 → `common/`」。
实际 `common/` 里只有 `emoji/`，6 个跨页展示件散在 `components/` 根：

| 文件 | 行数 | 使用处 |
| --- | --- | --- |
| `src/components/refresh-shade.tsx` | 34 | 6 处 |
| `src/components/series-cover.tsx` | 44 | 5 处 |
| `src/components/batch-manage.tsx` | 118 | 4 处（collection/history/liked/reservation 四页） |
| `src/components/series-row-card.tsx` | 102 | 2 处（rank-row、calendar-row） |
| `src/components/list-search.tsx` | 32 | 4 处 |
| `src/components/skeletons.tsx` | 32 | 4 处 |

结果是 `components/` 同时存在「文件」与「目录」两个层级，归属判定只能靠记忆。

### 4.2 改动

6 个文件移入 `src/components/common/`，`components/` 收敛成 `ui` / `common` / `layout`
三个子目录、零散 file。**与 P0 的 `common/comment/` 合并为一次提交**——两次 import
改写一次做完，比分开多做一轮全量引用改动划算。

### 4.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm knip` + CDP 全路由 probe。

---

## 5. P4 —— 两个同名不同物的 `EpisodePicker` ✅ 已落地

### 5.1 位置与现状

| 文件 | 行数 | 职责 | props |
| --- | --- | --- | --- |
| `src/features/player/components/episode-picker.tsx` | 333 | 播放器选集浮层（双 tab + 数字网格） | `seriesId/currentIndex/hint/onSelect/onClose` |
| `src/pages/series/components/episode-picker.tsx` | 148 | 下载批量选区（区间语法 + 反选） | `episodes/selected/onChange` |

同名、同在 components 约定下、职责与 props 完全不同。违反 `frontend.md`「命名准确」。

### 5.2 改动

`src/pages/series/components/episode-picker.tsx` → `episode-range-picker.tsx`
（组件导出名同步 `EpisodePicker` → `EpisodeRangePicker`）；player 侧保留 `EpisodePicker` 不动。

### 5.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm knip` + CDP probe `/tasks` 下载批量选择链路。

---

## 6. P5 —— 前后端域粒度错位，「同名同构」断了两处

### 6.1 位置与现状

`domain/api` 有 17 个域，前端 `commands/` 15 个、`queries/` 16 个，且错位：

- **四处合一**：`rank` + `new_drama` + `reservation` + 搜索 四个 API 域 → 前端只有
  `commands/rank.ts`；`rank_cmd.rs`(182) 里也确实混着七个命令：
  `rank_list` / `new_drama_list` / `search_series_cmd` / `search_suggest_cmd` /
  `reservation_list` / `reservation_reserve` / `new_drama_calendar`
- **二合一**：`discover` + `recommend` → `commands/discover.ts`
- 而前端 `pages/` 是 `rank/` `new-drama/` `reservation/` 三个独立页面目录

`dev-habits #23` 的核心是「看目录就能定位对应关系」，这里断了。

### 6.2 改动

按 `pages/` 的域粒度对齐：

- 前端：`commands/rank.ts` → `commands/{rank,new-drama,reservation,search}.ts`；
  `queries/rank.ts` 同步拆（`useRank` / `useNewDrama`+`useNewCalendar` / `useReservations`+`useReserveSeries`
  / `useSeriesSearchApp`+`useSearchSuggest`）；`commands/index.ts`、`queries/index.ts` 出口保持不变。
- 后端：`rank_cmd.rs` 按同样边界拆成 `rank_cmd.rs` / `new_drama_cmd.rs` / `reservation_cmd.rs`，
  `lib.rs` 的 `generate_handler!` 同步更新。**若 §7.2 的 `SOURCES` 自动发现已先落地**，
  拆出的新文件自动纳入守护；否则 `commands/mod.rs` 的清单要手动补三条。

优先级低于 P0~P3：现在还能忍，**下次加预约相关功能时顺带做**，不单独排期。

### 6.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm knip` + `cargo test` + `make lint`；
CDP probe `/rank` `/new` `/reservations` 三页。

---

## 7. P6/P7 —— 零散项（可随手带走，不单独排期）

### 7.1 `gen_tt_hash.py` 放错了根 ✅ 已落地

`src-tauri/gen_tt_hash.py` 是生成器，应在仓库根 `scripts/`（那里已有 `make-icons.py`、
`build-updater-manifest.py` 等 6 个同类脚本）。纯移动。

### 7.2 meta-test 的 `SOURCES` 清单是手工登记的坑 ✅ 已落地

`src-tauri/src/commands/mod.rs` 的 `commands_do_not_touch_tokio_context_directly` 用硬编码
`SOURCES` 数组列文件。**新增 command 文件不加进这个列表，就静默逃过守护**，而清单里
没有写这条说明。改成测试时用 `env!("CARGO_MANIFEST_DIR")` + `read_dir` 自动发现 `.rs`，
零依赖、自维护。

### 7.3 响应式栅格断点三处逐字重复

`grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 2xl:grid-cols-8` 出现在：

- `src/components/skeletons.tsx:11`
- `src/pages/series/components/series-card-grid.tsx:28`
- `src/pages/new-drama/components/new-drama-recommends.tsx:73`

按 `src/styles/index.css` 已有 `hg-loadbar` / `scrollbar-thin` 的先例定义成 `hg-cover-grid` 全局类。

### 7.4 目录名与路由名不一致且无说明

`/tasks` 的实现在 `pages/download/`；`pages/series/` 根本没有 series 路由（承载 `/browse` + `/detail`）。
这是**有意的按域分组**，但 `frontend.md` 目录归属一节没写，新人会找不到实现。补一句话说明即可。

### 7.5 `utils/playback-prefs.ts` 兼职管了隐身开关

`readIncognito`/`writeIncognito` 被 `src/features/player/components/incognito.ts` 从这导入。
隐身是窗口行为不是播放偏好。拆成 `playback-prefs.ts` + `incognito.ts` 两个小文件。

---

## 8. P1 —— dev-habits 第 54 条落地面：参数与载荷静态类型化

> 依据：`dev-habits` 第 54 条——「参数与载荷一律用静态类型承载，禁裸 JSON/map 拼装：Rust 定义 `struct`（`Serialize`/`Deserialize`），Go 定义结构体 + `json` tag，TS 用 zod schema + `z.infer` 推导类型；`serde_json::json!`、`map[string]any`、对象字面量只允许出现在测试 fixture 与日志序列化」。
> 上游同步：落地后 `references/ipc.md` 第三节「payload 用 `serde_json::json!({...})`」作废，改为「载荷定义静态类型 struct」——那是当初为省事留的口子，正是本节要堵的口。

### 8.1 位置与现状

三类违反面已逐类核实（数字为裸跑 grep 的站点数，非估算）：

**① Rust 事件载荷：25 处 emit 站点用 `json!` / `to_value` 拼装**

| 文件 | 站点 | 载荷形状 |
| --- | --- | --- |
| `commands/download/actions.rs` | 6 | `{id}` / `{}`，其中 1 处 `to_value(task)` |
| `commands/download/cleanup.rs` | 3 | `{}` ×2，其中 1 处 `to_value(task)` |
| `service/download_service/scheduler.rs` | 5 | `{id}`×2、`{id,error}`、进度四字段、`{}` |
| `commands/login_cmd.rs` | 5 | MFA 五态（`{state,message}` / `{state,name}`） |
| `commands/app_cmd.rs` | 3 | `{visible}`（走 `emit_to`） |
| `service/play_service/online.rs` | 2 | 进度五字段 |
| `commands/transcode_cmd.rs` | 1 | `{key,percent,phase}` |

三个加重情节：

- `actions.rs`/`cleanup.rs` 各有一处 `serde_json::to_value(task).unwrap_or_default()`——顺手吞掉序列化错误（`dev-habits #14`）；而 `merge_cmd.rs` 的 `&task`/`&snapshot`、`series_cmd.rs` 的 `&series_id`、`lib.rs` 的 `()` 就是仓内现成的正确范式，只差铺开。
- 前端 `useEvent<DownloadTask | null>(EVENTS.downloadTaskAdded, invalidate)` 之类是**假类型**：handler 是 invalidate，载荷既不被消费也不被校验，Rust 侧改键名前端零感知。
- `LoginMfaState`/`OnlineProgress`/`CompatProgress`/`IncognitoVisibility` 四类载荷 Rust 侧**没有任何类型**；前端 `OnlineProgress`/`CompatProgress` 还是手写 `export type` 而非 zod（`service/schema/play.ts:44,56`）——契约两侧手抄，无校验点。

**② Rust 第三方请求体：22 处生产态 `json!` 构造**

| 文件 | 站点 | 说明 |
| --- | --- | --- |
| `domain/api/danmaku/mod.rs` | 6 | §2 已列的 5 个 `*_payload` 构造器，返回 `Value`；已有抓包比对测试兜底 |
| `domain/api/interact/mod.rs` | 15 | 点赞/评论/回复发送载荷；其中 2 处 `payload["reply_to_reply_id"] = json!(rr)` 是条件字段下标插入 |
| `domain/api/login/mfa.rs` | 1 | MFA biz 载荷（80 行） |

豁免不动（第 54 条后半句明许）：各域测试段的 `json!` fixture（`detail/parse.rs`、`login/mod.rs`、`mfa.rs` 测试段、`danmaku`/`calendar` 测试段）、`register/mod.rs:454` 的 TT_DEBUG dump（调试输出）。

**③ 前端 IPC 入参：`call` 的 `args?: Record<string, unknown>` 是动态口子**

83 个 `call` 站点中 65 处传对象字面量（如 `commands/login.ts` 的 `{ mobile, code, mobileTicket: ticket ?? null }`）。对象字面量赋给索引签名不做键名检查——键名拼错 `tsc` 放过，运行时才被 Rust 拒。响应侧已合规（全走 zod），入参侧无 schema。

### 8.2 改动

1. **事件载荷 struct 化（25 处）**：下载系载荷（`{id}`、`{id,error}`、进度、空载荷）定义在 `service/download_service/events.rs`（与 `names` 同处，契约集中）；login/app/transcode/online 的载荷定义在各自 `*_cmd.rs`/`play_service` 顶部，能复用 `domain/model` 已有模型的优先复用。统一 `#[serde(rename_all = "camelCase")]`，键名与前端现状逐字对齐。
2. **空载荷用 marker struct，wire 零变化**：`queue-changed` 的 `json!({})` 改为 `emit(name, &QueueChanged)`（`struct QueueChanged;`，序列化仍是 `{}`）——载荷有了类型，wire 不动，前端无感。
3. **第三方请求体 struct 化，与 §2 P1 合流**：`*_payload` 构造器改返回 `Payload` struct——**字段声明顺序必须与 `json!` 字面量顺序一致**（client `to_vec` 出的 body 字节序不能变，`payload_shape_matches_capture` 等抓包比对测试就是守门人）；`reply_to_reply_id` 条件字段改 `Option<i64>` + `#[serde(skip_serializing_if = "Option::is_none")]`，替掉下标插入。danmaku 随 §2 样板第一批做，interact/mfa 随 P1 各域推进；`payload.rs` 的职责相应写成「构造类型化 Payload」，不是「搬 `Value` 构造器」。
4. **前端入参 zod 化（65 处）**——**2026-10-11 升级：本条已按 `COMMANDS` 命令注册表方案实施**。args zod 化照做，但落在注册表（命令名 → {args zod, response zod}）而非调用点 `satisfies`，命令名裸字符串与响应 schema 缺失两个洞一并堵上；以下原文留档：在 `service/schema/<域>.ts` 为带参命令补 args zod schema（与响应 schema 同文件），`z.infer` 出参数类型、调用点用 `satisfies`/标注约束对象字面量——把 `Record<string, unknown>` 的键名检查从运行时提到编译期。不做运行时 parse：入参都是本端构造的值，parse 属 `dev-habits #14` 的兜底；仅少数来源不纯的点（持久化读出的旧数据）局部 parse。
5. **文档同步**：`references/ipc.md` 第三节 payload 一句改为「载荷定义静态类型 struct」；`references/backend.md` 模型规约补一条「第三方请求体同受模型规约约束」。

### 8.3 性质与验证

性质：**行为中性**——事件载荷键名不变（空载荷走 marker struct，wire 仍是 `{}`）；请求体字节序不变（字段顺序对齐 + 抓包比对测试守门）；前端只多编译期约束。唯一行为差异：`to_value(task).unwrap_or_default()` 换成 `&task` 后，序列化失败不再退化为「发 null」而是整次 emit 静默失败（`let _ =`）——前者本就是吞错兜底，丢了更诚实。

验证：`cargo test --lib`（四个抓包比对测试全绿）+ `make lint` + `pnpm typecheck` + `pnpm lint` + `pnpm knip`；CDP probe 覆盖 `/player`（转码/在线进度事件）、`/tasks`（下载事件全态）、登录 MFA 五态、隐身切换（`emit_to` 的窗口目标）四条链路。

### 8.4 不作

- `signer/`、`media/platform`、`player/` 渲染结构——禁改区照旧。
- 测试 fixture 与 TT_DEBUG dump 内的 `json!`——第 54 条后半句明许。
- `bootstrap/device.rs` 的 `device_meta_set(Value)`——存储层接口，属第 24 条的领域，另案。
- `lib.rs` 的 `emit("close-requested", ())`、`merge_cmd` 的 `&task`/`&snapshot`、`series_cmd` 的 `&series_id`——已类型化，不动。

---

## 9. 推进顺序与提交切分

按「纯移动/重命名」与「行为可见改动」分开的原则分批：

| 批次 | 内容 | 性质 | commit 切分 |
| --- | --- | --- | --- |
| 第一批 | §3 P2 testdata 搬迁、§5 P4 重命名、§7.1 生成器归位 | 纯移动 | 三个独立 commit（一个动作一个） |
| 第二批 | §1 P0 共享评论件 + §4 P3 `common/` 归位 | 搬家 + 一处删除合并 | 先 `refactor:` 搬家，后 `refactor:` 收敛 i18n 与拆分 queries（若 i18n 键值有变更则单独一个） |
| 第三批 | §2 P1 danmaku 拆 `payload.rs`/`parse.rs` 做样板 + §8.2.1 事件载荷 struct 化（25 处）+ §8.2.3 danmaku 请求体随样板 struct 化 | 纯拆分 + 行为中性 | danmaku 拆分一个 commit；事件载荷一个 commit（25 处一次做完，不留半途状态）；请求体并入 danmaku commit |
| 第四批 | §6 P5 命令域对齐 + §8.2.4 前端入参 zod 化（65 处，按域同批补；已升级为 `COMMANDS` 注册表方案，见 §8.2.4 标注） | 纯搬迁 + 行为中性 | 前端、后端分开；args schema 随各域 schema 文件落 |
| 随 P1 各域 | §8.2.3 其余域（interact/login mfa）请求体 struct 化，跟着「一个域一个 commit」走 | 行为中性 | 随各域 commit |
| 随时 | §7 零散项 | 各自独立 | 随手带走 |

每批交付前裸跑确认退出码（不用管道吞失败）：`pnpm typecheck`、`pnpm lint`、`pnpm test`、
`pnpm knip`、`cargo test`、`make lint`。

**本计划未经确认不动代码。** 确认后从第一批开始执行。

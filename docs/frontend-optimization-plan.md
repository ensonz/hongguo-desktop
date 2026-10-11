# 前端优化计划（2026-10-11）

> 分支：dev ｜ 状态：**待确认，未经确认不动代码**
> 依据：`AGENTS.md` 红线 3/11 + `dev-habits` #52/#53/#54/#55 + shadcn/ui Critical Rules 全量扫描（三路体检：结构与模式 / shadcn 符合度 / 类型安全与工程质量），站点数均为逐处 grep 核实数，非估算
> 与 `structure-improvement-plan.md` 的关系：那份管「目录结构」，本文管「前端质量」。评论双实现（其 P0）、components 归位（其 P3）、EpisodePicker 重名（其 P4）、命令域对齐（其 P5）、danmaku.ts 拆分仍按原文档执行，本文不重复列出，只写接驳点（§4.7、§7.1）。
> 原则：同原文档——纯移动/重命名与行为修改分开提交；`player/` 渲染结构与 hgplayer 红皮的边界见 §8；新依赖 3 个已确认（§9）。

## 0. 体检总账与基线

结构底子是好的：业务代码零 `any` / `@ts-ignore`（唯一集中点在生成物 routeTree.gen.ts，已被排除），正规 Dialog 全带 Title，零手写 `dark:` 覆盖、零手写 animate-pulse 骨架、零 truncate 三连、零等值 w-h 并用；stores 全部 selector 形态无过度渲染；封面加载有 `series-cover` 唯一出口。问题集中在三类：

1. **「组件没装导致各写各的」**——`ui/` 只有 20 个 shadcn 件，缺 `dialog / avatar / empty / field / toggle-group / popover / spinner`，于是出现：手写弹窗 ×1（a11y 硬伤）、手写 portal 弹层 ×3、手写 active 按钮组 ×6 组、空状态 ×12（4 种 padding）、错误态 ×8~11、手写头像 ×2、badge span ×4、Loader2 手拼 ×10；另有**豁免区外硬编码色值 16 处**。
2. **「契约只落实了一半」**——响应侧全走 zod，入参侧 `call()` 的 `args?: Record<string, unknown>` 完全动态且命令名是裸字符串、无 schema 时 `raw as T`；事件载荷零运行时校验 + 3 个手写 type + 2 处假类型；`t()` 513 处裸字符串 key；ESLint 停在 recommended（非 type-aware）。
3. **「同一逻辑写了 N 遍」**——翻页去重 4 遍、digg 乐观更新 4 遍、组件层 setQueryData 10 处、mutation onError toast 三种文案形态并存、key 工厂被绕过 4 处 + 双轨 key。

基线（裸跑核实）：前端 197 个 `.ts/.tsx` 共 19,177 行；>300 行文件 15 个（4 个超 500，全在评论/剧评与播放器域）；`call` 站点 83 处；`t()` 调用 513 处；测试 5 个文件全部是 utils 纯函数，queries 层（业务逻辑大头）零覆盖。

---

## 1. F1 —— shadcn 基建：补装组件、button 升级、语义令牌、机械修正

### 1.1 现状

- `src/components/ui/` 现有 20 件，缺 7 件（清单见下）；`components.json`：new-york / zinc / lucide / radix。
- `ui/button.tsx` 用的是旧版兜底选择器 `[&_svg:not([class*='size-'])]:size-4`，不支持 `data-icon`；业务侧因此到处手动 `size-4`。
- `index.css` 已有 shadcn 标准族 + success/warning 令牌；但「热度/评分/计数」这类业务语义色没有令牌，16 处散写。

### 1.2 改动

1. **补装 7 件**：`pnpm dlx shadcn@latest add dialog avatar empty field toggle-group popover spinner`。按 shadcn skill 流程走：先 `info` 核对已装清单，`add` 后逐文件读产物核验（Sub-组件齐全、导入正确）。
2. **button.tsx 升级上游最新**：`add button --dry-run` → `--diff button.tsx` 逐文件比对本地改动后合并，使 `data-icon` 选择器生效。本地若有定制，按「无本地改动才覆盖，有则手工并」执行。
3. **业务语义令牌**：`index.css` 增三枚（值取现状主色 oklch 化），映射进 `@theme inline`：

   | 令牌 | 语义 | 现状色 |
   | --- | --- | --- |
   | `hot` | 热度 / 爆标 / 点赞激活 | red 系（`text-red-500` 等） |
   | `star` | 评分星 / 榜单前三 / 计数高亮 | amber 系 |
   | `warm` | 热度数值 / 计数 | orange 系 |

   落地时若发现两枚令牌语义重叠（如某处 amber 实为「计数」），按「一个语义一个令牌」合并，不并行保留。
4. **space-y ×2 → gap**：`layout/app-sidebar.tsx:65`（`flex-1 space-y-1 p-2` → `flex-1 flex flex-col gap-1 p-2`）、`series/series-detail-page.tsx:190`（`flex-1 space-y-4` → `flex-1 flex flex-col gap-4`）。
5. **模板字符串条件类 → cn()，5 处**：`rank/rank-row.tsx:67-69`（豁免区外）；player 区 4 处纯代码质量项——`player-controls.tsx:511`、`:553`、`:571`、`danmaku-layer.tsx:239`（产物字节级一致，零视觉变化，边界依据见 §8）。

### 1.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm knip`（新装组件若暂未被引用，knip 会报死文件——**装与用不分批时此问题不存在，若分批需在用上的批次前临时豁免或同批接上首个使用点**）。行为中性，无需 CDP。

---

## 2. F2 —— 手写浮层收编（a11y 硬伤优先）

### 2.1 现状

| 站点 | 现状 | 问题 |
| --- | --- | --- |
| `settings/login-dialog.tsx:128-186` | 手写 `fixed inset-0 z-50` 遮罩 + 手写面板 | 无 `role="dialog"`/`aria-modal`、无 Esc 关闭、无焦点圈禁；两个 Input **完全没有 Label** 只靠 placeholder；`:50` 硬编码事件名 `'login-mfa-state'` 绕过 `EVENTS.loginMfaState`（踩红线 3 精神），state 是宽 `string` |
| `common/emoji/emoji-picker.tsx:88-105` | createPortal + 手动 fixed 定位 + 手写 z-50 | 应为锚定触发器的 Popover |
| `rank/filter-panel-button.tsx:16-70` | 手写 dropdown + 手写 outside-click effect | 应为 Popover |
| `series/browse-page.tsx:298` | 手写 absolute 下拉面板 z-30 | 搜索建议面板，应为 Popover |

### 2.2 改动

1. **login-dialog 用 Dialog + Field 重写**：`Dialog` + `DialogContent/DialogTitle/DialogDescription` 修全 a11y（Esc、焦点、aria）；表单用 `FieldGroup` + `Field` + `FieldLabel` 补上缺失的 Label；事件名改 `EVENTS.loginMfaState`、state 收窄为字面量联合（F5 进 EventPayloads 后以 `z.infer` 为准）。保留现有交互（发送验证码 / 登录 / MFA 五态）。
2. **三处手写弹层 → Popover**：emoji-picker、filter-panel-button（删手写 outside-click effect）、browse-page 搜索建议。锚定、层叠、关闭行为全部交给组件；豁免区外的 4 处手写 z-index 随之消失。

### 2.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm knip`；CDP probe 四链路：登录（含 MFA 态流转）、评论框表情、榜单筛选、搜索建议；键盘走查（Tab/Esc）。一个浮层一个 commit。

---

## 3. F3 —— 展示件收编与色值语义化

### 3.1 现状

- **空状态 ×12 手写**，同一语义 4 种 padding（`py-8`/`py-10`/`py-16`/`p-4 py-16`）：tasks-page:166、history-page:175、rank-page:185、browse-page:374、series-detail-page:361、review-list:447、storage-page:86、liked-page:100、collection-page:119、home-page:405、comment-panel:485、new-calendar-view:97。
- **错误重试 UI 手写重复**：8 处 `isError` 分支（home-page:380、browse-page:363、series-detail-page:55/72/198 等）+ rank/liked/collection 列表头同款 `text-destructive` + retry 条，共约 11 处；player 区错误条（player-page:230 等）是 hgplayer 皮肤，**不在收编范围**。
- **手写头像 ×2**：comment-panel:497-504（size-8）、review-list:457-463（size-9）——两套实现已分叉。
- **badge span ×4**：豁免区外 2 处——rank-row:78（季数 chip）、new-drama-card:55（计数 pill）；豁免区内 2 处（immersive-info-overlay:89、error-bar:21）不动（§8）。
- **Loader2 手拼 ×10**：`mr-1 size-4 animate-spin` 模式（collection-page:102、history-page:131、home-page:386/398、liked-page:94、new-calendar-view:84、new-drama-recommends:80、rank-page:164、reservation-page:127）——`mr-1` 与 Button 自带 `gap-2` 重复，`size-4`/`animate-spin` 是 Spinner 的职责。
- **手写 active 按钮组 ×6**：top-bar-tabs:55-76（顶栏胶囊 tab）、collection-page:133-146（TYPE_TABS，裸 `<button>`）、rank-page:111-126（子榜竖排）、filter-panel:65-81（pill 单选）、new-drama-page:79-88（GENDERS，variant 三元）、new-calendar-view:58-73（日期条）。
- **豁免区外硬编码色值 16 处**（表见 3.2）。

### 3.2 改动

1. **空状态 → Empty**：`ui/empty` 收编 12 处，统一结构（icon + 标题 + 可选描述）；comment-panel:485 与 review-list:447 随原计划 P0 共享评论件一并落地。
2. **ErrorState 共享件**：新建 `src/components/common/error-state.tsx`（`Empty` 结构 + 重试回调 + 可选 description），收编豁免区外全部错误重试 UI；文案走 i18n（双语同步）。
3. **头像 → Avatar + AvatarFallback**：随原计划 P0 合并时在共享件落地一次（替掉两处手写，`AvatarFallback` 取昵称首字）。
4. **badge span → Badge**：豁免区外 2 处，色值走 F1 令牌。
5. **Loader2 ×10 → `<Spinner data-icon="inline-start" />`**：删 `mr-1 size-4 animate-spin`，间距交给 Button 的 gap。
6. **手写 active 组 → ToggleGroup**（`type="single"`）：6 组全收，**保持现视觉**（胶囊/pill 样式经 `className` 传给 `ToggleGroupItem`，不引新外观）；top-bar-tabs 的路由跳转语义保留（选中态随路由，不改成受控状态源）。
7. **色值 16 处 → F1 令牌**（同语义不同深浅一并归一，这正是收编的意义）：

   | 站点 | 现状 | 语义 | 令牌 |
   | --- | --- | --- | --- |
   | liked-page:237 | `fill-red-400 text-red-400` | 点赞激活 | hot |
   | calendar-row:69 | `text-red-500` | 热度 | hot |
   | series-detail-page:235 | `text-red-500`（Flame） | 热度 | hot |
   | series-detail-page:261 | `bg-red-500` | 爆标 | hot |
   | series-detail-page:291 | `text-red-500` | 预约/热度态 | hot |
   | review-list:113、:509 | `hover:text-red-500` | 点赞激活 | hot |
   | new-drama-card:88 | `text-amber-400`（Star） | 评分 | star |
   | series-detail-page:224 | `text-amber-400` | 评分 | star |
   | related-card:42 | `text-amber-300` | 评分 | star |
   | review-list:419 | `text-amber-500` | 评分 | star |
   | rank-row:67-68 | `text-amber-500` | 榜单前三 | star |
   | new-drama-card:55 | `text-amber-300` | 计数 pill | star（落地时核对语义） |
   | reservation-page:311 | `text-amber-400` | 状态提示 | star（落地时核对语义） |
   | series-card-grid:79 | `text-orange-500` | 计数 | warm |
   | suggest-row:54 | `text-orange-500` | 热度值 | warm |
   | rank-row:91 | `text-orange-400` | 热度值 | warm |

### 3.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm test` + `pnpm knip`；CDP 全路由扫空态/错误态/加载态/榜单与新剧页色值；用户可见文案改动同步 `zh-CN.json` + `en-US.json`。按组件类一个 commit（Empty 一个、ErrorState 一个、ToggleGroup 一个、色值一个……）。

---

## 4. F4 —— 查询层治理（行为中性）

### 4.1 现状

- **key 工厂被绕过 4 处**：`queries/search.ts:57`（`['search-suggest', kw]` 内联）、`new-drama/calendar-row.tsx:28`（手写 `['reservations']` invalidate）、`series/resolve-button.tsx:21-22`（手写 `['series-episodes']`/`['series-meta']`）、`queries/rank.ts:152`（`findAll({ queryKey: ['rank'] })` 手写根前缀）。
- **双轨 key**：`queries/rank.ts:17` 的 `RESERVATIONS_KEY_ROOT` 与 `common.ts` 的 `keys.reservations` 并存。
- **翻页去重写了 4 遍**：`common.ts:65 dedupBy`（已有，comment-panel/review-list 在用）之外，`rank.ts:43-54`、`search.ts:9-21`、`feed.ts:11` 各自内联同构实现。
- **同 key 不同 queryFn**：`queries/series.ts:65-79` 的 `useSeriesMeta` 与 `useSeriesEpisodes` 共用 `keys.seriesEpisodes` 但 queryFn 不同（注释称故意进同一缓存）——实际是「先挂载者定形态」的脆弱写法。
- **热重载 workaround 无门控**：`series.ts:31-38` `Promise.race` + 30s 超时兜底，生产也在跑。
- **假 hook**：`queries/cover.ts useWebCover` 名为 hook 实为纯同步函数。

### 4.2 改动

1. 4 处绕过全部改回 key 工厂；删 `RESERVATIONS_KEY_ROOT`，统一 `keys.reservations`。
2. 三份内联去重改用 `dedupBy`（若形态差异大，先把 `dedupBy` 泛型化收编三种形态，而不是留三份）。
3. `useSeriesMeta`/`useSeriesEpisodes` 统一为带 resolve 回落的那份 queryFn，meta 侧从同一缓存 select——一次请求两种视图，不再赌挂载顺序。
4. `Promise.race` 包进 `if (import.meta.env.DEV)`，注释写明「热重载丢应答的 dev 兜底」；生产路径让错误正常暴露。
5. `useWebCover` 移出 `queries/` 归 `utils/` 改纯函数名，调用点跟着改。
6. **接驳原文档 P0**：digg 乐观更新 ×4（`danmaku.ts:162/205/303/349`）→ 通用乐观工厂；组件层 setQueryData ×10（comment-panel 5 + review-list 5）→ 收进查询层。这两项归原文档第二批执行，本文只登记。

### 4.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm test` + `pnpm knip`；`useInfiniteStream` 三处调用链 CDP 翻页 probe（榜单翻页 / 搜索翻页 / feed 流）。行为中性，纯等价改写。

---

## 5. F5 —— 契约类型化（升级并取代原文档 §8.2.4）

### 5.1 现状

`call<T>(command: string, args?: Record<string, unknown>, schema?)`（`service/tauri/invoke.ts:11-21`）三个洞：

1. `command` 裸字符串——命令名拼错编译期不可见；
2. `args` 索引签名——对象字面量键名拼错 `tsc` 放过，运行时才被 Rust 拒（83 处调用点全在此口子上）；
3. `schema` 可选——不传时 `return raw as T` 裸信任断言，响应 schema 附着率不全（`call<void>`/`call<number>` 类站点普遍裸奔）。

事件侧：`useEvent` 纯泛型直转零校验（`events.ts:11-13`）；载荷类型三个来源混杂（`z.infer` / 手写 `export type`：`DownloadProgress`、`OnlineProgress`、`CompatProgress` / 假类型：`queries/download.ts:84-88`、`queries/series.ts:15`）。

### 5.2 改动

1. **`COMMANDS` 命令注册表**（取代原文档 §8.2.4 的「调用点 satisfies」路线）：`src/service/tauri/registry.ts`，命令名 → `{ args: ZodType, response: ZodType }`，schema 本体仍住在 `service/schema/<域>.ts`（保持域组织），注册表聚合。与 `EVENTS` 同性质的「两端镜像」常量，头注释钉住同步责任（Rust 命令改名/增删参数，同一 commit 改这里）：

   ```ts
   export const COMMANDS = {
     download_pause: { args: z.object({ id: z.string() }), response: z.void() },
     // …
   } as const;
   ```

   `call` 改按键索引的强类型入口：`call<C extends keyof typeof COMMANDS>(command: C, args: z.infer<…['args']>)`，返回类型取 `response` 推导。响应 schema 在注册表**必填**——`z.void()` 也算 schema，禁 `z.any()`；83 站点随迁移补齐缺失的响应 schema。`commands/<域>.ts` 保留为域门面，函数签名由注册表推导。
2. **事件载荷类型化**：`EventPayloads` 映射（事件名 → zod schema，载荷 struct 化的 Rust 侧按原文档 §8 批次走），`useEvent` 内 `safeParse`，失败 `console.error` + 丢弃——事件坏掉是静默的，校验失败必须 dev 可见；手写三个 type 改 `z.infer`；两处假类型站点清理。
3. **`t()` 路径类型化**：`locales/index.ts` 递归 keyof：

   ```ts
   type Paths<T> = { [K in keyof T & string]: T[K] extends object ? K | `${K}.${Paths<T[K]>}` : K }[keyof T & string];
   export type MessageKey = Paths<typeof zhCN>;
   ```

   513 处调用点获得编译期检查；en-US 侧 `satisfies Record<MessageKey, string>` 抓漏键；运行时回落 key 时 dev 下 `console.warn`（用户看到 `feed.loadFailed` 原文属于事故，现在它静默）。
4. **ESLint 升 type-aware**：`recommendedTypeChecked` + `projectService`，启用 `no-floating-promises` / `no-misused-promises` / `no-unsafe-*`（type-aware 附带）；新报问题随本批修净——这正是要它抓的；生成物已在 ignores。

### 5.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm test` + `pnpm knip` + 全路由 CDP probe（IPC 全面过手）。行为中性：命令名、参数键名、响应形状全不变，只加编译期约束与校验点。四个改动各一个 commit。

---

## 6. F6 —— 错误面与路由工程化

### 6.1 现状

- 无 `QueryCache`/`MutationCache` 全局 onError；UI 层 `.mutate(` 约 41 处，带 onError 约 20 处，文案三种形态并存（`toast.error(String(e))` / `toast.error(e.message)` / `toast.error(t(...), { description })`）；**tasks-page:119-135 批量操作全部只有 onSuccess——失败静默**。
- 无 ErrorBoundary、无 `errorComponent`/`notFoundComponent`（渲染异常落到 TanStack Router 英文默认页，深链 404 无中文兜底）。
- 14 条路由全部 eager import，无 `pendingComponent`；分包只做了 vendor 维度。
- `home-page.tsx:334-335` 注释声称「React Compiler 能自动 memo」——babel-plugin-react-compiler **并未安装**，失实。

### 6.2 改动

1. **全局错误面**：`QueryClient` 配 `QueryCache.onError` + `MutationCache.onError` → 统一 i18n toast（`call()` 归一的文案已是这个形态，全局兜底让事件/客户端异常同享）；mutation `meta: { silentOnError: true }` 作为跳过开关（乐观更新自带回滚+局部提示的场景用）。删 ~20 处手写 mutation onError toast，补 tasks-page 批量操作的失败可见性。
2. **渲染兜底**：`react-error-boundary` 包顶层；`createRouter` 配 `defaultErrorComponent` + `notFoundComponent`（中文，放 `src/components/common/`，文案进 i18n 双语）。
3. **路由 lazy**：14 条路由改 `lazyRouteComponent`（player/home/detail/browse 优先），`defaultPendingComponent` 复用 `skeletons.tsx` 对应形态。桌面端资源虽在本地，收益是冷启动解析开销。
4. **devtools**：`@tanstack/react-query-devtools`，`import.meta.env.DEV &&` 挂 QueryClientProvider 内。
5. 修正 home-page 失实注释（按事实写：Compiler 未启用）。

### 6.3 验证

`pnpm typecheck` + `pnpm lint` + `pnpm test` + `pnpm knip`；CDP probe：深链不存在的 id（notFound）、下载任务失败 toast、lazy 首开 pending 骨架。按项一个 commit。

---

## 7. F7 —— 性能与测试

### 7.1 改动

1. **评论/剧评虚拟化**：原计划 P0 合并成共享件后，接 `@tanstack/react-virtual`（`useVirtualizer` + `measureElement` 动态行高 + 保守 overscan）。评论/剧评是无限查询累积、长会话无界增长，全仓唯一真虚拟化需求；下载任务表量级可控不做。**依赖 P0**。
2. **queries 纯逻辑抽取 + 单测**：翻页 pageParam 推进、去重、digg 乐观 reducer 抽成纯函数模块，vitest node 环境直测（零新依赖）——queries 层是业务逻辑大头，现在零覆盖。
3. **schema fixture 测试**：取抓包真实响应样本存 `src/service/schema/__fixtures__/`，`schema.safeParse(fixture)` 断言——Rust 侧已有抓包比对测试守请求形状，前端这层守响应形状，serde 漂移立刻红。

### 7.2 验证

`pnpm test` 全绿为本批主验证；虚拟化 CDP probe 长评论滚动（万级楼层/滚动锚定/展开回复）。

---

## 8. player 区边界（成文说明，避免每次重新争论）

- **纳入**：`cn()` 等**零视觉变化**的代码质量项（F1 第 5 条的 player 4 处）。禁改区禁的是渲染结构与视觉，不是代码质量。
- **不动 + 理由**：
  1. player 内 14 处 z-index 是**成文 z 层级体系**（`player-controls.tsx:336-342` 有完整 z-scale 文档）。shadcn「禁手动 z-index」管的是 Dialog/Popover 这类 radix 浮层——它们自管堆叠；播放器的层是叠在 `<video>` 上的绝对定位兄弟层，规则不适用。把它「标准化」是把有意设计当违规治理。
  2. 红皮色值是红线 11 例外本身——hgplayer 1:1 对齐是需求，语义令牌化等于放弃对标。
  3. 沉浸流/错误条的 badge span（immersive-info-overlay:89、error-bar:21）色值属 hgplayer 规格，换 Badge 需整体覆盖 variant 样式才能保持外观，纯负收益。
- comment-panel 侧的手写头像/空状态不单独动：原文档 P0 合并共享件时自然收编（共享件出结构、player 侧红皮壳传色）。

---

## 9. 推进顺序、提交切分与依赖

| 批次 | 内容 | 性质 | commit 切分 |
| --- | --- | --- | --- |
| F1 | 装件、button 升级、令牌、space-y/cn | 中性 | 装件一个；button 升级一个；令牌 + 机械修一个 |
| F2 | login-dialog、三处 Popover | 行为可见 | 一个浮层一个 commit |
| F3 | Empty、ErrorState、Badge、Spinner、ToggleGroup、色值 | 行为可见 | 按组件类一个 commit |
| F4 | key/双轨/去重/series key/DEV 门控/useWebCover | 中性 | 按项 |
| F5 | COMMANDS、事件、t()、ESLint | 中性 | 四个独立 commit |
| F6 | 全局 onError、渲染兜底、lazy、devtools、注释修正 | 行为可见 | 按项 |
| F7 | 虚拟化、纯函数+单测、schema fixture | 中性 | 按项 |

依赖：F2/F3 依赖 F1 装的件；F3 的 Avatar/Empty（评论侧）、F4.6 接驳项、F7.1 依赖原文档 P0（其第二批）；其余互相独立。

每批交付前裸跑确认退出码（不用管道吞失败）：`pnpm typecheck`、`pnpm lint`、`pnpm test`、`pnpm knip`；本计划不动 Rust，无需 cargo。涉浮层/登录/路由的批次按各节 CDP probe 清单走。

**新依赖（已确认，3 个）**：`@tanstack/react-virtual`（F7）、`react-error-boundary`（F6）、`@tanstack/react-query-devtools`（F6，dev-only）——均选最新稳定版并在 commit 注明版本；随使用它们的批次安装，不提前引入。

**本计划未经确认不动代码。** 确认后从 F1 开始执行。

---

## 10. 不作

- >300 行的内聚页面（reservation 399 / history 325 / collection 317 / browse 406）不凑行数拆分——与原文档 §1.4 同一判断：单职责页面，拆分等出现第二职责。
- 表单库不引：仅手机号 + 验证码登录，`Field` 体系足够。
- `exactOptionalPropertyTypes` 收紧另案：成本高、收益边际，不混入本批。
- 组件级 jsdom / testing-library 设施不建：现有测试面（utils 纯函数 + 本计划新增的 queries 纯函数与 schema fixture）node 环境足够。
- `signer/`、`media/platform` 禁改区照旧；player 区按 §8 边界执行。

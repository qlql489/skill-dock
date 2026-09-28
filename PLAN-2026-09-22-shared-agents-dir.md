# 方案：`~/.agents/skills` 独立成「通用技能目录」节点 + MiMo 接入

日期：2026-09-22 · 状态：待评审

## 1. 一句话

把 `~/.agents/skills` 从"某个 Agent（当前是 Codex）的目录"升级成一个独立的**共享目录节点**；
已知兼容这个目录的 Agent 默认**关联**到该节点，安装位统一交给它；Agents 页加一个开关决定
这些关联 Agent 是**单列**还是**折叠进节点的关联列表**；顺带把小米 MiMo 加进内置目标。

## 2. 调研结论：谁真的读 `~/.agents/skills`

有官方文档或源码依据、**确认支持**的（括号内是本项目已有的 target id）：

| Agent | 依据 |
| --- | --- |
| Codex（`codex`） | `codex-rs/ext/skills/src/host_roots.rs`，常量 `AGENTS_DIR_NAME = ".agents"` |
| Cursor（`cursor`） | https://cursor.com/docs/context/skills |
| GitHub Copilot（`github-copilot`） | https://docs.github.com/en/copilot/concepts/agents/about-agent-skills |
| Gemini CLI（`gemini-cli`） | https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/skills.md（`~/.agents/skills` 为别名根） |
| OpenCode（`opencode`） | https://opencode.ai/docs/skills/ |
| Windsurf / Devin（`windsurf`） | https://docs.devin.ai/desktop/cascade/skills |
| Kimi Code（`kimi-code`） | https://github.com/MoonshotAI/kimi-cli/blob/main/docs/en/customization/skills.md |
| Qwen Code（`qwen`） | `QwenLM/qwen-code` `packages/core/src/config/storage.ts`：`SKILL_PROVIDER_CONFIG_DIRS = ['.qwen', '.agents']` |
| OpenClaw（`openclaw`） | https://docs.openclaw.ai/tools/skills |
| **MiMo（新增 `mimo`）** | https://github.com/XiaomiMiMo/MiMo-Code#builtin-skills（`.agents/skills/` 与 `~/.agents/skills/` 属兼容发现根） |

**明确不支持**：Claude Code —— 只读 `~/.claude/skills`，文档里从不提 `.agents`（https://code.claude.com/docs/en/skills）。

**没有公开依据**（不默认关联，但用户可在 UI 手动关联）：`zcode`、`deepseek-harness`、`pi`、`workbuddy`、
`minimax`/`minimax-builtin`、`trae`、`codebuddy`、`kiro`、`qoder`/`qoderwork`、`hanako`、`lingma`、`iflow`、
`kode`、`continue`、`hermes`、`antigravity`、`goose`、`roo-code`、`kilo-code`、`droid`、`crush`、`neovate`、
`openhands`、`grok`、`augment`、`junie`、`deepagents`。

**生态里有、我们目录里还没有的**：Zed（只读 `~/.agents/skills`，没有自己的目录）、Amp（`amp.svg` 图标其实
已经打进包了）。这两个"没有自己技能目录"的 Agent 需要先想清楚表示法（见 §11），本方案不做，列为后续增量。

## 3. 现状与本机数据（为什么要改）

- `targets.rs:223 dir_taken()` 强校验"一个目录只能属于一个目标"，所以**今天不可能让多个 Agent 共读
  `~/.agents/skills`**，只能有一个 Agent 占着它。
- 本机 `~/.skill-manager/state.json`：39 个目标，已启用 6 个（claude-code / codex / workbuddy / pi /
  deepseek-harness / zcode）；**codex 独占 `~/.agents/skills`，名下有 59 条安装台账**（全库最多）。
- `~/.agents/skills` 里已有约 170 个条目（真身目录 + 指向别处的软链）——这是事实上的主技能目录。
- 「中央仓库」（`ui.vault_path`）本机为空 → 走内置默认 `~/.skill-manager/skills`，与共享目录不冲突。

结论：这个改造不只是"换个显示方式"，它是当前架构下**唯一**能让多个 Agent 共用同一份技能的做法。

## 4. 数据模型

### 4.1 `AgentTarget` 新增两个字段（`state.rs`，两处都要 serde default）

| 字段 | 类型 | 语义 |
| --- | --- | --- |
| `is_shared_dir` | `bool`（默认 false） | true = 这个目标代表一个「多个 Agent 共读的技能目录」节点。全库只有一个：`agents-shared` |
| `linked_to` | `Option<String>`（默认 None） | 本目标关联到哪个共享目录节点。None = 独立 Agent，安装落自己的默认目录 |
| `link_touched` | `bool`（默认 false） | 用户是否手动改过关联（改过就不再被"默认关联"迁移覆盖），沿用现有 `user_touched` 的纪律 |

关联关系**只存在子项一侧**（节点不存列表），单一事实来源；节点被删/改名时子项自动回落为自己的目录。

### 4.2 `UiPrefs` 新增（`state.rs:205`）

```rust
#[serde(default = "default_true")]
pub show_linked_agents: bool,   // 默认 true = 保持现有观感，"显示已关联的 Agent"
```

`UiPrefs` 不进 `StateSnapshot`（`commands.rs:17-44`），所以照 `add_default_install_targets` 的样子配一对
独立命令：`groups.rs get_show_linked_agents / set_show_linked_agents`，前端进 `src/prefs.ts` 的缓存。

### 4.3 新增内置目标（`state.rs default_targets()`）

```rust
// 通用技能目录节点：id 固定，图标走 fallback 首字母或单独资产
{ id: "agents-shared", name: "通用技能目录", skills_dir: "~/.agents/skills/",
  description: "多个 Agent 共读的标准技能目录", is_shared_dir: true, enabled: true }

// MiMo
{ id: "mimo", name: "MiMo", skills_dir: "~/.config/mimocode/skills/",
  description: "Xiaomi MiMo Code / 桌面版", tag: Some("xiaomi"), linked_to: Some("agents-shared") }
```

检测规则（`detect.rs rule_for`）：
- `"mimo" => Rule { cli: Some("mimo"), fallback_dir: Some(".config/mimocode") }`
- `"agents-shared" => Rule { cli: None, fallback_dir: Some(".agents") }`（目录在=已装）

`AgentTarget` 会被 `get_state` 原样带到前端，所以 `src/types.ts` 的三个字段必须同步（踩过的
"字段不同步 → 页面空白" 坑）。

## 5. 核心决策：关联 = 安装位交给共享节点

**推荐语义**：一个 Agent 关联到节点后，它的**安装位就是节点**——新安装只往 `~/.agents/skills`
写一份软链，`~/.cursor/skills` 之类自己的目录降级为"只观察"（内容照扫，本机 skill 管理里照看得到，
但不再接收安装）。节点的开关等于"这批 Agent 的安装总开关"，停用时关联的 Agent 一起不接收安装。

理由：如果只是"把行藏起来"，藏起来之后你就没法给 Cursor 装东西了（它自己的目录仍是安装位，而它的
行已经不可见）——功能反而变少。反过来，装一份、十个 Agent 都能看到，正是用户想要的东西。

**备选（如果只想小步走）**：只做显示折叠，安装语义完全不变。代价是：共享目录要单独装一遍，
同一个技能会在 `~/.agents/skills` 和 `~/.cursor/skills` 各有一份软链，且隐藏后无法单独操作被隐藏的 Agent。
两条路的数据模型完全一样，后面想升级只差 `symlinks.rs` 的重定向那一步。

实现口径（后端单点收口 + 前端统一助手）：

- 后端 `symlinks::install()` / `uninstall()` 里做解析：目标若 `linked_to` 指向一个存在且启用的节点，
  就改用节点的目录并把台账 `target_id` 记成节点 id；节点缺失/停用时：缺失→回落到自己的目录，
  停用→报"通用技能目录未启用"。因为所有批量入口（组合应用、添加来源自动安装 phase 5
  `commands.rs:406-430`、`install_skill`）最终都走 `install()`，一处收口全覆盖。
- 前端加 `src/targets-links.ts`，导出 `installationTargets(snap)`（已启用、且排除"节点已启用时的关联子项"）
  和 `linkedChildren(nodeId, targets)`；把下面这些"选安装位"的地方从 `targets.filter(t => t.enabled)`
  换成它：
  - `src/views/library.ts:738`（技能卡每 Agent 徽章）、`:845`、`:866`（行尾总开关）
  - `src/views/skill.ts:159`（preview_cells 请求）、`:191`（Agent 网格）
  - `src/views/combos.ts:140`（组合编辑器的「应用到 Agents」）
  - `src/post-add.ts:73`（添加来源后的「全部安装」勾选列表）
  - `src/views/source-skills.ts:118` 是按台账反查目标（读侧），保持不动
- `groups::preview_cells / apply_cells` 同样按解析后的 id 计算，否则组合矩阵里会出现幽灵格子。

## 6. UI

### Agents 页（`src/views/targets.ts`）

先按节点把目标分三组：节点本身 / 直接显示的普通 Agent / 关联子项。

- 开关 **开**（默认）：和今天一样平铺，节点作为一行出现（名称「通用技能目录」，路径 `~/.agents/skills`），
  关联子项行尾加一枚淡淡的「共用」小胶囊，tooltip 写明"安装落在 ~/.agents/skills"，点胶囊跳到节点页。
- 开关 **关**：关联子项从「已启用」和「未启用」两个区块里**整体移除**（未启用面板的计数也要扣掉）；
  节点行下面加一颗胶囊「关联 N 个 Agent」，点击就地展开一个**关联列表**：
  图标 + 名称 + 自己的目录 + 该目录下技能数 + 「编辑」「解除关联」两个按钮，点行进该 Agent 的 skills 页。
  列表底部一行「添加关联 Agent」：弹窗从现有目标里挑（排除节点自己、已关联项），写入 `linked_to`。
- 节点行的开关 tooltip 补一句"停用后，关联的 N 个 Agent 也不再接收安装"——停用的后果必须写明白。
- 拖拽排序只作用于可见的已启用行；`reorder_targets`（`groups.rs:673`）本来就会把未列出的 id 按原相对
  顺序补到后面，被折叠的子项顺序不会乱。
- 节点停用时，关联子项连同节点一起落进「未启用」面板（保持"看得见就管得着"）。

### 设置页（`src/views/settings.ts`）

`通用` 里新增一个分组「Agents 列表」，一行开关：

> **显示已关联的 Agent**
> 关闭后，与「通用技能目录」共读同一目录的 Agent 不再单列，收进该目录的关联列表。

默认 `true`：升级后观感不变，不会出现"我的 Agent 不见了"的错觉；想折叠的用户自己去关（改默认值是一行）。

### 明确不动的

- 「本机 skill 管理」（`src/views/discover.ts`）：左侧 Agent 列表是**扫描范围选择器**，折叠掉的 Agent
  的目录里可能存着手工技能，藏起来就看不了了。这里保留全部 Agent，只加「共用」小胶囊。
- 项目页（`projects.rs agent_slots`）：槽位由 target 目录推导，节点进来后会**自动**多出一个 `.agents/skills`
  项目槽位（大多数新工具项目级也读这个目录），这是白捡的收益；关联子项自己的槽位照旧保留。

## 7. MiMo 接入

- id `mimo`，名称「MiMo」，描述「Xiaomi MiMo Code / 桌面版」，tag `xiaomi`（新 tag）。
- 自己的目录：`~/.config/mimocode/skills/`；CLI：`mimo`（npm `@mimo-ai/cli`）；兜底目录 `.config/mimocode`。
- 默认关联到 `agents-shared`（README 明说 `.agents/skills` 与 `~/.agents/skills` 是兼容发现根）。
- 图标：`src/agent-icons.ts` 加 `mimo -> mimo.svg` + `public/agent-icons/mimo.svg`。图标没到位时
  `agentIconMarkup` 会自动退化成首字母「M」方块，可以先上条目、后补资产。
- **待确认**：官方只写了"桌面版由 MiMo Code 驱动"，没写死桌面版自身的技能根。因为安装位已经交给共享目录，
  即使它自己的目录猜错也只影响"观察"这一个用途，风险可控。

## 8. 迁移（`AppState::load`，紧挨现有的 codex/pi 迁移段 `state.rs:303-334`）

1. `ensure_builtin_targets` 自动补进 `agents-shared` 与 `mimo`（幂等，已有逻辑）。
2. **独占目录改挂**：任何非节点目标的 `skills_dir == ~/.agents/skills`（本机就是 codex）→
   把它的目录改回自己的内置默认（codex → `~/.codex/skills`），并置 `linked_to = agents-shared`、
   `link_touched = true`。
3. **台账改指（必须做）**：`installations` 里 `target_id == 被改挂的 id` 且 `link_path` 落在共享目录下的记录，
   把 `target_id` 改成 `agents-shared`。不做的话，那 59 条台账会挂在 codex 名下、指向一个它"不该管"的目录，
   对账（`symlinks::reconcile`）和「待办」页会误报失效。
4. **默认关联**：`link_touched == false` 且目录仍等于内置默认（没被用户改过）的目标里，id 命中 §2 名单的，
   置 `linked_to = agents-shared`。用户点过「解除关联」的不会被再次关联（`link_touched` 挡住）。
5. 单测：`state.rs` 现有 backfill 测试旁边补"改挂 + 台账改指 + 幂等"三条；`detect.rs` 补 MiMo 规则测试。

## 9. 改动清单

**后端**

- `src-tauri/src/state.rs` — `AgentTarget` 三字段、`UiPrefs.show_linked_agents`、`default_targets()` 两个新条目、
  `load()` 迁移段、测试。
- `src-tauri/src/detect.rs` — `rule_for` 加 `mimo` / `agents-shared`。
- `src-tauri/src/symlinks.rs` — `install()` / `uninstall()` 的关联重定向（含停用节点的报错文案）。
- `src-tauri/src/groups.rs` — `get/set_show_linked_agents`、`set_target_link(id, node_id | None)`、
  `preview_cells` / `apply_cells` 走解析后的 id。
- `src-tauri/src/lib.rs` — 注册两个新命令。
- `src-tauri/src/targets.rs` — 只在 `classify_path` 文案上确认节点能被正确识别（无需逻辑改动）；
  `dir_taken` 保持现状即可（子项保留自己的目录，不撞车）。

**前端**

- `src/types.ts` — `AgentTarget` 三字段镜像。
- `src/prefs.ts` / `src/api.ts` — 开关缓存 + 两个新命令。
- `src/targets-links.ts`（新）— `installationTargets()` / `linkedChildren()`。
- `src/views/targets.ts` — 主要改动：分组、折叠、关联列表、添加/解除关联、节点 tooltip。
- `src/views/settings.ts` — 新分组与开关。
- `src/views/library.ts`、`src/views/skill.ts`、`src/views/combos.ts`、`src/post-add.ts` — 换成 `installationTargets()`。
- `src/agent-icons.ts` + `public/agent-icons/mimo.svg`。

**文档**

- `docs/guide/agents.md` — 新增「通用技能目录与关联」小节；`docs/guide/index.md` 视需要提一句。

## 10. 实施顺序

1. **P0 数据与后端语义**：字段 + 两个内置项 + 迁移（含台账改指）+ `install()` 重定向 + 单测。
   `cargo test` 全绿 + 手验"迁移后 59 条台账归到节点、codex 回到 `~/.codex/skills`"。
2. **P1 前端折叠**：`targets-links.ts` → 各安装位入口改造 → Agents 页折叠与关联列表 → 设置开关。
3. **P2 MiMo 与文档**：图标资产、`docs/guide/agents.md`。
4. **P3 可选**：Zed / Amp 条目（需要先定"没有自己目录的 Agent"表示法）；节点详情页头部的关联 Agent 图标行。

## 11. 风险与待确认

| # | 事项 | 处理 |
| --- | --- | --- |
| 1 | 台账改指漏掉 → 待办页误报"软链丢失" | 迁移里一并改指 + 单测覆盖，验收时对着「待办」页看一眼 |
| 2 | 关联后 Cursor 自己的目录不再接收安装（有人想给单个 Agent 装私货） | 「解除关联」一键恢复；关联列表里写明"安装落在共享目录" |
| 3 | 老配置里中央仓库路径 == `~/.agents/skills` | 加一条前置检查：命中时在节点行显示警告，并在安装时报"这个目录同时是中央仓库，请先在设置里改中央仓库路径"，比现在的"真实目录冲突"文案清楚 |
| 4 | 折叠开关和「本机 skill 管理」的 Agent 列表口径不一致 | 有意的（那里是扫描范围选择器），本期不动；若嫌乱再做二期统一 |
| 5 | MiMo 桌面版技能根未经官方确认 | 安装位已交给共享目录，猜错只影响观察 |
| 6 | `show_linked_agents` 默认值 | 建议 `true`（观感不变）。想一升级就看到整洁列表的话改成 `false` 即可 |
| 7 | 共享目录节点是否再加一个额外地址 `~/.config/agents/skills`（Vercel skills CLI / Amp / Kimi 推荐的通用全局根） | 可选，加了就能在本机 skill 管理里看到那批技能；不加不影响 |

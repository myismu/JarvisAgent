# JarvisAgent

<div align="center">

**一个 AI 驱动的桌面端编程助手**

基于 Tauri 2.1 + Vue 3 + Rust 构建，完整 Agent 自主循环，支持 54 个主流 LLM 模型（12 家厂商），具备快照版本控制、多 Agent 沙箱、方案审批、双轴模式系统等企业级能力

</div>

---

## ✨ 特性

- **多模型支持** — DeepSeek、Claude、GPT、Gemini、Qwen、豆包、MIMO 等 54 个主流 LLM，覆盖 12 家厂商
- **双轴模式系统** — Audience（User/Developer）× WorkMode（Edit/Plan）+ 独立权限档位（请求审批/帮我批准）。工具定义列表保持恒定以命中 prompt cache；"能不能做"由能力清单 + 目录过滤 + 运行时校验保证，"要不要问"由执行前判定按结构化事实决定
- **完整 Agent 循环** — 五阶段管线：初始化 → 意图验证 → 上下文构建 → 主循环（调模型 / 流式解析 / 执行工具 / 反思审查）→ 收尾沉淀
- **自动模式切换** — Edit 模式检测到复杂任务自动切换到 Plan 深度规划，审批后切回委派子 Agent 并行执行
- **快照引擎** — 文件级树形版本控制，原子化回滚，分支管理，多 Agent 沙箱并行与合并
- **子代理委派** — 主代理编排任务图，子 Agent 在干净上下文中独立执行，调度器自动并行
- **方案审批机制** — 复杂任务先提交结构化方案，用户预览编辑后批准，按依赖图调度执行
- **会话持久化** — SQLite 存储完整对话历史，视图引用方案消除双全量存储，多会话管理
- **记忆系统** — 全局记忆文件（agent_home/global/global_memory.md）跨会话沉淀用户偏好与项目上下文；主 Agent 用 ReadMemory / UpdateMemory 直接读写，每轮上下文只注入「身份 + 交互偏好」精简画像，超出预算才后台整理压缩
- **任务调度器** — 基于依赖图的并行调度，JoinSet 流式执行，无依赖任务自动并行，5 分钟超时保护
- **统一上下文压缩** — 使用单一压缩策略（自动摘要 + transcript），简化层级
- **现代 UI** — 类 IDE 毛玻璃界面（Glassmorphism），无边框窗口，明暗主题切换
- **中途取消** — 随时中断正在执行的 Agent 任务，保留部分输出
- **多模态理解** — 图片输入，自动压缩优化
- **Shell 安全** — 递归列目录强制排除依赖目录，危险命令检测，权限分级审批

## 🏗️ 技术栈

| 层级 | 技术 | 说明 |
|------|------|------|
| 前端框架 | Vue 3 + TypeScript | Composition API + `<script setup>` |
| 状态管理 | Pinia | 5 个 Store：session / chat / agent / permission / appView |
| 桌面框架 | Tauri 2.1 | Rust 后端，轻量高性能 |
| 后端运行时 | Rust + Tokio | 异步运行时，SSE 流式处理 |
| HTTP 客户端 | Reqwest | 流式 API 调用，OpenAI / Anthropic 双格式 |
| 数据库 | SQLite (rusqlite) | 会话、消息、快照、任务、运行记录 |
| 分词器 | tiktoken-rs | BPE 精确 Token 计数 |
| Markdown | marked | GFM + 增量渲染 |
| 构建 | Vite 6 | 极速 HMR 开发体验 |

## 🚀 快速开始

### 环境要求

- Node.js >= 18
- Rust >= 1.70
- pnpm >= 8

### 安装与运行

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 开发模式（热更新）
pnpm tauri build      # 生产构建
cargo test            # Rust 测试（在 src-tauri/ 下运行）
```

## ⚙️ 配置

首次运行点击设置按钮配置：

1. **API Key** — LLM API 密钥
2. **Base URL** — API 端点地址（自动补全路径）
3. **API Format** — `openai` 或 `anthropic` 格式
4. **主模型** — 主代理和子代理使用的模型
5. **工具模型** — 意图分类和记忆管理的轻量模型

支持多预设（Profile）管理，不同场景快速切换。配置保存采用原子写入（先写 tmp 再 rename），防止崩溃丢配置。

### 双轴模式与权限档位

两个**互相独立**的模式轴（Audience × WorkMode，历史上的 Chat / 只读保护模式已取消）：

| 模式轴 | 取值 | 作用 |
|---|---|---|
| **工作模式**（WorkMode） | Edit / Plan | 决定"直接干"还是"先出方案"：Plan 模式下写工具不可用，必须先 ProposePlan 并等审批 |
| **用户类型**（Audience） | 普通用户 / 开发者 | 只影响 UI 渲染与交流风格，不影响工具可用性 |

**权限档位**是与模式轴独立的安全维度（不是第三个模式轴）：**请求审批（默认）** 下改文件/删文件/跑命令一律先问；**帮我批准** 下只有删除、覆盖、改名、跑命令、批量改动才问。

权限判定的落点是**结构化事实**（用哪个工具、目标路径、文件是否已存在、一次影响几个文件、
项目内还是项目外），而不是读用户措辞猜意图；关键词只用于弹窗里的风险提示文案。

## 📁 项目结构

```
JarvisAgent/
├── src/                              # Vue 3 前端（主应用 + 监控页双入口）
│   ├── main.ts / App.vue             # 应用入口与根布局
│   ├── monitor-main.ts / MonitorApp.vue   # 独立监控页应用（monitor.html 入口）
│   ├── i18n.ts / locales/            # vue-i18n 国际化（zh-CN / en-US）
│   ├── vite-env.d.ts / PROJECT_STRUCTURE.md   # Vite 类型声明 / 前端结构说明
│   ├── api/ / pages/                  # 预留目录（暂空）
│   ├── assets/                        # 静态资源（global.css 等）
│   ├── types/index.ts                # 全部 TypeScript 类型定义
│   ├── stores/                       # Pinia 状态管理
│   │   ├── session.ts                #   会话生命周期 + 消息缓冲区
│   │   ├── chat.ts                   #   核心交互：发送/取消/撤回/渲染
│   │   ├── agent.ts                  #   Agent/子代理运行状态追踪
│   │   ├── permission.ts             #   权限请求 + 方案审批状态
│   │   └── appView.ts                #   视图/面板显隐偏好
│   ├── composables/
│   │   ├── useAgentEvents.ts         #   后端事件监听中枢 → 分发到各 Store
│   │   ├── useTheme.ts               #   亮/暗主题切换
│   │   ├── useWindow.ts              #   Tauri 窗口控制
│   │   └── usePreferences.ts         #   用户偏好持久化
│   ├── utils/
│   │   ├── toolDisplay.ts            #   工具调用分组与展示摘要
│   │   ├── agentTurnRender.ts        #   Agent 轮次渲染
│   │   ├── agentTurnState.ts         #   单轮 Agent 状态更新
│   │   ├── historyRender.ts          #   历史消息渲染
│   │   ├── timeline.ts               #   时间线数据处理
│   │   ├── markdown.ts               #   Markdown 渲染
│   │   └── html.ts                   #   HTML 工具函数
│   ├── services/
│   │   └── snapshotService.ts        #   快照 API 封装
│   └── components/
│       ├── layout/                   # TitleBar, Sidebar
│       ├── chat/                     # ChatArea, TerminalInput, AgentPanel, AgentTurn,
│       │                             # ExecutionPanel, ThinkingStatus, TodoPanel, ToolCallGroup,
│       │                             # PermissionCard, ContextInspector, SessionTaskBoard,
│       │                             # AgentSnapshotSection, WelcomeScreen
│       ├── common/                   # PermissionModal, PlanPreviewPanel, ConfirmModal,
│       │                             # RollbackConfirmModal, StreamingMarkdown
│       ├── checkpoint/               # CheckpointTimeline
│       ├── snapshot/                 # SnapshotTimeline, DiffViewer, LivePreview
│       ├── settings/                 # SettingsPanel（多预设 + 双轴选择）
│       ├── skill/                    # SkillManager, SkillMarket, SkillCard, SkillDetailPanel
│       └── todo/                     # Todo 组件（暂空）
├── src-tauri/                        # Rust 后端（三层：infra / core / command）
│   ├── src/
│   │   ├── lib.rs                    # Tauri 入口：数据目录 + 状态注册 + 命令绑定
│   │   ├── main.rs                   # 二进制入口
│   │   ├── infra/                    # ── 基础设施层：配置 / 数据库 / LLM / 状态 ──
│   │   │   ├── config/               #   config.rs（AgentConfig + 原子写入）+ data_paths.rs
│   │   │   ├── db/                   #   mod.rs（SQLite 连接）+ schema.rs（19 张表 + 迁移）
│   │   │   ├── llm/                  #   api_format + api_client（指数退避 + Retry-After）
│   │   │   │                         #   + adapters + registry（模型能力）+ token_count
│   │   │   ├── providers/            #   anthropic.rs / openai.rs（双协议实现）
│   │   │   ├── state/                #   state.rs（SessionManager/WorkspaceState）+ events.rs
│   │   │   ├── types/                #   models.rs + traits.rs（LlmProvider）+ error.rs + constants.rs
│   │   │   ├── background.rs         #   后台任务管理 + Tauri 事件推送
│   │   │   └── debug_logger.rs       #   调试日志
│   │   ├── core/                     # ── 业务层：Agent / 工具 / 编排 / 回滚 / 会话 ──
│   │   │   ├── agent/
│   │   │   │   ├── pipeline.rs       #   五阶段 Agent 管线主循环（压缩 + 反思）
│   │   │   │   ├── stream.rs         #   SSE 流式解析（Anthropic + OpenAI）
│   │   │   │   ├── context.rs        #   动态上下文构建
│   │   │   │   ├── tools_runner.rs   #   工具调用并行执行引擎
│   │   │   │   ├── prompts.rs            #   系统提示词多维组装（按优先级 P0/P1/P2 分层）
│   │   │   │   ├── prompts/          #   提示词模板（audience / mode / os / base）
│   │   │   │   └── reflection/       #   反思审查（mod / prompt / strategy）
│   │   │   ├── intent/               #   三层意图分类（规则→上下文→LLM 兜底）
│   │   │   │                         #   + plan_detector（复杂任务检测）
│   │   │   ├── orchestration/        #   scheduler + subagents + agent_runs + tasks
│   │   │   │   └── multi_agent/      #   沙箱（sandbox）+ 分支合并（merge）
│   │   │   ├── rollback/             #   snapshot + patch + replay + store + gc + journal
│   │   │   ├── session/              #   会话持久化 + memory（单级摘要压缩 + 裁剪）+ repository
│   │   │   └── tools/                #   工具系统中枢 + 8 个子系统：
│   │   │       ├── file_tools/       #   文件读写/编辑/搜索/符号（14 个文件）
│   │   │       ├── shell_tools/      #   Shell + 后台任务 + Git + 安全检查
│   │   │       ├── agent_tools/      #   子代理、技能、压缩、方案审批、模式切换、记忆
│   │   │       ├── task_tools/       #   持久化任务 CRUD + 轻量待办
│   │   │       ├── search_tools/     #   Glob + Grep 搜索
│   │   │       ├── notebook_tools/   #   Jupyter Notebook cell 编辑
│   │   │       ├── system_tools/     #   系统信息 + 工作区设置
│   │   │       └── framework/        #   工具注册表、权限、渐进式披露、调用日志
│   │   └── command/                  # ── Tauri 命令层（12 个文件）──
│   │       ├── session.rs            #   会话 CRUD + 撤回 + 后台任务 + 子代理查询
│   │       ├── config.rs / app_config.rs  # 配置读写 / 窗口状态 + UI 偏好
│   │       ├── checkpoint.rs / snapshot.rs # 检查点回滚 / 快照引擎命令
│   │       ├── permission.rs / history.rs  # 权限回调 / 历史渲染
│   │       ├── history_types.rs      #   历史渲染类型定义
│   │       ├── mod.rs                #   命令模块注册
│   │       └── merge.rs / sandbox.rs / skill.rs
│   ├── capabilities/                 # Tauri 权限能力声明
│   ├── gen/ / icons/                  # Tauri 生成目录 / 应用图标
│   ├── model_registry.json           # 模型能力注册表（编译时内嵌，54 个模型）
│   ├── intent_rules.json             # 意图分类外部规则（10 类）
│   ├── tauri.conf.json               # Tauri 应用配置
│   ├── build.rs / Cargo.toml / Cargo.lock   # 构建脚本 / 依赖清单 / 锁文件
│   └── .taurignore                   # Tauri 打包忽略规则
├── skills/                           # 内置技能目录（SKILL.md）
├── demo/                             # Agent 机制演示脚本（s01~s12）
├── doc/                              # 架构文档
├── data/                             # 运行期数据（SQLite / 配置 / 会话）
├── index.html / monitor.html         # Vite 双入口
└── package.json / pnpm-workspace.yaml
```

## 🔧 核心架构（已重构）

### 新的 Agent 管线

```
用户输入 → 多层意图分类（规则 → 上下文 → LLM）
           → 组装工具定义（恒定列表，保证 prompt cache 命中）
           → 动态上下文注入（意图标签 + 项目索引 + 用户画像）
           → 模块化子 Agent 并行执行（自研调度器）
           → 结果聚合与持久化（快照、会话标题、记忆超预算时后台整理）
```

### 双轴模式系统

```
Audience 轴（谁在用）   WorkMode 轴（在干什么）
  User ── Developer       Edit ────── Plan
  ↑ 只用户手动切换           ↑ 用户手动 + Agent 自动切

  Audience → UI 渲染 + 交流风格
  WorkMode → 系统提示词 + 工具可用性（Plan 禁写）

独立维度 权限档位（改动前问多严，不是第三个模式轴）：
  请求审批 ────── 帮我批准
  ↑ 用户手动切换
  权限档位 → 执行前判定：放行 / 弹窗问 / 直接拒绝
```

提示词按优先级分层组装：基础规则（P0/P1/P2）+ 写操作与编排规则 + Audience 风格 + WorkMode 规则 + OS 规则 + 沙箱约束。模式切换不重启 Pipeline，下一轮 LLM 请求自动使用新提示词。

### 能力清单（Capability Manifest）

工具可用性只有一套口径：`ToolRegistry::is_available(意图 × 工作模式)`。工具目录过滤（GetToolCatalog / DiscoverTools）、搜索、运行时校验（ExecuteTool / 直接调用）、写操作兜底（`should_block_write_tool`）全部由它推导——写操作工具名单也只保留一份（`ToolRegistry::WRITE_TOOLS`）。「目录里看不见的工具」任何路径都调不到。

`Capabilities::for_work_mode()` 把"当前模式能做什么"编译成能力清单，并且是**从注册表反推**的（能力清单 = 工具目录的投影），不存在第二套真相。同一个清单同时喂给四层：

```
① 能力冲突快速判定  受限模式（Plan）下用户要求了被禁能力 → 直接给确定性回复
                    （0 次模型调用、0 次工具往返）
② 第一轮上下文 <capabilities> 声明能力边界，并明令禁止用发现类工具去试探
③ 目录/搜索  一次调用就给出能力结论，不让模型从"列表里没有"去推断
④ 运行时校验 模型即便硬调也拿不到 schema、调不通（兜底）
```

判定"用户这条消息要什么能力"用**规则层**（`rules.rs` 的唯一一张表，`classify_by_rules_detailed()` 返回 `RuleCategory`）+ `intent::mode_conflict` 的误拦防护（提问式表达放行、纯内容请求放行、要求改文件必须有明确的项目工件引用或已绑定工作区）。判不出来就交给主模型回答——本轮已带能力边界声明，不会产生工具往返。

### 权限档位与执行前判定

```
工具调用
  ↓ ① 采集事实：用哪个工具 / 目标路径 / 文件是否已存在 / 影响几个文件 / 现有守卫怎么说
  ↓ ② 查"本会话已允许"（工具 + 范围：文件类=目录，命令类=同一条命令）
  ↓ ③ 判定：放行 → 执行 ｜ 要问 → 弹窗 ｜ 拒绝 → 不执行并把原因回灌给模型
```

- 判定器 `policy::judge()` 是纯函数；事实采集 `policy_guard::prepare_facts()` 与执行前判定共用同一份实现，保证口径一致。
- 覆盖范围：主助手、子代理、通过 ExecuteTool 代理执行的调用，**全部**经过同一道门。
- 弹窗三个按钮：允许本次调用 / 本次会话都允许（写明精确范围）/ 拒绝（可带说明回灌给模型）。

### 视图引用方案（数据库）

```
session_messages 表（唯一数据源，只增不减）
  └── message_id + content_json + seq + source（chat/compact）

session_memory 表（LLM 活动视图索引）
  └── message_ids: ["id1", "id2", ...]  ← 不存消息内容，只存 ID 列表

压缩：摘要写入 session_messages（source='compact'）→ 更新 message_ids → 原始消息完整保留
回滚：隐藏检查点后的消息 → 更新 message_ids → 原始消息从 session_messages 恢复
```

### 上下文压缩（单级 LLM 摘要）

```
发送前裁剪（每轮恒定）：
- 过滤 internal/background 内部消息，只保留 chat/compact/context 来源
- CHAT 模式把工具返回详情折叠为一行（错误信息保留）
- 图片仅保留最近 2 条消息中的，远期图片折叠为文本摘要
- 单条工具结果超过 5 万字符截断

LLM 摘要压缩（auto_compact，唯一真正的压缩）：
- 触发：本地估算 token 超过上限（100k）的 70%（自动），或 LLM 调用 CompactConversation（手动）
- 流程：旧历史备份为 JSONL transcript → 删 internal/background → 保留最近 6 条消息 →
  其余交轻量模型总结 → 用 source='compact' 摘要 + 最近消息替换
- 原始消息在 session_messages 中完整保留，可随时恢复

备注：ConsolidateMemory 是全局记忆整理（合并/压缩用户档案），只写记忆文件，不修改对话历史。
```

### 快照引擎

```
代码变更 → Patch（Create/Update/Delete/Rename）→ Snapshot（版本快照）
         → 树形分支管理 → AtomicFileRollback（staging + rename 原子写入）
         → 多 Agent 沙箱 → 分支合并（LCA 三方合并 + 冲突解决）
         → 增量检查点（基于前一个 checkpoint workspace_state 增量计算）
         → GC 三阶段清理（脱离分支节点 → 孤儿快照 → 孤儿内容）
```

### 调度器

```
CreateTask 创建任务图 → UpdateTask 设 blocked_by 依赖
  → RunSubagentsSequentially 启动调度器
    → JoinSet 流式调度（谁先完成就解锁谁的下游）
    → 无依赖任务自动并行
    → 5 分钟超时保护
```

### 前端事件架构

```
Rust emit("chat-content") ──→ useAgentEvents.listen()
                            ├──→ sessionStore（写入 buffer）
                            ├──→ chatStore.triggerRender()（增量渲染）
                            ├──→ agentStore（更新运行状态）
                            └──→ permissionStore（弹出确认弹窗）

后台任务完成 → emit("bg-task-done") → AgentPanel 0 延迟更新（替代轮询）
子 Agent 状态  → emit("subagent-updated") → 2s 批量节流更新
```

## 🛠️ 内置工具一览

| 类别 | 工具 | 说明 |
|------|------|------|
| 文件读取 | ReadFile, ReadFileSkeleton | 读文件全文/骨架，支持行号范围 |
| 文件写入 | WriteFile, EditFile, ApplyPatch, DeleteFile, RenameFile | 写文件、搜索替换编辑、应用 diff、删除/重命名 |
| 目录 | ListDirectory, SearchRepo | 列目录、生成仓库地图 |
| 搜索 | SearchText, FindFiles | 文本搜索(grep)、文件名搜索(glob) |
| 符号 | FindSymbol, ReadSymbol, FindReferences, CodeSearch | 符号定位、读定义、查引用 |
| Shell | RunCommand, StartBackgroundCommand, CheckBackgroundCommand | 命令执行、后台服务、状态检查 |
| Git | RunGitCommand | 只读 Git 操作 |
| 任务 | CreateTask, UpdateTask, DeleteTask, ListTasks, GetTask, SummarizeTasks | 持久化任务图 CRUD |
| 待办 | UpdateTodos | 轻量待办清单 |
| 子代理 | RunSubagent, RunSubagentsSequentially | 委派子代理、启动调度器 |
| 规划 | ProposePlan, SwitchWorkMode | 方案审批、模式切换 |
| 工具发现 | DiscoverTools, GetToolCatalog, ExecuteTool | 渐进式披露：搜索/列举/执行延迟工具 |
| 会话 | CompactConversation | 手动压缩对话历史 |
| 记忆 | ReadMemory, UpdateMemory, ConsolidateMemory | 读/写/整理全局记忆文件 |
| 系统 | SetWorkspace | 工作区设置（OS/工作目录已由提示词自动注入，无需工具） |
| 技能 | LoadSkill | 按名称加载技能知识 |
| Notebook | EditNotebook | Jupyter Notebook cell 编辑 |

## 🛡️ 安全特性

- **沙箱限制** — 会话绑定工作目录，路径遍历自动拦截
- **Shell 安全** — 递归列目录（dir /s、tree）强制排除 node_modules 等依赖目录
- **权限审批** — Shell 等敏感操作需用户确认，系统无限期等待你的决策（无自动超时）；拒绝时可附一句说明，直接回灌给模型
- **循环检测** — Agent 循环超 30 轮暂停确认，绝对上限 200 轮
- **429 限流** — 解析 Retry-After 头按服务器建议等待
- **快照回滚** — 所有文件操作可追溯可撤销，原子化写入防崩溃

## 🙏 致谢

- **[learn-Code-code](https://github.com/shareAI-lab/learn-claude-code)** — 架构设计的灵感来源

## 📄 许可证

MIT License

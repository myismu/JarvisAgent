# JarvisAgent

<div align="center">

**一个 AI 驱动的桌面端编程助手**

基于 Tauri 2 + Vue 3 + Rust 构建，完整 Agent 自主循环，支持 93 个主流 LLM 模型（12 家厂商），具备快照版本控制、多 Agent 沙箱、方案审批、双轴模式系统等企业级能力

</div>

---

## 特性

- **完整 Agent 自主循环** — 四阶段流水管线（初始化 → 循环前准备 → 主循环 → 收尾），支持复杂任务自动切换规划模式、结构化方案审批、中途取消与进程崩溃恢复。主循环与子代理调度器以 `select!` 并存，子代理执行期间主代理可继续对话

- **跨模型 Prompt Cache 字节恒定** — 工具定义、系统提示词、消息结构在会话内保持字节级稳定，模式切换、工具开关、提示词编辑都不破坏前缀缓存。这是贯穿提示词 / 工具 / 上下文三个系统的一条硬约束：缓存前缀排在消息之前，动一个字节则后续全部历史失效

- **能力与权限的双层管控** — 「能不能做」由能力清单保证（从工具注册表**反推**，单一事实来源，不存在第二套真相）；「要不要问」由权限判定按**结构化事实**决定（用哪个工具、目标路径、文件是否已存在、一次影响几个文件），不读用户措辞猜意图。另有独立于两者的只读保护闸门，只拒绝不询问

- **文件级版本控制与两条恢复通道** — 快照树 + 分支管理 + 原子回滚（staging + rename）。文本文件走快照内容恢复；目录、二进制、超大文件在快照里存不下内容，走回收站本体恢复——两条通道互补，后者是前者的必要补充

- **多 Agent 编排** — 依赖图任务调度（`JoinSet` 流式执行，完成即级联解锁，无依赖任务自动并行）+ 子代理独立上下文与独立 token 计量 + 沙箱分支与补丁级三方合并

- **上下文压缩、视图引用与全局记忆** — 单级 LLM 摘要压缩，触发阈值用厂商实测值标定（本地估算系统性偏低，长会话实测偏差约 3 倍）；消息只存一份，压缩与回滚只改活动视图的 ID 列表，原始消息随时可恢复；跨会话用户画像独立沉淀，每轮只注入「身份 + 交互偏好」精简画像

- **多模型接入** — 93 个模型 / 12 家厂商，OpenAI 与 Anthropic 双协议。SSE 协议解析收敛在单一模块，业务层只见统一事件；超时分层施加（不设整体时限，以免掐断长流式生成）、固定阶梯重试、429 限流、跨厂商用量字段归一化（未知不冒充 0）

- **桌面端体验** — 类 IDE 毛玻璃界面、明暗主题、中英双语、图片输入；13 个提示词文件与全部工具均可在设置中查看与调整（磁盘优先、内置兜底）

## 技术栈

| 层级 | 技术 | 说明 |
|------|------|------|
| 前端框架 | Vue 3.5 + TypeScript | Composition API + `<script setup>` |
| 状态管理 | Pinia 3 | 5 个 Store：session / chat / agent / permission / appView |
| 桌面框架 | Tauri 2 | Rust 后端，轻量高性能；无边框透明窗口 |
| 后端运行时 | Rust + Tokio | 异步运行时，SSE 流式处理 |
| HTTP 客户端 | Reqwest 0.12 | 流式 API 调用，OpenAI / Anthropic 双协议 |
| 数据库 | SQLite (rusqlite) | 18 张表，会话、消息、快照、任务、运行记录 |
| 分词器 | tiktoken-rs | BPE Token 计数，未知模型回退 cl100k_base，再回退字符估算 |
| 国际化 | vue-i18n 9 | zh-CN / en-US |
| Markdown | marked | GFM + 增量渲染 |
| 构建 | Vite 6 | 极速 HMR，双入口（主应用 + 监控页） |

## 快速开始

### 环境要求

- Node.js >= 18
- Rust >= 1.77.2
- pnpm >= 8

### 安装与运行

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 开发模式（热更新）
pnpm build            # 前端类型检查与构建：vue-tsc --noEmit && vite build
pnpm tauri build      # 生产构建
cargo test            # Rust 测试（在 src-tauri/ 下运行）
```

## 配置

首次运行点击设置按钮配置。设置面板分四个页签：常规 / 预设 / 提示词 / 工具。

1. **API Key** — LLM API 密钥
2. **Base URL** — API 端点地址（按 API 格式自动补全 `/chat/completions` 或 `/messages` 路径）
3. **API Format** — `openai` 或 `anthropic` 格式
4. **主模型** — 主代理和子代理使用的模型
5. **工具模型**（utility model）— 上下文压缩摘要、全局记忆整理、反思审查使用的轻量模型
6. **反思模式** — `smart`（默认）/ `always` / `off`

支持多预设（Profile）管理，不同场景快速切换；另有一个「全局预设」用于与主对话无关的后台调用。配置保存采用原子写入（先写 tmp 再 rename），防止崩溃丢配置。

### 双轴模式与权限档位

两个**互相独立**的模式轴（Audience × WorkMode；历史上的 Chat 模式已取消，只读保护则从模式轴降为一个独立闸门，见下）：

| 模式轴 | 取值 | 作用 |
|---|---|---|
| **工作模式**（WorkMode） | Edit / Plan | 决定「直接干」还是「先出方案」：Plan 模式下写工具不可用，必须先 ProposePlan 并等审批；探索后发现影响面很小（1~2 个文件）可自行降级回 Edit |
| **用户类型**（Audience） | 普通用户 / 开发者 | 只影响 UI 渲染与交流风格，不影响工具可用性 |

**权限档位**是与模式轴独立的安全维度（不是第三个模式轴）：**请求审批（默认）** 下改文件 / 删文件 / 跑命令一律先问；**帮我批准** 下只有删除、覆盖、改名、跑命令、批量改动才问。

权限判定的落点是**结构化事实**（用哪个工具、目标路径、文件是否已存在、一次影响几个文件、项目内还是项目外），而不是读用户措辞猜意图；关键词只用于弹窗里的风险提示文案。

此外还有一道**只读保护**硬闸门：独立于档位，不看工作模式、切模式也绕不过，且只能拒绝不能询问。

## 项目结构

```
JarvisAgent/
├── src/                              # Vue 3 前端（主应用 + 监控页双入口）
│   ├── main.ts / App.vue             # 应用入口与根布局
│   ├── monitor-main.ts / MonitorApp.vue   # 独立监控窗口应用（monitor.html 入口）
│   ├── i18n.ts / locales/            # vue-i18n 国际化（zh-CN / en-US）
│   ├── vite-env.d.ts / PROJECT_STRUCTURE.md   # Vite 类型声明 / 前端结构说明
│   ├── api/ / pages/                 # 预留目录（暂空）
│   ├── assets/global.css             # CSS 变量主题 + 毛玻璃令牌
│   ├── types/index.ts                # 全部 TypeScript 类型定义
│   ├── stores/                       # Pinia 状态管理
│   │   ├── session.ts                #   会话生命周期 + 消息缓冲区 + 分页游标
│   │   ├── chat.ts                   #   核心交互：发送/取消/撤回/渲染/懒加载
│   │   ├── agent.ts                  #   Agent/子代理运行状态 + 上下文快照 + 面板显隐
│   │   ├── permission.ts             #   权限请求 + 方案审批状态（按会话排队）
│   │   └── appView.ts                #   主内容区视图切换（聊天 / 技能管理）
│   ├── composables/
│   │   ├── useAgentEvents.ts         #   后端事件监听中枢 → 分发到各 Store
│   │   ├── useTheme.ts               #   亮/暗主题切换（同步监控窗口）
│   │   ├── useWindow.ts              #   Tauri 窗口控制 + 跨窗口同步
│   │   ├── usePreferences.ts         #   用户偏好持久化
│   │   ├── useThinkingMode.ts        #   深度思考档位（会话级，发送时落库）
│   │   └── useToast.ts               #   全局轻提示单例队列
│   ├── utils/
│   │   ├── toolDisplay.ts            #   工具调用分组与展示摘要
│   │   ├── agentTurnRender.ts        #   Agent 轮次渲染
│   │   ├── agentTurnState.ts         #   单轮 Agent 状态更新
│   │   ├── contextUsage.ts           #   上下文占用读数唯一口径（实测优先，估算兜底）
│   │   ├── thinking.ts               #   深度思考纯函数（Rust 侧镜像，共用测试向量）
│   │   ├── timeline.ts               #   时间线数据处理
│   │   └── markdown.ts               #   Markdown 渲染
│   ├── services/
│   │   └── snapshotService.ts        #   快照 API 封装
│   └── components/
│       ├── layout/                   # TitleBar, Sidebar
│       ├── chat/                     # ChatArea, TerminalInput, AgentPanel, AgentTurn,
│       │                             # AgentTurnNotice, MessageRail, ExecutionPanel,
│       │                             # ThinkingStatus, TodoPanel, ToolCallGroup,
│       │                             # PermissionCard, ContextInspector, CacheHitTooltip,
│       │                             # SessionTaskBoard, AgentSnapshotSection, WelcomeScreen
│       ├── common/                   # PermissionModal, PlanPreviewPanel, ConfirmModal,
│       │                             # RollbackConfirmModal, StreamingMarkdown, ToastHost
│       ├── checkpoint/               # CheckpointTimeline
│       ├── snapshot/                 # SnapshotTimeline, DiffViewer, LivePreview, FileOpIcon
│       ├── settings/                 # SettingsPanel（四页签）+ PromptsTab + ToolsPanel
│       ├── skill/                    # SkillManager, SkillMarket, SkillCard, SkillDetailPanel
│       └── todo/                     # Todo 组件（暂空）
├── src-tauri/                        # Rust 后端（三层：infra / core / command）
│   ├── src/
│   │   ├── lib.rs                    # Tauri 入口：数据目录 + 状态注册 + 105 个命令绑定
│   │   ├── main.rs                   # 二进制入口
│   │   ├── infra/                    # ── 基础设施层：配置 / 数据库 / LLM / 状态 ──
│   │   │   ├── config/               #   config.rs（AppConfig + 预设 + 原子写入）+ data_paths.rs
│   │   │   ├── db/                   #   mod.rs（SQLite 连接）+ schema.rs（18 张表 + 迁移 v1~v20）
│   │   │   ├── llm/                  #   api_format + api_client（固定阶梯重试 + Retry-After）
│   │   │   │                         #   + adapters + registry（模型能力注册表）
│   │   │   │                         #   + stream_parse（SSE 帧解析唯一实现）+ token_count
│   │   │   │                         #   + context_budget（压缩判据）+ usage + usage_memory
│   │   │   ├── providers/            #   anthropic.rs / openai.rs（双协议请求体构建）
│   │   │   ├── state/                #   state.rs（SessionManager/WorkspaceState）
│   │   │   ├── types/                #   models.rs + traits.rs（LlmProvider）+ error.rs + constants.rs
│   │   │   ├── background.rs         #   后台任务管理 + Tauri 事件推送
│   │   │   └── debug_logger.rs       #   调试日志
│   │   ├── core/                     # ── 业务层：Agent / 工具 / 编排 / 回滚 / 会话 ──
│   │   │   ├── agent/
│   │   │   │   ├── pipeline.rs       #   四阶段 Agent 管线主循环（压缩 + 反思 + Plan 看门狗）
│   │   │   │   ├── stream.rs         #   流式事件副作用处理（协议解析在 infra/llm/stream_parse）
│   │   │   │   ├── context.rs        #   动态上下文构建（快照 + 用户消息 + 深度思考决策）
│   │   │   │   ├── tools_runner.rs   #   工具调用并行执行引擎
│   │   │   │   ├── prompts.rs        #   系统提示词多维组装（按 P0/P1/P2 分层 + 磁盘覆盖层）
│   │   │   │   ├── prompts/          #   13 个提示词模板（base / audience / mode / os / subagent）
│   │   │   │   └── reflection/       #   反思审查（mod / prompt / strategy）
│   │   │   ├── complex_task.rs       #   复杂任务检测：is_complex_task 命中 → 强制 Plan 审批
│   │   │   │                         #   + detect_plan_in_text 正文方案检测 → 重定向 ProposePlan
│   │   │   ├── orchestration/        #   scheduler + subagents + agent_runs + tasks
│   │   │   │   │                     #   + agent_run_repository
│   │   │   │   └── multi_agent/      #   沙箱（sandbox）+ 分支合并（merge）
│   │   │   ├── rollback/             #   snapshot + patch + replay + store + gc + journal
│   │   │   │                         #   + trash（回收站）+ rollback_logger
│   │   │   │                         #   + session_manager（检查点 + GC 触发）
│   │   │   ├── session/              #   会话持久化 + memory（单级摘要压缩）+ thinking（档位裁决）
│   │   │   │                         #   + repository + resource_repository（transcript 等资源）
│   │   │   └── tools/                #   工具系统中枢 + 8 个子系统：
│   │   │       ├── file_tools/       #   文件读写/编辑/搜索/符号（14 个文件）
│   │   │       ├── shell_tools/      #   Shell + 后台任务 + 安全检查（git 走 RunCommand，无专用工具）
│   │   │       ├── agent_tools/      #   子代理、技能、压缩、方案审批、模式切换、记忆
│   │   │       ├── task_tools/       #   持久化任务 CRUD + 轻量待办
│   │   │       │   └── persistent/   #   任务 CRUD 实现（create/update/delete/list/get/summary）
│   │   │       ├── search_tools/     #   Glob + Grep 搜索
│   │   │       ├── notebook_tools/   #   Jupyter Notebook cell 编辑
│   │   │       ├── system_tools/     #   系统工具（当前无模型可调用项，见模块注释）
│   │   │       └── framework/        #   工具注册表、能力清单、权限判定、渐进式披露、
│   │   │                             #   允许清单落盘、审计与调用日志、工具开关过滤
│   │   └── command/                  # ── Tauri 命令层（12 个模块 / 14 个文件）──
│   │       ├── session.rs            #   会话 CRUD + 撤回 + 后台任务 + 子代理查询
│   │       ├── config.rs / app_config.rs  # 配置读写 / 窗口状态 + UI 偏好
│   │       ├── checkpoint.rs / snapshot.rs # 检查点回滚 / 快照引擎命令
│   │       ├── permission.rs / history.rs  # 权限回调 / 历史渲染
│   │       ├── prompt.rs / tool.rs   #   提示词磁盘化读写 / 工具开关
│   │       ├── history_types.rs      #   历史渲染类型定义
│   │       ├── mod.rs                #   命令模块注册
│   │       └── merge.rs / sandbox.rs / skill.rs
│   ├── capabilities/                 # Tauri 权限能力声明
│   ├── gen/ / icons/                 # Tauri 生成目录 / 应用图标
│   ├── model_registry.json           # 模型能力注册表（编译时内嵌，93 个模型 / 12 家厂商）
│   ├── tauri.conf.json               # Tauri 应用配置
│   ├── build.rs / Cargo.toml / Cargo.lock   # 构建脚本 / 依赖清单 / 锁文件
│   └── .taurignore                   # Tauri 打包忽略规则
├── skills/                           # 内置技能目录（SKILL.md）：agent-builder / code-review /
│                                     # file-header-doc / mcp-builder / pdf / ui-ux-pro-max
├── demo/                             # 机制演示与参考：s01~s12 Python 脚本 + s_full.py
│                                     # + claudecode_tools（Claude Code 工具源码参考）
│                                     # + 教程页（rust / py-ts）+ 面试准备页
├── doc/                              # 架构与设计文档
├── data/                             # 运行期数据（SQLite / 配置 / 提示词覆盖 / 授权 / 回收站 / 日志）
├── index.html / monitor.html         # Vite 双入口
└── package.json / pnpm-workspace.yaml
```

各层的模块级清单见 `PROJECT_STRUCTURE.md` 与两份分层文档（`src/PROJECT_STRUCTURE.md`、`src-tauri/PROJECT_STRUCTURE.md`）。

## 核心架构

### Agent 管线（四阶段）

```
用户输入
  → 初始化 setup()      纯规则识别复杂任务（命中 → 强制 Plan 审批）；沙箱目录校验；
                       崩溃恢复 + InProgress 任务恢复指令；输入框自然语言审批
  → 循环前准备 pre_loop()  组装系统提示词与工具定义（两者恒定，保证 prompt cache 命中）；
                       上下文快照注入（工作模式规则 + 能力清单 + 用户画像 + 项目结构）；
                       注入用户消息并落库；深度思考整轮一次性裁决；登记 agent_run
  → 主循环 run_main_loop()  调模型 → 流式解析 → 并行执行工具（执行前权限判定）→ 反思审查
                       有活跃调度器时与任务事件 select! 并存
  → 收尾 finalize()     结果聚合与持久化（快照、会话标题、token 计量、记忆超预算时后台整理）
```

### 双轴模式系统

```
Audience 轴（谁在用）   WorkMode 轴（在干什么）
  User ── Developer       Edit ────── Plan
  ↑ 只用户手动切换           ↑ 用户手动 + Agent 自动切

  Audience → UI 渲染 + 交流风格
  WorkMode → 上下文快照（能力边界）+ 工具可用性（Plan 禁写）

独立维度 权限档位（改动前问多严，不是第三个模式轴）：
  请求审批 ────── 帮我批准
  ↑ 用户手动切换
  权限档位 → 执行前判定：放行 / 弹窗问 / 直接拒绝
```

系统提示词按优先级分层组装且**字节恒定**（基础规则 P0/P1/P2 + 写操作与命令执行规则 + Audience 风格 + OS 规则 + 沙箱约束；「上下文快照读取规则」标 P0，按级别排序后落在 P0 段），以命中前缀缓存。提示词支持磁盘覆盖层（`data/prompts/<path>`）：磁盘优先、内置兜底。

WorkMode 规则不进 system，而是在每个用户回合入口注入一次 `<context_snapshot>`。模式切换不打断当前循环：中途 `SwitchWorkMode` 会追加一条 seq 更大的完整快照（新模式提示词 + 能力清单 + 项目结构与用户画像），不回改已发送的旧快照，因此前缀缓存继续命中；同时提示重新拉取工具目录。历史中的旧快照仅作参考。

### 能力清单（Capability Manifest）

工具可用性只有一套口径。可见性判定是**三维**的：

```
ToolRegistry::is_available(工具名 × 意图 × 工作模式 × 用户工具开关)
                          └── 其中工具开关（ToolFilter）优先级最高
```

工具目录过滤（GetToolCatalog / DiscoverTools）、搜索、运行时校验（ExecuteTool / 直接调用）、写操作兜底（`should_block_write_tool`）全部由它推导——写操作工具名单也只保留一份（`ToolRegistry::WRITE_TOOLS`），另有 `PLAN_BLOCKED_EXTRA` 声明规划模式额外收走的子代理工具。「目录里看不见的工具」任何路径都调不到。

`Capabilities::for_work_mode()` 把「当前模式能做什么」编译成能力清单，并且是**从注册表反推**的（能力清单 = 工具目录的投影），不存在第二套真相。同一个清单喂给四个消费方：

```
① 每轮上下文快照  <capabilities> 声明能力边界，并明令禁止用发现类工具去试探
② 目录 / 搜索     一次调用就给出能力结论，不让模型从「列表里没有」去推断
③ 运行时校验      模型即便硬调也拿不到 schema、调不通（兜底）
④ 模式切换时的追加快照  切换后立即用新口径重新声明一次
```

规划模式下「用户要改文件」不再提前截断——交给模型自己解释并走方案审批流程（用户少一次往返）；能力边界由上述四层保证。意图分类已取消，只剩复杂任务检测（`core/complex_task.rs`），唯一用途是识别复杂任务并强制走 Plan 审批，误判可按降级条件自行切回 Edit。

### 权限档位与执行前判定

```
工具调用
  ↓ ① 采集事实：用哪个工具 / 目标路径 / 文件是否已存在 / 影响几个文件 / 现有守卫怎么说
  ↓ ② 查「已允许」（工具 + 范围：文件类 = 目录，命令类 = 同一条命令）
  ↓ ③ 判定：放行 → 执行 ｜ 要问 → 弹窗 ｜ 拒绝 → 不执行并把原因回灌给模型
```

- 判定器 `policy::judge()` 是纯函数；事实采集 `policy_guard::prepare_facts()` 与执行前判定共用同一份实现，保证口径一致。
- 覆盖范围：主助手、子代理、通过 ExecuteTool 代理执行的调用，**全部**经过同一道门。RunCommand 内层再用同一套允许键判一次，避免重复弹卡。
- 弹窗三个按钮：允许一次 / 本项目允许（写明精确范围，登记后**跨会话记住**并落盘 `data/permissions/`）/ 拒绝（可带说明回灌给模型）。无明确范围键时「本项目允许」不显示。
- 决策态区分「用户拒绝」与「没等到结论」（`Interrupted`），切换档位时会清扫已授权的待决项，每次判定留审计痕迹。

### 视图引用方案（数据库）

```
session_messages 表（唯一数据源）
  └── message_id + content_json + seq + source（chat/compact/context/internal/background）

session_memory 表（LLM 活动视图索引）
  └── message_ids: ["id1", "id2", ...]  ← 不存消息内容，只存 ID 列表

压缩：摘要写入 session_messages（source='compact'）→ 更新 message_ids → 原始消息保留
回滚：隐藏检查点后的消息 → 更新 message_ids → 原始消息从 session_messages 恢复
```

物理删除仅发生在三处：撤回 / 编辑重发需截断、压缩时清掉 internal 与 background、会话彻底删除。其余情况一律靠 `hidden_at` / `recalled_at` 标记控制可见性。历史消息按轮做游标分页懒加载，不一次性灌进前端。

### 上下文压缩（单级 LLM 摘要）

```
发送前裁剪（每轮恒定）：
- 过滤 internal/background 内部消息，只保留 chat/compact/context 来源
- 已发出的历史一律只读：工具结果不做二次折叠（改写缓存前缀得不偿失）
- 图片以「本轮用户消息」为界：本轮始终携带 base64，往轮折叠为 [图片: type] 文本摘要
- 单条工具结果写入时超过 5 万字符即截断

LLM 摘要压缩（auto_compact，唯一真正的压缩）：
- 触发：本地估算 token 超过（模型窗口 − 输出预算）的 85%（自动），
        或 LLM 调用 CompactConversation（手动）
- 分子标定：k = 上轮厂商实测 input ÷ 同源本地估算，避免估算系统性偏低导致漏压
- 流程：旧历史备份为 JSONL transcript → 删 internal/background →
        前缀整体交轻量模型总结 → 用 source='compact' 摘要替换（不留尾巴）
- 原始消息在 session_messages 中保留，可随时恢复

备注：ConsolidateMemory 是全局记忆整理（合并/压缩用户档案），只写记忆文件，不修改对话历史。
```

### 快照引擎

```
代码变更 → Patch（Create/Update/Delete/Rename）→ Snapshot（版本快照）
         → 树形分支管理 → AtomicFileRollback（staging + rename 原子写入，含重试）
         → 多 Agent 沙箱 → 分支合并（LCA 定位 + 补丁级合并 + 冲突解决）
         → 增量检查点（基于前一个 checkpoint 的 workspace_state 增量计算）
         → 回收站（目录/二进制/大文件回滚时的本体来源，按引用清理）
         → GC 三阶段（脱离分支链路且超期的快照 → 未被引用的孤儿内容 → 未被快照引用的回收站条目）
```

GC 在**每个会话本进程首次被打开时**跑一次，不做全局定时扫描。

### 调度器

```
CreateTask 创建任务图 → UpdateTask 设 blocked_by 依赖
  → RunSubagentsSequentially 启动调度器
    → JoinSet 流式调度（谁先完成就解锁谁的下游）
    → 无依赖任务自动并行
    → 5 分钟超时保护
```

### 前端事件架构

后端通过 Tauri 事件向前端推送 34 个 kebab-case 事件，`src/composables/useAgentEvents.ts` 是集中监听中枢：

```
Rust emit("chat-content")  ──→ useAgentEvents.listen() ──→ sessionStore 写入 buffer
                                                          └→ chatStore.triggerRender()

其余事件各自独立送达，不经 chat-content 分流：
  agent-step             → agentStore（运行状态）
  permission-request     → permissionStore（弹窗）
  plan-proposal(-stream) → permissionStore（方案审批）
  chat-turn-start/end    → 轮次边界与收尾
  subagent-updated       → 2s 批量节流更新
  bg-task-done           → AgentPanel 即时刷新（另留 3s 轮询兜底）
  todo-update / memory-updated / session-usage-updated / context-snapshot-updated / ...
```

部分事件由组件就地注册（如 `snapshot-created`、`config-updated`、`bg-task-done`），另有若干纯前端事件用于主窗口与监控窗口之间同步（`monitor-session-changed` / `monitor-theme-changed` / `monitor-locale-changed`）。

## 内置工具一览

共 38 个模型可调用工具。核心工具 schema 常驻，其余为按需工具（`GetToolCatalog` → `DiscoverTools` → `ExecuteTool` 三步取用）。

| 类别 | 工具 | 说明 |
|------|------|------|
| 文件读取 | ReadFile, ReadFileSkeleton | 读文件全文/骨架，支持行号范围 |
| 文件写入 | WriteFile, EditFile, ApplyPatch, DeleteFile, RenameFile | 写文件、搜索替换编辑、应用 diff、删除（含目录）、重命名 |
| 目录 | ListDirectory, SearchRepo | 列目录、生成仓库地图 |
| 搜索 | SearchText, FindFiles | 文本搜索（grep）、文件名搜索（glob） |
| 符号 | FindSymbol, ReadSymbol, FindReferences, CodeSearch | 符号定位、读定义、查引用 |
| Shell | RunCommand, StartBackgroundCommand, CheckBackgroundCommand | 命令执行、后台服务、状态检查（git 读写统一走 RunCommand） |
| 任务 | CreateTask, UpdateTask, DeleteTask, ListTasks, GetTask, SummarizeTasks | 持久化任务图 CRUD |
| 待办 | UpdateTodos | 轻量待办清单 |
| 子代理 | RunSubagent, RunSubagentsSequentially | 委派子代理、启动调度器 |
| 规划 | ProposePlan, SwitchWorkMode | 方案审批、模式切换 |
| 工具发现 | DiscoverTools, GetToolCatalog, ExecuteTool | 渐进式披露：搜索/列举/执行按需工具 |
| 会话 | CompactConversation | 手动压缩对话历史 |
| 记忆 | ReadMemory, UpdateMemory, ConsolidateMemory | 读/写/整理全局记忆文件 |
| 技能 | LoadSkill | 按名称加载技能知识 |
| Notebook | EditNotebook | Jupyter Notebook cell 编辑 |

## 安全特性

- **沙箱限制** — 会话绑定工作目录，路径遍历自动拦截
- **Shell 安全** — 递归列目录（dir /s、tree）强制排除 node_modules/.git/target/dist 等依赖目录；危险命令（递归删除、破坏性 git、DROP TABLE、系统级操作）检测并告警
- **权限审批** — Shell 等敏感操作需用户确认，系统无限期等待你的决策（无自动超时）；拒绝时可附一句说明，直接回灌给模型
- **只读保护** — 会话级开关，开启后禁止一切改动工作区的操作（写文件、非只读命令、后台服务、派子代理）；独立于权限档位与工作模式，**切模式绕不过**；只硬拒不询问，且刻意不落盘（重启归零）
- **循环检测** — Agent 循环超 30 轮暂停确认（授权后计数归零），绝对上限 200 轮；Plan 模式空转看门狗（累计 6 次无进展，无工具轮计 1 / 累计 10 个 loop 未交方案或未降级即先做一次进度小结再收敛）
- **429 限流** — 解析 Retry-After 头按服务器建议等待；连接类失败走固定阶梯重试（1s / 3s / 5s），4xx 立即返回不重试
- **快照回滚** — 所有文件操作可追溯可撤销，原子化写入防崩溃

## 致谢

- **[learn-Code-code](https://github.com/shareAI-lab/learn-claude-code)** — 架构设计的灵感来源

## 许可证

MIT License

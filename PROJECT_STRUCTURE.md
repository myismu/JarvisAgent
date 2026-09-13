# JarvisAgent 项目结构文档

本文档面向开发者与 Agent，帮助快速理解当前项目目录、核心模块边界、调用链路与改动入口。

## 1. 项目概览

JarvisAgent 是一个基于 **Tauri 2.0 + Vue 3 + Pinia + Rust/Tokio** 的桌面 AI 编程助手。

- 前端负责会话界面、执行过程展示、权限弹窗、设置面板、快照/检查点可视化。
- 后端采用三层架构：
  - `infra` 基础设施层：数据模型、LLM 客户端、数据库、配置、全局状态。
  - `core` 业务层：Agent 主循环、意图分类、工具系统、任务编排、会话、快照回滚。
  - `command` 命令层：Tauri invoke handler 胶水（所有前端可调用命令）。
- 前后端通信通过 Tauri 完成：前端使用 `invoke` 调用 Rust 命令，后端使用 `emit` 推送流式事件。

## 2. 顶层目录结构

```text
JarvisAgent/
├── src/                         # Vue 3 前端源码（主应用 + 监控页双入口）
├── src-tauri/                   # Tauri/Rust 后端源码与桌面应用配置
├── skills/                      # 内置技能目录，按技能子目录组织 SKILL.md
├── demo/                        # Agent 机制演示与参考脚本（s01~s12 + s_full.py）
├── doc/                         # 架构与设计文档
├── data/                        # 运行期数据目录，开发模式下由后端自动使用
├── dist/                        # 前端构建产物，Tauri 打包时使用
├── node_modules/                # 前端依赖
├── index.html                   # Vite 主应用入口
├── monitor.html                 # Vite 监控页入口（独立应用）
├── package.json                 # 前端依赖与脚本
├── pnpm-lock.yaml               # pnpm 锁文件
├── vite.config.ts               # Vite 配置，Tauri 开发端口固定为 1420，双入口构建
├── tsconfig*.json               # TypeScript 配置
└── README.md / PROJECT_STRUCTURE.md
```

## 3. 常用命令

```bash
pnpm tauri dev       # 启动桌面开发环境，前端热更新 + Rust 后端
pnpm build           # 前端类型检查与构建：vue-tsc --noEmit && vite build
pnpm tauri build     # 构建桌面应用
pnpm install         # 安装前端依赖
cargo test           # 在 src-tauri/ 下运行 Rust 测试
```

## 4. 前端结构：`src/`

```text
src/
├── main.ts                      # Vue 应用入口，挂载 Pinia 与 App
├── App.vue                      # 主布局容器：标题栏、侧栏、聊天区、输入区、Agent 面板、弹窗
├── monitor-main.ts / MonitorApp.vue  # 独立监控页应用（monitor.html 入口）
├── i18n.ts / locales/           # vue-i18n 国际化初始化与语言包（zh-CN / en-US）
├── assets/
│   └── global.css               # 全局样式、CSS 变量、暗色模式与基础重置
├── components/
│   ├── layout/                  # 窗口级布局组件
│   │   ├── TitleBar.vue         # 自定义标题栏
│   │   └── Sidebar.vue          # 会话侧栏与设置入口
│   ├── chat/                    # 聊天与 Agent 执行展示组件
│   │   ├── ChatArea.vue         # 聊天消息主区域
│   │   ├── TerminalInput.vue    # 用户输入区
│   │   ├── AgentPanel.vue       # Agent 执行流程侧栏
│   │   ├── AgentTurn.vue        # 单轮 Agent 执行视图
│   │   ├── ExecutionPanel.vue   # 工具调用/执行详情展示
│   │   ├── ThinkingStatus.vue   # 思考状态展示
│   │   ├── TodoPanel.vue        # 待办清单面板
│   │   ├── ToolCallGroup.vue    # 工具调用分组展示
│   │   ├── PermissionCard.vue   # 权限请求卡片
│   │   ├── ContextInspector.vue # 上下文内容检查器
│   │   ├── AgentSnapshotSection.vue # Agent 快照区块
│   │   ├── SessionTaskBoard.vue # 会话任务看板
│   │   └── WelcomeScreen.vue    # 初始欢迎页
│   ├── common/                  # 通用弹窗与确认组件
│   │   ├── PermissionModal.vue  # 工具权限确认弹窗
│   │   ├── PlanPreviewPanel.vue # 方案审批面板
│   │   ├── ConfirmModal.vue     # 通用确认弹窗
│   │   ├── RollbackConfirmModal.vue # 回滚确认弹窗
│   │   └── StreamingMarkdown.vue    # 流式 Markdown 渲染
│   ├── checkpoint/              # 检查点时间线展示
│   │   └── CheckpointTimeline.vue
│   ├── snapshot/                # 快照时间线、Diff、实时预览
│   │   ├── SnapshotTimeline.vue
│   │   ├── DiffViewer.vue
│   │   └── LivePreview.vue
│   ├── settings/                # 设置面板
│   │   └── SettingsPanel.vue
│   └── skill/                   # 技能管理
│       ├── SkillManager.vue
│       ├── SkillMarket.vue
│       ├── SkillCard.vue
│       └── SkillDetailPanel.vue
├── composables/
│   ├── useAgentEvents.ts        # Tauri 事件桥，监听后端事件并分发到 Pinia
│   ├── usePreferences.ts        # 本地偏好设置，如面板显隐与侧栏折叠
│   ├── useTheme.ts              # 主题处理
│   └── useWindow.ts             # 窗口相关能力封装
├── stores/
│   ├── session.ts               # 会话视图状态、当前会话、Token 统计、流式缓冲
│   ├── chat.ts                  # 聊天渲染与滚动控制
│   ├── agent.ts                 # Agent run、子 Agent、执行步骤、任务展示状态
│   ├── permission.ts            # 权限请求、计划文档、审批状态
│   └── appView.ts               # 视图/面板显隐偏好的全局视图状态
├── services/
│   └── snapshotService.ts       # 快照相关 Tauri invoke 封装
├── types/
│   └── index.ts                 # 前端共享类型，新增后端事件类型时应同步维护
└── utils/
    ├── agentTurnState.ts        # 单轮 Agent 状态更新工具
    ├── agentTurnRender.ts       # Agent 执行内容渲染辅助
    ├── historyRender.ts         # 历史消息渲染辅助
    ├── timeline.ts              # 时间线数据处理
    ├── toolDisplay.ts           # 工具调用分组与展示摘要
    ├── markdown.ts              # Markdown 渲染
    └── html.ts                  # HTML 处理辅助
```

### 前端关键链路

1. `src/main.ts` 创建 Vue 应用并安装 Pinia。
2. `src/App.vue` 初始化 `useAgentEvents()`，负责注册后端事件监听。
3. 用户在 `TerminalInput.vue` 输入消息后，前端通过 Tauri `invoke` 调用后端命令。
4. 后端流式推送事件，`useAgentEvents.ts` 根据事件类型更新 `session/chat/agent/permission` 等 store。
5. `ChatArea.vue` 与 `AgentPanel.vue` 根据 store 状态实时渲染消息、思考、工具调用、子 Agent 与计划审批。

## 5. 后端结构：`src-tauri/`

```text
src-tauri/
├── tauri.conf.json              # Tauri 应用配置、窗口配置、构建命令、图标
├── Cargo.toml                   # Rust crate 配置与依赖
├── model_registry.json          # 模型能力注册表（include_str! 编译时内嵌，无需运行时文件）
├── intent_rules.json            # 意图分类外部规则（10 类意图 + priority_order）
├── icons/                       # 桌面与移动平台图标资源
├── capabilities/                # Tauri 权限能力配置
├── gen/                         # Tauri 生成 schema/权限辅助文件（勿手动修改）
└── src/
    ├── main.rs                  # 二进制入口，调用 jarvisagent_lib::run()
    ├── lib.rs                   # Tauri 后端入口：数据目录探测、状态注册、插件、invoke_handler
    ├── infra/                   # 基础设施层
    ├── core/                    # 业务层（Agent / 工具 / 编排 / 回滚 / 会话 / 意图）
    └── command/                 # Tauri 命令层（所有前端可调用命令）
```

## 6. 后端基础层：`src-tauri/src/infra/`

```text
infra/
├── mod.rs                       # 基础设施模块入口
├── config/
│   ├── mod.rs
│   ├── config.rs                # 配置加载与保存（原子写入：先写 tmp 再 rename）
│   └── data_paths.rs            # data 目录、会话、图片、快照等运行期路径管理
├── db/
│   ├── mod.rs                   # SQLite 连接管理
│   └── schema.rs                # 19 张表 schema 定义 + 增量迁移
├── llm/
│   ├── mod.rs
│   ├── api_format.rs            # ApiFormat 枚举（认证头、版本头）
│   ├── api_client.rs            # HTTP 客户端、指数退避、429 Retry-After
│   ├── adapters.rs              # Anthropic ↔ OpenAI 消息格式转换
│   ├── registry.rs              # 读取 model_registry.json 的模型能力
│   └── token_count.rs           # tiktoken BPE Token 计数
├── providers/
│   ├── mod.rs
│   ├── anthropic.rs             # Anthropic Messages API 实现
│   └── openai.rs                # OpenAI Chat Completions API 实现
├── state/
│   ├── mod.rs
│   ├── state.rs                 # SessionManager、SessionContext、WorkspaceState、SnapshotRegistry
│   └── events.rs                # Tauri 事件名常量（domain:action 规范）
├── types/
│   ├── mod.rs
│   ├── models.rs                # 消息、工具、会话、计划文档等共享数据模型
│   ├── traits.rs                # LlmProvider 等核心 trait 抽象
│   ├── error.rs                 # AgentError / ApiError / DbError 等分层错误类型
│   └── constants.rs             # 全局常量
├── background.rs                # 后台任务管理 + Tauri 事件推送（bg-task-done）
└── debug_logger.rs              # 调试日志
```

## 7. Agent 主流程：`core/agent/`

```text
agent/
├── mod.rs                       # Agent 模块入口，导出 ask_jarvis / resume_jarvis
├── pipeline.rs                  # Agent 主循环流水线入口 run_pipeline() / resume_pipeline()
├── context.rs                   # 动态上下文构建：记忆、技能、目录结构等
├── stream.rs                    # SSE 流式响应解析：文本、thinking、tool_use
├── tools_runner.rs              # 执行模型返回的工具调用并组装 tool_result
├── prompts.rs                   # 系统提示词三层组装（Base + Audience + WorkMode）
├── prompts/                     # 提示词模板目录
│   ├── base_p0.md ~ base_p2.md  # 基础提示词（按 audience 拆分）
│   ├── subagent.md              # 子代理提示词
│   ├── audience/                # user.md / developer.md 交流风格
│   ├── mode/                    # chat.md / edit.md / plan.md 工作模式规则
│   └── os/                      # linux.md / macos.md / windows.md OS 规则
└── reflection/                  # 反思审查机制
    ├── mod.rs
    ├── prompt.rs                # 反思提示词
    └── strategy.rs              # 反思策略与防循环控制
```

### Agent 执行阶段

`run_pipeline()` 是后端 Agent 的主入口，整体流程可理解为：

```text
会话初始化
→ 配置与模型加载
→ 意图分类
→ 工具集选择
→ 动态上下文构建
→ LLM 流式请求
→ 解析 text/thinking/tool_use
→ 执行工具调用
→ 反思审查（reflection）
→ 写入会话与执行记录
→ 推送前端事件
```

关键约束：

- 取消令牌贯穿全流程，用户可中断运行。
- 循环次数受常量限制，避免 Agent 无限自循环。
- 工具采用渐进式披露，核心工具默认可见，延迟工具通过 `DiscoverTools` / `GetToolCatalog` / `ExecuteTool` 激活。
- 计划类输出可被转成方案审批文档，并在前端右侧面板展示。

## 8. Tauri 命令层：`src-tauri/src/command/`

```text
command/
├── mod.rs                       # 命令模块入口
├── session.rs                   # 会话 CRUD、工作目录、撤回、后台任务、子 Agent、计划文档查询
├── config.rs                    # 配置读取与保存
├── app_config.rs                # 窗口状态、UI 偏好、技能激活状态
├── permission.rs                # 权限确认、取消当前 Agent
├── history.rs / history_types.rs # 会话历史渲染与类型
├── checkpoint.rs                # 检查点、分支、回滚
├── snapshot.rs                  # 快照创建、查询、分支、回滚
├── sandbox.rs                   # 多 Agent 沙盒创建、完成、放弃、发布、比较
├── merge.rs                     # 沙盒/分支合并预览、执行与冲突查询
└── skill.rs                     # 技能列表与详情查询
```

所有前端可调用命令必须在 `src-tauri/src/lib.rs` 的 `invoke_handler` 中注册。
新增命令时通常需要同时修改：

1. `command/<domain>.rs`：实现 `#[tauri::command]` 函数。
2. `command/mod.rs`：声明新模块。
3. `src-tauri/src/lib.rs`：加入 `tauri::generate_handler!`。
4. 前端调用处：使用 `invoke("command_name", payload)`。

## 9. LLM 抽象层：`infra/llm/` 与 `infra/providers/`

```text
infra/llm/
├── mod.rs                       # LLM 模块入口
├── api_format.rs                # API 协议格式枚举与通用 header/version 逻辑
├── api_client.rs                # HTTP 客户端、重试、流式请求
├── adapters.rs                  # Anthropic/OpenAI 消息格式转换
├── registry.rs                  # 读取 model_registry.json 的模型能力
└── token_count.rs               # tiktoken BPE Token 计数

infra/providers/
├── mod.rs                       # Provider 模块入口
├── anthropic.rs                 # Anthropic API 格式实现
└── openai.rs                    # OpenAI 兼容 API 格式实现
```

开发约定：

- 新增 API 格式能力优先扩展 `LlmProvider` trait 与 provider 实现。
- 不要在业务代码中散落字符串格式判断。
- 模型是否支持 thinking、vision、上下文长度、单轮最大 token 等能力应从 `model_registry.json` 查询。

## 10. 工具系统：`core/tools/`

```text
tools/
├── mod.rs                       # 工具系统中枢：意图过滤、路由分发、技能加载、写工具拦截
├── file_tools/                  # 文件读写、编辑、搜索、符号（14 个文件）
│   ├── mod.rs                   # 工具注册入口
│   ├── read.rs / write.rs / edit.rs / patch.rs / delete.rs / rename.rs
│   ├── directory.rs / search.rs / symbol.rs / diff.rs / common.rs / workspace.rs / registry.rs
├── shell_tools/                 # Shell 执行（10 个文件）
│   ├── mod.rs                   # 工具注册入口
│   ├── execution.rs / background.rs / readonly.rs / git.rs
│   ├── security.rs / guards.rs / regexes.rs / types.rs / utils.rs
├── agent_tools/                 # Agent 专用工具
│   ├── mod.rs                   # 工具注册入口（LoadSkill / GetToolCatalog）
│   ├── subagent.rs              # 子代理执行引擎（RunSubagent / RunSubagentsSequentially）
│   ├── skill.rs                 # 技能加载（LoadSkill）
│   ├── compact.rs               # 上下文压缩 + 记忆整理（CompactConversation / ConsolidateMemory）
│   ├── plan.rs                  # 方案审批（ProposePlan）
│   └── switch_mode.rs           # 模式切换（SwitchWorkMode）
├── task_tools/                  # 任务与待办
│   ├── mod.rs
│   ├── registry.rs / todo_write.rs
│   └── persistent/              # 持久化任务（CreateTask / UpdateTask / DeleteTask / …）
├── search_tools/                # Glob + Grep 搜索（FindFiles / SearchText）
│   └── mod.rs
├── notebook_tools/              # Jupyter Notebook 编辑
│   ├── mod.rs
│   └── notebook_guard.rs
├── system_tools/                # 工作区设置（SetWorkspace；OS/CWD/Home 已由提示词自动注入）
│   └── mod.rs
└── framework/                   # 工具框架层
    ├── mod.rs                   # ToolCallResult、路由定义等
    ├── registry.rs              # 全局工具注册表（define_tools! / tool_def! 宏）
    ├── tool_search.rs           # 渐进式工具披露（DiscoverTools / ExecuteTool）
    ├── permission.rs            # 路径权限与用户授权请求
    ├── agent_registry.rs        # 子 Agent 类型注册
    └── tool_call_logger.rs      # 工具调用审计日志
```

### 工具选择规则

- `CHAT` 意图：只暴露只读工具子集。
- `MEMORY_QUERY` 意图：只暴露记忆查询相关的轻量工具。
- `PROJECT_ACTION` 意图：暴露核心工具，并允许通过 `DiscoverTools` / `ExecuteTool` 激活延迟工具。
- `SUBAGENT` 意图：限制部分工具，避免子 Agent 递归调度或执行不合适能力。
- 写工具（WriteFile、EditFile、RunCommand 等）默认注册为延迟工具，聊天意图下禁止调用。

### 技能加载

`load_all_skills()` 会递归扫描运行期 `skills/` 目录下的 `SKILL.md`，解析 YAML frontmatter 中的 `name` 和 `description`，再把正文作为技能内容注入 Agent 上下文。技能激活状态通过 `command::app_config` 持久化。

## 11. 编排系统：`core/orchestration/`

```text
orchestration/
├── mod.rs                       # 编排模块入口
├── agent_runs.rs                # 主 Agent 执行记录与事件
├── agent_run_repository.rs      # Agent 运行记录 SQLite 仓储
├── subagents.rs                 # 子 Agent 运行状态与监控
├── tasks.rs                     # 任务 CRUD 与依赖管理
├── scheduler.rs                 # 基于依赖图的任务调度（JoinSet 流式调度）
└── multi_agent/                 # 多 Agent 协作
    ├── mod.rs
    ├── sandbox.rs               # 沙盒隔离
    └── merge.rs                 # 沙盒合并（LCA 三方合并 + 冲突解决）
```

该模块负责记录主 Agent/子 Agent 的运行轨迹，支持任务拆分、依赖关系、并行调度和前端执行面板展示。

## 12. 会话、记忆与数据目录：`core/session/`

```text
session/
├── mod.rs                       # 会话 CRUD、历史消息、图片、计划文档、Token 统计
├── repository.rs                # 会话 SQLite 仓储 + 消息加载
├── resource_repository.rs       # 附件/资源 SQLite 仓储
└── memory.rs                    # 会话记忆与上下文压缩（视图引用方案）
```

运行期数据默认写入 `data/`：

```text
data/
├── app-config.json              # 应用配置
├── jarvis.sqlite3               # SQLite 主数据库（会话/消息/快照/任务/运行记录）
├── global/                      # 全局记忆文件等
├── cache/                       # 缓存数据
├── logs/                        # 日志输出
├── tmp/                         # 临时文件
└── （.sessions / .tasks / .checkpoints / .snapshots 等目录随需创建）
```

开发模式下，后端会自动检测数据目录：

- `pnpm tauri dev`：使用项目根目录下的 `data/`。
- `cargo run` 且当前目录为 `src-tauri/`：回到项目根目录使用 `data/`。
- 打包后：使用可执行文件所在目录下的 `data/`。

## 13. 快照、检查点与多 Agent 沙盒：`core/rollback/` + `core/orchestration/multi_agent/`

```text
rollback/
├── mod.rs                       # 回滚引擎入口
├── snapshot.rs                  # 文件级快照数据结构与快照树
├── journal.rs                   # 操作日志
├── patch.rs                     # 文本差异与 patch（Create/Update/Delete/Rename）
├── replay.rs                    # 快照回放 + 原子文件回滚（staging + rename）
├── gc.rs                        # 三阶段垃圾回收（脱离分支 → 孤儿快照 → 孤儿内容）
├── session_manager.rs           # 会话级快照注册与分支管理
├── store.rs                     # 快照 SQLite 持久化
└── rollback_logger.rs           # 回滚日志

orchestration/multi_agent/       # 多 Agent 协作（见第 11 节）
├── sandbox.rs                   # 沙盒隔离
└── merge.rs                     # LCA 三方合并 + 冲突解决
```

相关前端展示位于：

- `src/components/snapshot/`
- `src/components/checkpoint/`
- `src/services/snapshotService.ts`

该系统用于记录文件变更、支持回滚、沙盒隔离、多 Agent 分支工作与合并预览。

## 14. 后端启动与命令注册链路

```text
src-tauri/src/main.rs
→ jarvisagent_lib::run()
→ src-tauri/src/lib.rs::run()
→ 探测并锁定 data 目录（AGENT_HOME_DIR）
→ infra::db::init() 初始化 SQLite（19 张表 + 增量迁移）
→ 恢复工作目录
→ 恢复或创建启动会话
→ tauri::Builder::default()
→ manage(...) 注册全局状态
→ plugin(...) 注册 Tauri 插件
→ invoke_handler(...) 注册前端命令（~90 个）
→ run(tauri::generate_context!())
```

`lib.rs` 中注册的核心状态包括：

- `SessionManager`（`infra::state::state`）：活跃会话生命周期、取消令牌、权限请求等。
- `BackgroundState` / `CompactingState`（`infra::background`）：后台任务与压缩状态。
- `SubAgentMonitorState`（`core::orchestration::subagents`）：子 Agent 运行状态。
- `ConfigState` / `RuntimeConfigState`（`infra::config::config`）：应用配置。
- `WorkspaceState`（`infra::state::state`）：当前工作目录。
- `SnapshotRegistry`（`infra::state::state`）：会话级快照管理器注册表。

## 15. 前后端事件与状态同步

```text
Rust command / Agent pipeline
→ app.emit(...)
→ src/composables/useAgentEvents.ts
→ Pinia stores
→ Vue components
```

`useAgentEvents.ts` 是前端事件桥的核心：

- 监听后端流式文本、thinking、工具调用、权限请求、计划文档、子 Agent、Agent run 等事件。
- 将事件归一化后写入 `session/chat/agent/permission` store。
- 负责 HMR 场景下清理旧监听器，避免重复注册。

新增事件时通常需要同步修改：

1. Rust 端事件 payload。
2. `src/types/index.ts` 中的类型定义。
3. `src/composables/useAgentEvents.ts` 中的监听与分发逻辑。
4. 对应 store 与组件展示。

## 16. Agent 改动入口速查

| 目标 | 优先查看/修改位置 |
| --- | --- |
| 修改 Agent 主循环 | `src-tauri/src/core/agent/pipeline.rs` |
| 修改上下文注入 | `src-tauri/src/core/agent/context.rs` |
| 修改提示词组装 | `src-tauri/src/core/agent/prompts.rs` |
| 修改 SSE 解析 | `src-tauri/src/core/agent/stream.rs` |
| 修改工具执行结果处理 | `src-tauri/src/core/agent/tools_runner.rs` |
| 新增工具 | `src-tauri/src/core/tools/` 对应子模块 + `framework/registry.rs` 注册宏 |
| 修改权限逻辑 | `src-tauri/src/core/tools/framework/permission.rs` 与 `command/permission.rs` |
| 新增 Tauri 命令 | `src-tauri/src/command/` + `src-tauri/src/lib.rs` |
| 新增模型能力 | `src-tauri/model_registry.json` + `infra/llm/registry.rs` |
| 新增 Provider/API 格式 | `infra/types/traits.rs`、`infra/llm/api_format.rs`、`infra/providers/` |
| 修改会话持久化 | `src-tauri/src/core/session/mod.rs` |
| 修改前端事件处理 | `src/composables/useAgentEvents.ts` |
| 修改会话 UI 状态 | `src/stores/session.ts`、`src/stores/chat.ts` |
| 修改执行面板 | `src/stores/agent.ts`、`src/components/chat/AgentPanel.vue` |
| 修改权限/计划审批 UI | `src/stores/permission.ts`、`src/components/common/` |
| 修改快照 UI | `src/components/snapshot/`、`src/services/snapshotService.ts` |
| 修改技能管理 | `src/components/skill/`、`src-tauri/src/command/skill.rs` |

## 17. 开发约定与注意事项

- 不得删除源码中已有中文注释。
- 新增错误类型优先使用 `thiserror`，避免裸字符串错误。
- 新增前端事件类型时，应先在 `src/types/index.ts` 定义，再在 `useAgentEvents.ts` 处理。
- 新增后端命令时，必须注册到 `src-tauri/src/lib.rs` 的 `invoke_handler`。
- 新增 API 格式逻辑时，优先扩展 `LlmProvider` 抽象，不要在业务逻辑中添加零散格式判断。
- 前端状态应优先进入 Pinia store，组件只负责展示与轻量交互。
- 涉及文件写入、Shell 执行、回滚、合并等能力时，应经过权限与安全检查。
- 工具定义统一通过 `define_tools!` / `tool_def!` 宏注册到 `framework/registry.rs`，保持工具集稳定以利于 prompt cache。
- `src-tauri/target/`、`node_modules/`、`dist/` 为生成物或依赖目录，Agent 通常不应主动修改。

## 18. 推荐阅读顺序

### 新开发者

1. `src/App.vue`：理解界面骨架。
2. `src/composables/useAgentEvents.ts`：理解前端如何接收后端事件。
3. `src/stores/session.ts`、`src/stores/agent.ts`、`src/stores/permission.ts`：理解核心状态。
4. `src-tauri/src/lib.rs`：理解后端启动和命令注册。
5. `src-tauri/src/core/agent/pipeline.rs`：理解 Agent 主循环。
6. `src-tauri/src/core/tools/mod.rs`：理解工具系统。

### Agent 执行任务前

1. 先确认任务属于前端、后端、工具系统、LLM 适配、会话持久化还是快照系统。
2. 根据“Agent 改动入口速查”定位模块。
3. 如果修改前后端通信，检查 `invoke_handler`、前端 `invoke`、事件 payload、`src/types/index.ts` 是否一致。
4. 如果修改工具或 Shell 能力，检查权限、安全策略与子 Agent 限制。
5. 如果修改 UI，启动开发环境并实际验证主路径和边界状态。
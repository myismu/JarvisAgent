# JarvisAgent 项目结构文档

本文档面向开发者与 Agent，帮助快速理解当前项目目录、核心模块边界、调用链路与改动入口。

## 1. 项目概览

JarvisAgent 是一个基于 **Tauri 2.0 + Vue 3 + Pinia + Rust/Tokio** 的桌面 AI 编程助手。

- 前端负责会话界面、执行过程展示、权限弹窗、设置面板、快照/检查点可视化。
- 后端负责 Agent 主循环、LLM API 适配、工具调用、权限控制、会话持久化、快照/检查点、子 Agent 编排。
- 前后端通信通过 Tauri 完成：前端使用 `invoke` 调用 Rust 命令，后端使用 `emit` 推送流式事件。

## 2. 顶层目录结构

```text
JarvisAgent/
├── src/                         # Vue 3 前端源码
├── src-tauri/                   # Tauri/Rust 后端源码与桌面应用配置
├── skills/                      # 内置技能目录，按技能子目录组织 SKILL.md
├── demo/                        # Agent/Claude Code 机制演示与参考代码
├── data/                        # 运行期数据目录，开发模式下由后端自动使用
├── dist/                        # 前端构建产物，Tauri 打包时使用
├── node_modules/                # 前端依赖
├── package.json                 # 前端依赖与脚本
├── pnpm-lock.yaml               # pnpm 锁文件
├── vite.config.ts               # Vite 配置，Tauri 开发端口固定为 1420
├── tsconfig*.json               # TypeScript 配置
└── CLAUDE.md                    # 项目级 Agent 协作说明
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
├── assets/
│   └── global.css               # 全局样式、CSS 变量、暗色模式与基础重置
├── components/
│   ├── layout/                  # 窗口级布局组件
│   │   ├── TitleBar.vue         # 自定义标题栏
│   │   └── Sidebar.vue          # 会话侧栏与设置入口
│   ├── chat/                    # 聊天与 Agent 执行展示组件
│   │   ├── ChatArea.vue         # 聊天消息主区域
│   │   ├── TerminalInput.vue    # 用户输入区
│   │   ├── MessageBubble.vue    # 单条消息气泡
│   │   ├── AgentPanel.vue       # Agent 执行流程侧栏
│   │   ├── AgentTurn.vue        # 单轮 Agent 执行视图
│   │   ├── ExecutionPanel.vue   # 工具调用/执行详情展示
│   │   ├── ThinkingStatus.vue   # 思考状态展示
│   │   └── WelcomeScreen.vue    # 初始欢迎页
│   ├── common/                  # 通用弹窗与确认组件
│   │   ├── PermissionModal.vue  # 工具权限确认弹窗
│   │   ├── PlanPreviewPanel.vue # 方案审批面板
│   │   ├── ConfirmModal.vue     # 通用确认弹窗
│   │   └── RollbackConfirmModal.vue # 回滚确认弹窗
│   ├── checkpoint/              # 检查点时间线展示
│   ├── snapshot/                # 快照时间线、Diff、实时预览
│   └── settings/                # 设置面板
├── composables/
│   ├── useAgentEvents.ts        # Tauri 事件桥，监听后端事件并分发到 Pinia
│   ├── usePreferences.ts        # 本地偏好设置，如面板显隐与侧栏折叠
│   ├── useTheme.ts              # 主题处理
│   └── useWindow.ts             # 窗口相关能力封装
├── stores/
│   ├── session.ts               # 会话视图状态、当前会话、Token 统计、流式缓冲
│   ├── chat.ts                  # 聊天渲染与滚动控制
│   ├── agent.ts                 # Agent run、子 Agent、执行步骤、任务展示状态
│   └── permission.ts            # 权限请求、计划文档、审批状态
├── services/
│   └── snapshotService.ts       # 快照相关 Tauri invoke 封装
├── types/
│   └── index.ts                 # 前端共享类型，新增后端事件类型时应同步维护
└── utils/
    ├── agentTurnState.ts        # 单轮 Agent 状态更新工具
    ├── agentTurnRender.ts       # Agent 执行内容渲染辅助
    ├── historyRender.ts         # 历史消息渲染辅助
    ├── markdown.ts              # Markdown 渲染
    ├── timeline.ts              # 时间线数据处理
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
├── model_registry.json          # 模型能力注册表，定义上下文、thinking、视觉等能力
├── icons/                       # 桌面与移动平台图标资源
├── capabilities/                # Tauri 权限能力配置
└── src/
    ├── main.rs                  # 二进制入口，调用 jarvisagent_lib::run()
    ├── lib.rs                   # Tauri 后端入口，初始化状态、插件、invoke handler
    └── core/                    # 后端核心业务模块
```

## 6. 后端核心模块：`src-tauri/src/core/`

```text
core/
├── mod.rs                       # 核心模块入口，声明并重导出主要模块与命令
├── config.rs                    # 应用配置加载、保存与 AgentConfig
├── constants.rs                 # 全局常量
├── data_paths.rs                # data 目录、会话、图片、快照等运行期路径管理
├── error.rs                     # AgentError / ApiError / ToolError / MemoryError 等错误类型
├── models.rs                    # 消息、工具、会话、计划文档等共享数据模型
├── state.rs                     # SessionManager、SessionContext、WorkspaceState、SnapshotRegistry
├── traits.rs                    # LlmProvider 等核心 trait 抽象
├── agent/                       # Agent 主流程
├── commands/                    # 前端可调用的 Tauri 命令
├── infra/                       # 基础设施能力：提示词、后台任务、调试日志
├── intent/                      # 用户意图分类
├── llm/                         # LLM API 协议、客户端、适配器、模型注册表
├── providers/                   # Anthropic/OpenAI 等 Provider 实现
├── orchestration/               # 主 Agent run、子 Agent、任务与调度
├── session/                     # 会话持久化、记忆、检查点关联
├── snapshot_engine/             # 文件快照引擎、回放、GC、patch、多 Agent 沙盒
├── snapshot_manager/            # 会话级快照管理器与存储
└── tools/                       # 工具定义、渐进式披露、权限、执行路由
```

## 7. Agent 主流程：`core/agent/`

```text
agent/
├── mod.rs                       # Agent 模块入口，导出 ask_jarvis
├── pipeline.rs                  # Agent 主循环流水线入口 run_pipeline()
├── context.rs                   # 动态上下文构建：记忆、技能、目录结构等
├── stream.rs                    # SSE 流式响应解析：文本、thinking、tool_use
└── tools_runner.rs              # 执行模型返回的工具调用并组装 tool_result
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
→ 写入会话与执行记录
→ 推送前端事件
```

关键约束：

- 取消令牌贯穿全流程，用户可中断运行。
- 循环次数受常量限制，避免 Agent 无限自循环。
- 工具采用渐进式披露，核心工具默认可见，延迟工具通过 `search_tools` 激活。
- 计划类输出可被转成方案审批文档，并在前端右侧面板展示。

## 8. Tauri 命令层：`core/commands/`

```text
commands/
├── mod.rs                       # 命令模块入口
├── config.rs                    # 配置读取与保存
├── session.rs                   # 会话 CRUD、工作目录、Agent run、子 Agent、计划文档查询
├── permission.rs                # 权限确认、取消当前 Agent
├── history.rs                   # 会话历史渲染
├── checkpoint.rs                # 检查点、分支、回滚、提交 pending 操作
├── snapshot.rs                  # 快照创建、查询、分支、回滚
├── sandbox.rs                   # 多 Agent 沙盒创建、完成、放弃、发布、比较
└── merge.rs                     # 沙盒/分支合并预览、执行与冲突查询
```

所有前端可调用命令必须在 `src-tauri/src/lib.rs` 的 `invoke_handler` 中注册。
新增命令时通常需要同时修改：

1. `core/commands/<domain>.rs`：实现 `#[tauri::command]` 函数。
2. `core/commands/mod.rs`：声明新模块。
3. `core/mod.rs`：按需重导出。
4. `src-tauri/src/lib.rs`：加入 `tauri::generate_handler!`。
5. 前端调用处：使用 `invoke("command_name", payload)`。

## 9. LLM 抽象层：`core/llm/` 与 `core/providers/`

```text
llm/
├── mod.rs                       # LLM 模块入口
├── api_format.rs                # API 协议格式枚举与通用 header/version 逻辑
├── api_client.rs                # HTTP 客户端、重试、流式请求
├── adapters.rs                  # Anthropic/OpenAI 消息格式转换
└── registry.rs                  # 读取 model_registry.json 的模型能力

providers/
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
├── mod.rs                       # 工具系统入口：定义组装、技能加载、工具调用路由
├── registry.rs                  # 工具注册表
├── tool_search.rs               # 渐进式工具披露与 search_tools
├── file_tools.rs                # 文件读写、编辑、搜索等工具
├── shell_tools.rs               # Bash/PowerShell 等命令执行工具
├── shell_security.rs            # Shell 安全检查与危险命令识别
├── permission.rs                # 路径权限与用户授权请求
├── system_tools.rs              # 系统级工具，如 compact/dream 等
├── task_tools.rs                # 任务/Todo 工具
├── agent_tools.rs               # 子 Agent 调用工具
├── agent_registry.rs            # 子 Agent 类型注册
├── claude_code_tools.rs         # Claude Code 风格工具兼容/封装
└── notebook_tools.rs            # Notebook 编辑工具
```

### 工具选择规则

- `CHAT` 意图：不暴露工具。
- `MEMORY_QUERY` 意图：只暴露记忆查询相关的轻量工具。
- `PROJECT_ACTION` 意图：暴露核心工具，并允许通过 `search_tools` 激活延迟工具。
- `SUBAGENT` 意图：限制部分工具，避免子 Agent 递归调度或执行不合适能力。

### 技能加载

`load_all_skills()` 会递归扫描运行期 `skills/` 目录下的 `SKILL.md`，解析 YAML frontmatter 中的 `name` 和 `description`，再把正文作为技能内容注入 Agent 上下文。

## 11. 编排系统：`core/orchestration/`

```text
orchestration/
├── mod.rs                       # 编排模块入口
├── agent_runs.rs                # 主 Agent 执行记录与事件
├── subagents.rs                 # 子 Agent 运行状态与监控
├── tasks.rs                     # 任务 CRUD 与依赖管理
└── scheduler.rs                 # 基于依赖图的任务调度
```

该模块负责记录主 Agent/子 Agent 的运行轨迹，支持任务拆分、依赖关系、并行调度和前端执行面板展示。

## 12. 会话、记忆与数据目录

```text
session/
├── mod.rs                       # 会话 CRUD、历史消息、图片、计划文档、Token 统计
├── memory.rs                    # 会话记忆与上下文压缩相关逻辑
└── checkpoint.rs                # 会话级检查点关联逻辑
```

运行期数据默认写入 `data/`：

```text
data/
├── config.json                  # 应用配置
├── .sessions/                   # 会话 JSON、图片、计划文档等
├── .tasks/                      # 任务数据
├── .checkpoints/                # 检查点数据
├── .snapshots/                  # 快照数据
├── skills/                      # 运行期技能目录
└── global_memory.md             # 全局记忆文件
```

开发模式下，后端会自动检测数据目录：

- `pnpm tauri dev`：使用项目根目录下的 `data/`。
- `cargo run` 且当前目录为 `src-tauri/`：回到项目根目录使用 `data/`。
- 打包后：使用可执行文件所在目录下的 `data/`。

## 13. 快照、检查点与多 Agent 沙盒

```text
snapshot_engine/
├── mod.rs                       # 快照引擎入口
├── snapshot.rs                  # 文件级快照数据结构与操作
├── journal.rs                   # 操作日志
├── patch.rs                     # 文本差异与 patch
├── replay.rs                    # 快照回放
├── gc.rs                        # 快照垃圾回收
└── multi_agent/
    ├── mod.rs                   # 多 Agent 快照子系统入口
    ├── sandbox.rs               # 沙盒隔离
    └── merge.rs                 # 沙盒合并

snapshot_manager/
├── mod.rs                       # 快照管理器入口
├── session_manager.rs           # 会话级快照注册与管理
└── store.rs                     # 快照持久化存储
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
→ 初始化 data 目录
→ 恢复工作目录
→ 恢复或创建启动会话
→ tauri::Builder::default()
→ manage(...) 注册全局状态
→ plugin(...) 注册 Tauri 插件
→ invoke_handler(...) 注册前端命令
→ run(tauri::generate_context!())
```

`lib.rs` 中注册的核心状态包括：

- `SessionManager`：活跃会话生命周期、取消令牌、权限请求等。
- `BackgroundState`：后台任务状态。
- `SubAgentMonitorState`：子 Agent 运行状态。
- `ConfigState`：应用配置。
- `WorkspaceState`：当前工作目录。
- `SnapshotRegistry`：会话级快照管理器。

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
| 修改 SSE 解析 | `src-tauri/src/core/agent/stream.rs` |
| 修改工具执行结果处理 | `src-tauri/src/core/agent/tools_runner.rs` |
| 新增工具 | `src-tauri/src/core/tools/`，必要时更新 `tools/mod.rs` 路由 |
| 修改权限逻辑 | `src-tauri/src/core/tools/permission.rs` 与 `commands/permission.rs` |
| 新增 Tauri 命令 | `src-tauri/src/core/commands/` + `src-tauri/src/lib.rs` |
| 新增模型能力 | `src-tauri/model_registry.json` + `core/llm/registry.rs` |
| 新增 Provider/API 格式 | `core/traits.rs`、`core/llm/api_format.rs`、`core/providers/` |
| 修改会话持久化 | `src-tauri/src/core/session/mod.rs` |
| 修改前端事件处理 | `src/composables/useAgentEvents.ts` |
| 修改会话 UI 状态 | `src/stores/session.ts`、`src/stores/chat.ts` |
| 修改执行面板 | `src/stores/agent.ts`、`src/components/chat/AgentPanel.vue` |
| 修改权限/计划审批 UI | `src/stores/permission.ts`、`src/components/common/` |
| 修改快照 UI | `src/components/snapshot/`、`src/services/snapshotService.ts` |

## 17. 开发约定与注意事项

- 不得删除源码中已有中文注释。
- 新增错误类型优先使用 `thiserror`，避免裸字符串错误。
- 新增前端事件类型时，应先在 `src/types/index.ts` 定义，再在 `useAgentEvents.ts` 处理。
- 新增后端命令时，必须注册到 `src-tauri/src/lib.rs` 的 `invoke_handler`。
- 新增 API 格式逻辑时，优先扩展 `LlmProvider` 抽象，不要在业务逻辑中添加零散格式判断。
- 前端状态应优先进入 Pinia store，组件只负责展示与轻量交互。
- 涉及文件写入、Shell 执行、回滚、合并等能力时，应经过权限与安全检查。
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

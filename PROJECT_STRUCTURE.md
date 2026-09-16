# JarvisAgent 项目结构文档

本文档是**跨层总纲**：讲清项目长什么样、前后端怎么连、要改的东西在哪个文件。

各层的模块清单与职责**不在这里重复**，见两份分层文档：

| 分层文档 | 覆盖范围 |
|---|---|
| [`src/PROJECT_STRUCTURE.md`](src/PROJECT_STRUCTURE.md) | 前端全部：组件 / composables / stores / 类型 / utils，以及前端核心数据流 |
| [`src-tauri/PROJECT_STRUCTURE.md`](src-tauri/PROJECT_STRUCTURE.md) | 后端全部：三层架构 / Agent 管线 / 工具系统 / 编排 / 回滚，以及后端快速定位表 |

> **维护约定**：新增分层细节写进对应分层文档，不要往本文件回抄。
> 此前本文件把两层的结构又浓缩抄了一遍（§4 ≈ 前端文档、§6-13 ≈ 后端文档），
> 结果改一处要同步三处，最后三份一起过期——意图分类被删掉后，三份文档都还在讲它。

## 1. 项目概览

JarvisAgent 是一个基于 **Tauri 2.0 + Vue 3 + Pinia + Rust/Tokio** 的桌面 AI 编程助手。

- 前端负责会话界面、执行过程展示、权限弹窗、设置面板、快照/检查点可视化。
- 后端采用三层架构：
  - `infra` 基础设施层：数据模型、LLM 客户端、数据库、配置、全局状态。
  - `core` 业务层：Agent 主循环、复杂任务检测、工具系统、任务编排、会话、快照回滚。
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

## 4. 后端结构总览：`src-tauri/`

```text
src-tauri/
├── tauri.conf.json              # Tauri 应用配置、窗口配置、构建命令、图标
├── Cargo.toml                   # Rust crate 配置与依赖
├── model_registry.json          # 模型能力注册表（include_str! 编译时内嵌，无需运行时文件）
├── icons/                       # 桌面与移动平台图标资源
├── capabilities/                # Tauri 权限能力配置
├── gen/                         # Tauri 生成 schema/权限辅助文件（勿手动修改）
└── src/
    ├── main.rs                  # 二进制入口，调用 jarvisagent_lib::run()
    ├── lib.rs                   # Tauri 后端入口：数据目录探测、状态注册、插件、invoke_handler
    ├── infra/                   # 基础设施层
    ├── core/                    # 业务层（Agent / 工具 / 编排 / 回滚 / 会话）
    └── command/                 # Tauri 命令层（所有前端可调用命令）
```

模块级清单见 [`src-tauri/PROJECT_STRUCTURE.md`](src-tauri/PROJECT_STRUCTURE.md)。

## 5. 后端启动与命令注册链路

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

## 6. 前后端事件与状态同步

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

## 7. Agent 改动入口速查

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

## 8. 开发约定与注意事项

- 不得删除源码中已有中文注释。
- 新增错误类型优先使用 `thiserror`，避免裸字符串错误。
- 新增前端事件类型时，应先在 `src/types/index.ts` 定义，再在 `useAgentEvents.ts` 处理。
- 新增后端命令时，必须注册到 `src-tauri/src/lib.rs` 的 `invoke_handler`。
- 新增 API 格式逻辑时，优先扩展 `LlmProvider` 抽象，不要在业务逻辑中添加零散格式判断。
- 前端状态应优先进入 Pinia store，组件只负责展示与轻量交互。
- 涉及文件写入、Shell 执行、回滚、合并等能力时，应经过权限与安全检查。
- 工具定义统一通过 `define_tools!` / `tool_def!` 宏注册到 `framework/registry.rs`，保持工具集稳定以利于 prompt cache。
- `src-tauri/target/`、`node_modules/`、`dist/` 为生成物或依赖目录，Agent 通常不应主动修改。

## 9. 推荐阅读顺序

### 新开发者

1. `src/App.vue`：理解界面骨架。
2. `src/composables/useAgentEvents.ts`：理解前端如何接收后端事件。
3. `src/stores/session.ts`、`src/stores/agent.ts`、`src/stores/permission.ts`：理解核心状态。
4. `src-tauri/src/lib.rs`：理解后端启动和命令注册。
5. `src-tauri/src/core/agent/pipeline.rs`：理解 Agent 主循环。
6. `src-tauri/src/core/tools/mod.rs`：理解工具系统。

### Agent 执行任务前

1. 先确认任务属于前端、后端、工具系统、LLM 适配、会话持久化还是快照系统。
2. 根据「Agent 改动入口速查」定位模块。
3. 如果修改前后端通信，检查 `invoke_handler`、前端 `invoke`、事件 payload、`src/types/index.ts` 是否一致。
4. 如果修改工具或 Shell 能力，检查权限、安全策略与子 Agent 限制。
5. 如果修改 UI，启动开发环境并实际验证主路径和边界状态。

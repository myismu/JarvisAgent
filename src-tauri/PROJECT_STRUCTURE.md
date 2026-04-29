# src-tauri 项目结构说明

本文档只总结 `E:\demo\JarvisAgent\src-tauri` Rust/Tauri 后端结构，面向开发者与 Agent 快速定位后端入口、核心模块和修改位置。

## 目录定位

`src-tauri` 是 JarvisAgent 的桌面端后端工程，负责：

- 初始化 Tauri 应用、插件、全局状态与前端可调用命令。
- 执行 Agent 主循环、LLM 流式请求、工具调用、权限审批与事件回传。
- 管理会话、配置、任务、子代理、快照、检查点、沙盒与合并。
- 提供跨平台桌面应用打包配置、权限能力声明与图标资源。

## 顶层结构

```text
src-tauri/
├── Cargo.toml                   # Rust crate 配置、依赖声明、lib crate 类型
├── Cargo.lock                   # Rust 依赖锁文件
├── build.rs                     # Tauri 构建脚本
├── tauri.conf.json              # Tauri 应用配置：窗口、打包、bundle、权限等
├── .taurignore                  # Tauri 打包忽略规则
├── .gitignore                   # src-tauri 内部 Git 忽略规则
├── model_registry.json          # 模型能力注册表：上下文、最大 token、thinking、vision 等
├── PROJECT_STRUCTURE.md         # src-tauri 后端结构导航文档
├── capabilities/
│   └── default.json             # Tauri 2 capability 权限声明
├── gen/                         # Tauri 生成的 schema/权限辅助文件
├── icons/                       # 桌面端、Windows、macOS、iOS、Android 图标资源
├── target/                      # Cargo 构建产物，开发时可忽略
└── src/                         # Rust 后端源码
```

## 源码入口：`src/`

```text
src/
├── main.rs                      # 二进制入口，启动 jarvisagent_lib::run()
├── lib.rs                       # Tauri 后端入口：数据目录、状态、插件、invoke handler
└── core/                        # 后端核心业务模块
```

### `src/lib.rs`

`lib.rs` 是 Tauri 后端的真实启动入口，主要职责：

1. 检测并锁定运行时 `data/` 目录。
2. 迁移旧工作区布局并恢复上次工作目录。
3. 恢复或创建启动会话。
4. 注册全局状态：`SessionManager`、`BackgroundState`、`SubAgentMonitorState`、`ConfigState`、`WorkspaceState`、`SnapshotRegistry`。
5. 注册 Tauri 插件：opener、dialog、fs、window-state。
6. 在 `invoke_handler` 中注册前端可调用命令。

## 核心模块总览：`src/core/`

```text
core/
├── mod.rs                       # core 统一出口：声明子模块并重导出常用命令/状态
├── config.rs                    # 配置加载、保存、Profile 与图片压缩配置
├── constants.rs                 # 文件名、事件名、默认值等常量
├── data_paths.rs                # data/ 运行时路径、会话路径、旧布局迁移
├── error.rs                     # AgentError、ApiError、ToolError、MemoryError 等错误类型
├── models.rs                    # 后端共享数据结构与 Tauri 返回类型
├── state.rs                     # SessionManager、SessionContext、WorkspaceState、SnapshotRegistry
├── traits.rs                    # LlmProvider 等跨模块 trait 抽象
├── agent/                       # Agent 主管线
├── commands/                    # Tauri invoke 命令实现
├── infra/                       # 基础设施：后台任务、日志、提示词
├── intent/                      # 用户意图分类
├── llm/                         # LLM 通用层：HTTP、格式、适配、模型注册表
├── providers/                   # Anthropic/OpenAI Provider 实现
├── tools/                       # Agent 工具系统
├── orchestration/               # Agent run、子代理、任务、调度编排
├── session/                     # 会话元数据、历史、记忆、检查点关联
├── snapshot_engine/             # 文件级快照引擎
└── snapshot_manager/            # 会话级快照管理器
```

## Agent 管线：`core/agent/`

```text
agent/
├── mod.rs                       # ask_jarvis Tauri 命令入口
├── pipeline.rs                  # run_pipeline 主流程
├── context.rs                   # 动态上下文注入：记忆、技能、目录结构等
├── stream.rs                    # SSE 流解析：文本、thinking、tool_use
└── tools_runner.rs              # 执行模型返回的工具调用并回填观察结果
```

主流程：

```text
前端 invoke("ask_jarvis")
  → core::agent::ask_jarvis
  → pipeline::run_pipeline
  → intent 分类
  → tools 按需加载
  → context 组装动态上下文
  → provider 发起 LLM 流式请求
  → stream 解析输出块与工具调用
  → tools_runner 执行工具
  → emit 事件回前端
```

## Tauri 命令层：`core/commands/`

```text
commands/
├── mod.rs                       # commands 模块声明
├── config.rs                    # get_config、save_config_cmd、get_image_compress_config
├── permission.rs                # cancel_jarvis、resolve_permission
├── session.rs                   # 会话 CRUD、Agent run、子代理、后台任务、计划文档查询
├── history.rs                   # get_session_history
├── checkpoint.rs                # 检查点、分支、回滚、提交、清理 pending operations
├── snapshot.rs                  # 快照创建、查询、详情、分支、回滚
├── sandbox.rs                   # 多 Agent 沙盒创建、查询、完成、放弃、发布、对比
└── merge.rs                     # 合并预览、执行、冲突查询
```

新增前端 `invoke` 命令时通常需要两步：

1. 在 `core/commands/` 的对应文件中实现 `#[tauri::command]` 函数。
2. 在 `src/lib.rs` 的 `tauri::generate_handler![...]` 中注册。

## LLM 抽象：`core/llm/` 与 `core/providers/`

```text
llm/
├── mod.rs                       # LLM 服务抽象层导出
├── api_format.rs                # ApiFormat：OpenAI / Anthropic 协议差异
├── api_client.rs                # HTTP 客户端、重试、流式请求
├── adapters.rs                  # 消息格式转换适配器
└── registry.rs                  # 读取 model_registry.json，提供模型能力查询命令

providers/
├── mod.rs                       # Provider 创建与导出
├── anthropic.rs                 # Anthropic Messages API 实现
└── openai.rs                    # OpenAI Chat Completions 兼容实现
```

设计约束：

- API 协议差异应沉到 `LlmProvider`、`ApiFormat` 和具体 `providers/`，避免在业务流程里散落字符串判断。
- 新模型能力优先更新 `model_registry.json`，再检查 `core/llm/registry.rs` 的读取逻辑。
- `traits.rs` 是 Provider 抽象边界，修改流式能力、thinking 参数、工具调用格式时先看这里。

## 工具系统：`core/tools/`

```text
tools/
├── mod.rs                       # 工具加载、按意图选择、路由与执行入口
├── registry.rs                  # ToolDef、ToolRegistry 与工具 schema 注册宏
├── permission.rs                # 工具权限审批、沙箱路径策略
├── shell_security.rs            # Shell 命令安全检查与风险识别
├── shell_tools.rs               # Bash/PowerShell/后台任务工具
├── file_tools/                  # 文件读取、写入、编辑、搜索、目录与快照记录
│   ├── mod.rs                   # file_tools 聚合导出与注册入口
│   ├── registry.rs              # read/write/edit/search/list 工具 schema
│   ├── common.rs                # 文件大小、行数、忽略规则、行尾/引号归一化
│   ├── read.rs                  # read_file、read_file_skeleton
│   ├── write.rs                 # write_file：普通文本写入 + 检查点/快照
│   ├── edit.rs                  # edit_file：唯一匹配替换 + TOCTOU 防护
│   ├── search.rs                # search_repo、search_in_dir
│   ├── directory.rs             # list_directory、generate_repo_map
│   ├── diff.rs                  # snapshot_engine TextDiff 生成
│   ├── notebook_guard.rs        # 阻止文本工具直接改写 Notebook
│   └── workspace.rs             # 会话工作区、检查点与快照桥接
├── notebook_tools.rs            # Notebook cell 级编辑工具
├── task_tools/                  # 任务工具，分为 todo_write 与持久化任务
│   ├── mod.rs                   # task_tools 聚合导出
│   ├── registry.rs              # task 工具 schema 注册
│   ├── todo_write.rs            # 兼容 TodoWrite 风格的会话内任务列表
│   └── persistent/              # 持久化任务 CRUD 与摘要
│       ├── mod.rs
│       ├── common.rs
│       ├── create.rs
│       ├── update.rs
│       ├── delete.rs
│       ├── list.rs
│       ├── get.rs
│       └── summary.rs
├── system_tools.rs              # 系统信息类工具
├── agent_tools.rs               # 子代理、Skill、计划模式等 Agent 工具
├── agent_registry.rs            # 内置子代理定义与注册
├── claude_code_tools.rs         # Claude Code 兼容工具描述
└── tool_search.rs               # 延迟工具搜索与激活
```

工具调用链：

```text
模型返回 tool_use
  → agent/tools_runner.rs
  → tools/mod.rs 路由
  → tools/permission.rs 判断是否需要审批
  → 具体工具模块执行
  → 执行结果写回 Agent 循环
```

修改建议：

- 新增工具：先扩展对应子模块的 `registry.rs` 定义，再在 `tools/mod.rs` 路由到具体实现。
- 文件工具已拆分到 `core/tools/file_tools/`，普通文本写入/编辑应复用其中的检查点、快照、Notebook 防护和 TOCTOU 逻辑。
- Shell 类工具必须同步考虑 `shell_security.rs` 和权限审批。
- 文件修改类工具要考虑快照/检查点记录，避免绕过现有变更追踪。

## 编排与会话：`core/orchestration/` 与 `core/session/`

```text
orchestration/
├── mod.rs                       # 编排模块导出
├── agent_runs.rs                # Agent run 事件与执行历史
├── subagents.rs                 # 子代理启动、监控、取消与事件记录
├── tasks.rs                     # 任务系统持久化
└── scheduler.rs                 # 调度相关能力

session/
├── mod.rs                       # 会话创建、切换、删除、重命名、元数据管理
├── checkpoint.rs                # 会话与检查点系统的关联逻辑
└── memory.rs                    # 会话记忆与上下文压缩相关逻辑
```

关注点：

- 会话生命周期问题优先看 `session/mod.rs` 和 `commands/session.rs`。
- Agent 执行记录或前端执行面板异常优先看 `orchestration/agent_runs.rs`。
- 子代理运行、取消、事件查询异常优先看 `orchestration/subagents.rs`。
- 任务列表与任务状态异常优先看 `orchestration/tasks.rs` 和 `tools/task_tools.rs`。

## 快照、检查点、沙盒：`core/snapshot_engine/` 与 `core/snapshot_manager/`

```text
snapshot_engine/
├── mod.rs                       # 快照引擎模块导出
├── snapshot.rs                  # 快照结构、快照读写
├── patch.rs                     # 文件变更 Patch 表示
├── replay.rs                    # 快照重放、回滚
├── journal.rs                   # 操作日志，保障原子性与恢复
├── gc.rs                        # 快照垃圾回收
└── multi_agent/
    ├── mod.rs                   # 多 Agent 快照能力导出
    ├── sandbox.rs               # 沙盒分支生命周期
    └── merge.rs                 # 沙盒合并、冲突检测与处理

snapshot_manager/
├── mod.rs                       # 快照管理器导出
├── session_manager.rs           # SessionManagerRegistry，会话级快照上下文
└── store.rs                     # 快照持久化存储
```

相关命令入口：

- 检查点：`core/commands/checkpoint.rs`
- 快照：`core/commands/snapshot.rs`
- 沙盒：`core/commands/sandbox.rs`
- 合并：`core/commands/merge.rs`

## 基础设施与意图分类

```text
infra/
├── mod.rs                       # infra 模块导出
├── background.rs                # 后台任务状态与输出管理
├── debug_logger.rs              # 调试日志
└── prompts.rs                   # 系统提示词与模板

intent/
├── mod.rs                       # 意图分类入口
└── rules.rs                     # 规则分类辅助
```

- 意图分类会影响后续工具加载范围；普通用户模式会先走规则/上下文/LLM 分类，开发者视图发送消息时会在 Agent 管线中直接进入项目操作流程。
- 普通用户携带图片/截图时仍会走意图分类，并追加截图可能代表报错、UI 异常、终端输出或代码问题反馈的提示，避免仅因有图降级成闲聊。
- 当前常见意图标签包括：

- `CHAT`：普通聊天，通常折叠历史工具结果以节省 token。
- `CODE_READ` / `CODE_WRITE` / `CODE_REVIEW`：代码读取、修改与审查。
- `TASK_EXECUTE` / `TASK_PLAN` / `TASK_CONTINUE`：命令执行、复杂任务规划与任务延续。
- `QUESTION`：技术问题、概念解释、方案咨询。
- `MEMORY_QUERY`：记忆查询。
- `SETTINGS`：应用配置、模型、主题、偏好设置。
- `DANGEROUS`：危险操作，需要更严格的确认。

## 运行时数据路径

`src/lib.rs` 会检测运行环境并把数据目录锁定到 `data/`：

```text
开发模式 pnpm tauri dev：项目根目录/data/
开发模式 cargo run：项目根目录/data/
打包后：可执行文件所在目录/data/
```

后端路径相关逻辑集中在：

- `src/lib.rs`：启动时检测、创建、迁移数据目录。
- `core/data_paths.rs`：统一生成会话、任务、快照、工作区等路径。

## 常用开发命令

在仓库根目录运行：

```bash
pnpm tauri dev                   # 启动桌面开发模式
pnpm tauri build                 # 打包桌面应用
```

在 `src-tauri/` 目录运行：

```bash
cargo test                       # 运行 Rust 测试
cargo check                      # Rust 类型检查
cargo fmt                        # Rust 格式化
```

## Agent 快速定位表

| 需求 | 优先查看 |
| --- | --- |
| 前端调用后端命令失败 | `src/lib.rs`、`core/commands/*` |
| 用户发送消息后的 Agent 主流程 | `core/agent/mod.rs`、`core/agent/pipeline.rs` |
| 普通/开发者回复视图影响意图路由 | 前端 `src/composables/usePreferences.ts`、后端 `core/agent/pipeline.rs` |
| SSE 流式输出、thinking、工具调用解析异常 | `core/agent/stream.rs` |
| 工具执行结果异常 | `core/agent/tools_runner.rs`、`core/tools/mod.rs`、具体工具文件 |
| 文件读写/搜索/目录工具异常 | `core/tools/file_tools/` |
| 权限弹窗或危险命令判断异常 | `core/tools/permission.rs`、`core/tools/shell_security.rs`、`commands/permission.rs` |
| 模型参数、thinking、vision 能力异常 | `model_registry.json`、`core/llm/registry.rs`、`core/llm/api_format.rs` |
| OpenAI/Anthropic 协议兼容问题 | `core/traits.rs`、`core/providers/openai.rs`、`core/providers/anthropic.rs` |
| 会话、历史、工作区恢复异常 | `core/session/`、`core/commands/session.rs`、`core/data_paths.rs` |
| Agent 执行记录或执行面板异常 | `core/orchestration/agent_runs.rs`、`core/commands/session.rs` |
| 子代理异常 | `core/orchestration/subagents.rs`、`core/tools/agent_tools.rs` |
| 任务系统异常 | `core/orchestration/tasks.rs`、`core/tools/task_tools.rs` |
| 快照、回滚、沙盒、合并异常 | `core/snapshot_engine/`、`core/snapshot_manager/`、`core/commands/{snapshot,checkpoint,sandbox,merge}.rs` |

## 修改约束

- 不要删除已有中文注释。
- 新增后端错误类型优先使用 `thiserror`，不要随意返回裸字符串错误。
- 新 API 格式应扩展 `LlmProvider`/`ApiFormat`，不要在业务代码中新增零散格式判断。
- 新 Tauri 命令需要同时实现命令函数并在 `src/lib.rs` 注册。
- 修改工具、Shell、文件写入、回滚、合并等能力时，必须考虑权限审批、快照记录和用户数据安全。

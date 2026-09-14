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
├── model_registry.json          # 模型能力注册表（include_str! 编译时内嵌，context/thinking/vision 等）
├── PROJECT_STRUCTURE.md         # src-tauri 后端结构导航文档
├── capabilities/
│   └── default.json             # Tauri 2 capability 权限声明
├── gen/                         # Tauri 生成的 schema/权限辅助文件（勿手动修改）
├── icons/                       # 桌面端、Windows、macOS、iOS、Android 图标资源
├── target/                      # Cargo 构建产物，开发时可忽略
└── src/                         # Rust 后端源码
```

## 源码入口：`src/` 与三层架构

```text
src/
├── main.rs                      # 二进制入口，启动 jarvisagent_lib::run()
├── lib.rs                       # Tauri 后端入口：数据目录、状态、插件、invoke_handler
├── infra/                       # 基础设施层：配置 / 数据库 / LLM / Provider / 状态 / 类型
├── core/                        # 业务层：Agent / 工具 / 编排 / 回滚 / 会话 / 意图
└── command/                     # Tauri 命令层：所有前端可调用命令
```

### `src/lib.rs`

`lib.rs` 是 Tauri 后端的真实启动入口，主要职责：

1. 检测并锁定运行时 `data/` 目录（`AGENT_HOME_DIR`）。
2. `infra::db::init()` 初始化 SQLite（19 张表 + 增量迁移）。
3. 恢复上次工作目录与启动会话。
4. 注册全局状态：`SessionManager`、`BackgroundState`、`CompactingState`、`SubAgentMonitorState`、`ConfigState`、`RuntimeConfigState`、`WorkspaceState`、`SnapshotRegistry`。
5. 注册 Tauri 插件：opener、dialog、fs、window-state。
6. 在 `invoke_handler` 中注册全部前端可调用命令（约 90 个）。
7. 退出时清理所有后台任务进程（`kill_all_process_tree`）。

## 基础设施层：`src/infra/`

```text
infra/
├── mod.rs                       # 基础设施模块入口
├── config/
│   ├── config.rs                # 配置加载与保存（原子写入：先写 tmp 再 rename）；AgentConfig、RuntimeSettings
│   └── data_paths.rs            # data 目录、会话、图片、快照等运行期路径管理
├── db/
│   ├── mod.rs                   # SQLite 连接管理与初始化
│   └── schema.rs                # 19 张表 schema 定义 + 增量迁移
├── llm/
│   ├── api_format.rs            # ApiFormat 枚举（认证头、版本头）
│   ├── api_client.rs            # HTTP 客户端、指数退避、429 Retry-After
│   ├── adapters.rs              # Anthropic ↔ OpenAI 消息格式转换
│   ├── registry.rs              # 读取 model_registry.json 的模型能力与命令入口
│   └── token_count.rs           # tiktoken BPE Token 计数
├── providers/
│   ├── anthropic.rs             # Anthropic Messages API 实现
│   └── openai.rs                # OpenAI Chat Completions 兼容实现
├── state/
│   ├── state.rs                 # SessionManager、SessionContext、WorkspaceState、SnapshotRegistry
│   └── events.rs                # Tauri 事件名常量（domain:action 规范）
├── types/
│   ├── models.rs                # 消息、工具、会话、计划文档等共享数据模型
│   ├── traits.rs                # LlmProvider 等核心 trait 抽象
│   ├── error.rs                 # AgentError、ApiError、DbError 等分层错误类型
│   └── constants.rs             # 全局常量
├── background.rs                # 后台任务状态与输出管理（bg-task-done 事件推送）
└── debug_logger.rs              # 调试日志
```

## Agent 管线：`core/agent/`

```text
agent/
├── mod.rs                       # ask_jarvis / resume_jarvis Tauri 命令入口
├── pipeline.rs                  # run_pipeline / resume_pipeline 主流程（五阶段流水线）
├── context.rs                   # 动态上下文注入：记忆、技能、目录结构等
├── stream.rs                    # SSE 流解析：文本、thinking、tool_use
├── tools_runner.rs              # 执行模型返回的工具调用并回填观察结果
├── prompts.rs                   # 系统提示词三层组装（Base + Audience + WorkMode）
├── prompts/                     # 提示词模板目录
│   ├── base_p0.md ~ base_p2.md  # 基础提示词
│   ├── subagent.md              # 子代理提示词
│   ├── audience/                # user.md / developer.md 交流风格
│   ├── mode/                    # chat.md / edit.md / plan.md 工作模式规则
│   └── os/                      # linux.md / macos.md / windows.md OS 规则
└── reflection/                  # 反思审查机制
    ├── mod.rs
    ├── prompt.rs                # 反思提示词
    └── strategy.rs              # 反思策略与防循环控制
```

主流程：

```text
前端 invoke("ask_jarvis")
  → core::agent::ask_jarvis
  → pipeline::run_pipeline（初始化 → 复杂任务检测 → 上下文构建 → 主循环 → 收尾）
  → complex_task 判定 → tools 按 WorkMode 加载 → context 组装动态上下文
  → provider 发起 LLM 流式请求 → stream 解析输出块与工具调用
  → tools_runner 执行工具 → reflection 反思审查
  → 压缩检查 → 写回消息与执行记录 → emit 事件回前端
```

关键约束：

- 取消令牌（`CancellationToken`）贯穿全流程，用户可随时中断；中断可恢复（`resume_jarvis`）。
- 循环次数受常量限制（超 30 轮暂停确认，绝对上限 200 轮）。
- 流式中断会触发 `fix_broken_tool_call_pairs` 防御性修复，保证 tool_calls 与 tool_result 配对。

## Tauri 命令层：`src/command/`

```text
command/
├── mod.rs                       # command 模块声明
├── config.rs                    # get_config、save_config_cmd、get_image_compress_config
├── app_config.rs                # 窗口状态、UI 偏好、技能激活状态
├── permission.rs                # cancel_jarvis、resolve_permission、权限状态查询
├── session.rs                   # 会话 CRUD、Agent run、子代理、后台任务、计划文档查询
├── history.rs / history_types.rs # 历史消息渲染与类型
├── checkpoint.rs                # 检查点、分支、回滚、提交、清理 pending operations
├── snapshot.rs                  # 快照创建、查询、详情、分支、回滚
├── sandbox.rs                   # 多 Agent 沙盒创建、查询、完成、放弃、发布、对比
├── merge.rs                     # 合并预览、执行、冲突查询
└── skill.rs                     # 技能列表与详情查询
```

新增前端 `invoke` 命令时通常需要两步：

1. 在 `src/command/` 的对应文件中实现 `#[tauri::command]` 函数。
2. 在 `src/lib.rs` 的 `tauri::generate_handler![...]` 中注册。

## LLM 抽象：`infra/llm/` 与 `infra/providers/`

```text
infra/llm/
├── api_format.rs                # ApiFormat：OpenAI / Anthropic 协议差异
├── api_client.rs                # HTTP 客户端、重试、流式请求
├── adapters.rs                  # 消息格式转换适配器
├── registry.rs                  # 读取 model_registry.json，提供模型能力查询命令
└── token_count.rs               # tiktoken BPE 精确 Token 计数

infra/providers/
├── anthropic.rs                 # Anthropic Messages API 实现
└── openai.rs                    # OpenAI Chat Completions 兼容实现
```

设计约束：

- API 协议差异应沉到 `LlmProvider`（`infra/types/traits.rs`）、`ApiFormat` 和具体 `infra/providers/`，避免在业务流程里散落字符串判断。
- 新模型能力优先更新 `model_registry.json`，再检查 `infra/llm/registry.rs` 的读取逻辑。
- 修改流式能力、thinking 参数、工具调用格式时先看 `infra/types/traits.rs`。

## 工具系统：`core/tools/`

```text
tools/
├── mod.rs                       # 工具系统中枢：意图过滤、路由分发、技能加载、写工具拦截
├── file_tools/                  # 文件读取、写入、编辑、搜索、符号、目录（14 个文件）
│   ├── mod.rs                   # 聚合导出与注册入口
│   ├── registry.rs              # read/write/edit/search/list 等工具 schema
│   ├── read.rs / write.rs / edit.rs / patch.rs
│   ├── delete.rs / rename.rs
│   ├── directory.rs / search.rs / symbol.rs
│   ├── diff.rs / common.rs / workspace.rs
├── shell_tools/                 # Shell 执行（10 个文件）
│   ├── mod.rs                   # 聚合导出与注册入口
│   ├── execution.rs / background.rs / readonly.rs / git.rs
│   └── security.rs / guards.rs / regexes.rs / types.rs / utils.rs
├── agent_tools/                 # Agent 专用工具
│   ├── mod.rs                   # 注册入口（LoadSkill / GetToolCatalog）
│   ├── subagent.rs              # 子代理执行引擎（RunSubagent / RunSubagentsSequentially）
│   ├── skill.rs                 # 技能加载
│   ├── compact.rs               # 上下文压缩 + 记忆整理（CompactConversation / ConsolidateMemory）
│   ├── plan.rs                  # 方案审批（ProposePlan）
│   └── switch_mode.rs           # 模式切换（SwitchWorkMode）
├── task_tools/                  # 任务与待办
│   ├── mod.rs
│   ├── registry.rs / todo_write.rs
│   └── persistent/              # 持久化任务 CRUD 与摘要
├── search_tools/                # Glob + Grep 搜索（FindFiles / SearchText / CodeSearch）
├── notebook_tools/              # Notebook cell 级编辑
│   ├── mod.rs
│   └── notebook_guard.rs        # 阻止文本工具直接改写 Notebook
├── system_tools/                # 工作区设置（SetWorkspace；OS/CWD/Home 已由提示词自动注入）
└── framework/                   # 工具框架层
    ├── registry.rs              # 全局工具注册表（define_tools! / tool_def! 宏）
    ├── tool_search.rs           # 渐进式工具披露（DiscoverTools / ExecuteTool）
    ├── permission.rs            # 工具权限审批、沙箱路径策略
    ├── agent_registry.rs        # 内置子代理定义与注册
    └── tool_call_logger.rs      # 工具调用审计日志
```

工具调用链：

```text
模型返回 tool_use
  → agent/tools_runner.rs
  → tools/mod.rs 路由（含意图与 WorkMode 过滤）
  → framework/permission.rs 判断是否需要审批
  → 具体工具模块执行
  → 执行结果写回 Agent 循环
```

修改建议：

- 新增工具：在对应子模块实现执行函数，再通过 `define_tools!` 宏注册到 `framework/registry.rs`，并在 `tools/mod.rs` 的路由中分发。
- 工具集保持稳定（tool 参数顺序不变）以利于 prompt cache 命中，不要随意增删工具定义。
- 文件修改类工具要复用时 `file_tools` 中的检查点、快照与 Notebook 防护逻辑，避免绕过变更追踪。
- Shell 类工具必须同步考虑 `framework/permission.rs` 审批与 `shell_tools/security.rs` 安全检查。
- 写工具（WriteFile、EditFile、RunCommand 等）默认注册为延迟工具，聊天意图下禁止调用。

## 编排系统：`core/orchestration/`

```text
orchestration/
├── mod.rs                       # 编排模块导出
├── agent_runs.rs                # 主 Agent run 事件与执行历史
├── agent_run_repository.rs      # Agent 运行记录 SQLite 仓储
├── subagents.rs                 # 子代理启动、监控、取消与事件记录
├── tasks.rs                     # 任务系统持久化与依赖管理
├── scheduler.rs                 # 基于依赖图的并行调度（JoinSet 流式调度）
└── multi_agent/                 # 多 Agent 协作
    ├── sandbox.rs               # 沙盒分支生命周期
    └── merge.rs                 # LCA 三方合并、冲突检测与处理
```

调度约束：

- 无依赖任务自动并行，谁先完成就解锁谁的下游。
- 5 分钟超时保护。
- 子代理事件通过 `subagent-updated` 事件批量节流推送前端。

## 会话与记忆：`core/session/`

```text
session/
├── mod.rs                       # 会话创建、切换、删除、重命名、元数据管理
├── repository.rs                # 会话 SQLite 仓储 + 消息加载
├── resource_repository.rs       # 附件/资源 SQLite 仓储
└── memory.rs                    # 会话记忆与上下文压缩（视图引用方案）
```

视图引用方案：`session_messages`（唯一数据源，只增不改） + `session_memory`（只存 message_id 列表），压缩产生 `source='compact'` 摘要消息，原始消息永久可恢复。

关注点：

- 会话生命周期问题优先看 `session/mod.rs` 和 `command/session.rs`。
- Agent 执行记录或前端执行面板异常优先看 `orchestration/agent_runs.rs`。
- 子代理运行、取消、事件查询异常优先看 `orchestration/subagents.rs`。
- 任务列表与任务状态异常优先看 `orchestration/tasks.rs` 和 `tools/task_tools/`。

## 回滚引擎与多 Agent 沙盒：`core/rollback/` + `core/orchestration/multi_agent/`

```text
rollback/
├── mod.rs                       # 回滚引擎模块导出
├── snapshot.rs                  # 文件级快照结构与快照树
├── patch.rs                     # 文件变更 Patch（Create/Update/Delete/Rename）
├── replay.rs                    # 快照重放 + 原子文件回滚（staging + rename）
├── store.rs                     # 快照 SQLite 持久化
├── gc.rs                        # 三阶段垃圾回收（脱离分支 → 孤儿快照 → 孤儿内容）
├── journal.rs                   # 操作日志
├── session_manager.rs           # 会话级快照注册与分支管理
└── rollback_logger.rs           # 回滚日志

orchestration/multi_agent/
├── sandbox.rs                   # 沙盒隔离
└── merge.rs                     # LCA 三方合并 + 冲突解决
```

相关命令入口：

- 检查点：`command/checkpoint.rs`
- 快照：`command/snapshot.rs`
- 沙盒：`command/sandbox.rs`
- 合并：`command/merge.rs`

## 复杂任务检测：`core/complex_task.rs`

方案审批流程的两道确定性检测（正则，零 LLM 开销），与模型的语义判断互补：

- 前置 `is_complex_task`：识别用户输入中"需要方案审批的复杂任务"表达，命中则
  pipeline 首轮强制切换 Plan 模式；其余输入不做意图区分，直接交给主 LLM 结合
  上下文处理。误判可按 mode/plan.md 的降级条件自行切回 Edit。
- 后置 `detect_plan_in_text`：Edit/Plan 模式下检测 LLM 是否绕过 `ProposePlan`
  把方案写进回复正文，命中则重定向到方案审批流程。

## 运行时数据路径

`src/lib.rs` 会检测运行环境并把数据目录锁定到 `data/`（`AGENT_HOME_DIR`）：

```text
pnpm tauri dev：       项目根目录/data/
cargo run（src-tauri）：项目根目录/data/
打包后：               可执行文件所在目录/data/
```

后端路径相关逻辑集中在：

- `src/lib.rs`：启动时检测、创建数据目录。
- `infra/config/data_paths.rs`：统一生成 SQLite、全局数据、图片、会话、快照等路径。

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
cargo clippy                     # Rust lint 检查
```

## Agent 快速定位表

| 需求 | 优先查看 |
| --- | --- |
| 前端调用后端命令失败 | `src/lib.rs`、`src/command/*` |
| 用户发送消息后的 Agent 主流程 | `core/agent/mod.rs`、`core/agent/pipeline.rs` |
| 中断恢复 Agent run | `core/agent/pipeline.rs`（resume_pipeline）、`command/session.rs`（prepare_resume_agent_run） |
| SSE 流式输出、thinking、工具调用解析异常 | `core/agent/stream.rs` |
| 反思审查行为异常 | `core/agent/reflection/` |
| 工具执行结果异常 | `core/agent/tools_runner.rs`、`core/tools/mod.rs`、具体工具文件 |
| 文件读写/搜索/目录工具异常 | `core/tools/file_tools/` |
| 权限弹窗或危险命令判断异常 | `core/tools/framework/permission.rs`、`core/tools/shell_tools/security.rs`、`command/permission.rs` |
| 模型参数、thinking、vision 能力异常 | `model_registry.json`、`infra/llm/registry.rs`、`infra/llm/api_format.rs` |
| OpenAI/Anthropic 协议兼容问题 | `infra/types/traits.rs`、`infra/providers/openai.rs`、`infra/providers/anthropic.rs` |
| Token 计数不准确 | `infra/llm/token_count.rs` |
| 会话、历史、工作区恢复异常 | `core/session/`、`command/session.rs`、`infra/config/data_paths.rs` |
| 数据库 schema / 迁移异常 | `infra/db/schema.rs`、`infra/db/mod.rs` |
| Agent 执行记录或执行面板异常 | `core/orchestration/agent_runs.rs`、`command/session.rs` |
| 子代理异常 | `core/orchestration/subagents.rs`、`core/tools/agent_tools/subagent.rs` |
| 任务系统异常 | `core/orchestration/tasks.rs`、`core/tools/task_tools/` |
| 快照、回滚异常 | `core/rollback/`、`command/{snapshot,checkpoint}.rs` |
| 沙盒、合并异常 | `core/orchestration/multi_agent/`、`command/{sandbox,merge}.rs` |
| 复杂任务误判 | `core/complex_task.rs` |
| 配置读写异常 | `infra/config/config.rs`、`command/config.rs` |

## 修改约束

- 不要删除已有中文注释。
- 新增后端错误类型优先使用 `thiserror`，不要随意返回裸字符串错误。
- 新 API 格式应扩展 `LlmProvider`/`ApiFormat`，不要在业务代码中新增零散格式判断。
- 新 Tauri 命令需要同时实现命令函数并在 `src/lib.rs` 注册。
- 工具定义统一通过 `define_tools!` / `tool_def!` 宏注册，保证工具 schema 稳定以利于 prompt cache。
- 修改工具、Shell、文件写入、回滚、合并等能力时，必须考虑权限审批、快照记录和用户数据安全。
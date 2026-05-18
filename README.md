# JarvisAgent

<div align="center">

**一个 AI 驱动的桌面端编程助手**

基于 Tauri 2.0 + Vue 3 + Rust 构建，完整 Agent 自主循环，支持 20+ 主流 LLM 模型，具备快照版本控制、多 Agent 沙箱、方案审批、双轴模式系统等企业级能力

</div>

---

## ✨ 特性

- 🤖 **多模型支持** — DeepSeek、Claude、GPT、Gemini、Qwen、豆包、MIMO 等 20+ 主流 LLM
- 🎛️ **双轴模式系统** — Audience（User/Developer）× WorkMode（Chat/Edit/Plan）正交组合，工具集与提示词按模式动态切换
- 🔄 **完整 Agent 循环** — 意图分类 → 工具加载 → 动态上下文注入 → SSE 流式输出 → 自主决策执行
- 🔀 **自动模式切换** — Edit 模式检测到复杂任务自动切 Plan 深度规划，审批后切回委派子 Agent 并行执行
- 📸 **快照引擎** — 文件级树形版本控制，原子化回滚，分支管理，多 Agent 沙箱并行与合并
- 🤝 **子代理委派** — 主代理编排任务图，子 Agent 在干净上下文中独立执行，调度器自动并行
- 📋 **方案审批机制** — 复杂任务先提交结构化方案，用户预览编辑后批准，按依赖图调度执行
- 💾 **会话持久化** — SQLite 存储完整对话历史，视图引用方案消除双全量存储，多会话管理
- 🧩 **记忆系统** — 全局记忆 + 项目记忆，长期保留用户偏好与项目上下文
- 📊 **任务调度器** — 基于依赖图的并行调度，JoinSet 流式执行，无依赖任务自动并行，5 分钟超时保护
- 🗜️ **三级上下文压缩** — L1 micro（截断旧工具结果）→ L2 mid（移除早期 thinking）→ L3 auto（LLM 摘要 + transcript）
- 🎨 **现代 UI** — 类 IDE 毛玻璃界面（Glassmorphism），无边框窗口，明暗主题切换
- ⏹️ **中途取消** — 随时中断正在执行的 Agent 任务，保留部分输出
- 🖼️ **多模态理解** — 图片输入，自动压缩优化
- 🛡️ **Shell 安全** — 递归列目录强制排除依赖目录，危险命令检测，权限分级审批

## 🏗️ 技术栈

| 层级 | 技术 | 说明 |
|------|------|------|
| 前端框架 | Vue 3 + TypeScript | Composition API + `<script setup>` |
| 状态管理 | Pinia | 4 个 Store：session / chat / agent / permission |
| 桌面框架 | Tauri 2.0 | Rust 后端，轻量高性能 |
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

### 工作模式

| 模式 | 工具集 | 说明 |
|------|--------|------|
| **Chat** | 14 个只读工具 | 轻量问答、信息查询 |
| **Edit** | 全部 30 个工具 | 代码开发、文件编辑、任务执行 |
| **Plan** | 21 个规划工具 | 只读探索 + 方案制定 + 任务图拆解 |

用户类型（Audience）分为 **普通用户** 和 **开发者**，仅影响 UI 渲染细节和交流风格，不影响工具可用性。

## 📁 项目结构

```
JarvisAgent/
├── src/                              # Vue 3 前端
│   ├── main.ts                       # 应用入口
│   ├── App.vue                       # 根布局
│   ├── types/index.ts                # 全部 TypeScript 类型定义
│   ├── stores/                       # Pinia 状态管理
│   │   ├── session.ts                #   会话生命周期 + 消息缓冲区
│   │   ├── chat.ts                   #   核心交互：发送/取消/撤回/渲染
│   │   ├── agent.ts                  #   Agent/子代理运行状态追踪
│   │   └── permission.ts             #   权限请求 + 方案审批状态
│   ├── composables/
│   │   ├── useAgentEvents.ts         #   后端事件监听中枢 → 分发到各 Store
│   │   ├── useTheme.ts               #   亮/暗主题切换
│   │   ├── useWindow.ts              #   Tauri 窗口控制
│   │   └── usePreferences.ts         #   用户偏好持久化
│   ├── utils/
│   │   ├── toolDisplay.ts            #   工具调用分组与展示摘要
│   │   ├── agentTurnRender.ts        #   Agent 轮次渲染
│   │   ├── markdown.ts               #   Markdown 渲染
│   │   └── html.ts                   #   HTML 工具函数
│   ├── services/
│   │   └── snapshotService.ts        #   快照 API 封装
│   └── components/
│       ├── layout/                   # TitleBar, Sidebar
│       ├── chat/                     # ChatArea, TerminalInput, AgentPanel, TodoPanel, ExecutionPanel
│       ├── common/                   # PermissionModal, PlanPreviewPanel, ConfirmModal
│       ├── checkpoint/               # CheckpointTimeline
│       ├── snapshot/                 # SnapshotTimeline, DiffViewer, LivePreview
│       └── settings/                 # SettingsPanel（多预设 + 双轴选择）
├── src-tauri/                        # Rust 后端
│   ├── src/
│   │   ├── lib.rs                    # Tauri 入口：状态注册 + 命令绑定
│   │   ├── core/
│   │   │   ├── mod.rs                # 模块导出
│   │   │   ├── agent/
│   │   │   │   ├── pipeline.rs       # 五阶段 Agent 管线主循环
│   │   │   │   ├── stream.rs         # SSE 流式解析（Anthropic + OpenAI）
│   │   │   │   ├── context.rs        # 动态上下文构建
│   │   │   │   └── tools_runner.rs   # 工具调用并行执行引擎
│   │   │   ├── commands/             # Tauri 命令处理（11 个文件）
│   │   │   │   ├── session.rs        #   会话 CRUD + 撤回
│   │   │   │   ├── checkpoint.rs     #   检查点 + 回滚
│   │   │   │   ├── snapshot.rs       #   快照引擎命令
│   │   │   │   ├── config.rs         #   配置读写
│   │   │   │   ├── permission.rs     #   权限审批回调
│   │   │   │   ├── window_state.rs   #   窗口状态 + UI 偏好
│   │   │   │   ├── merge.rs          #   分支合并
│   │   │   │   ├── sandbox.rs        #   多 Agent 沙箱
│   │   │   │   └── history.rs        #   历史记录渲染
│   │   │   ├── infra/
│   │   │   │   ├── prompts.rs        #   系统提示词（三层组装）
│   │   │   │   ├── background.rs     #   后台任务管理 + Tauri 事件推送
│   │   │   │   └── debug_logger.rs   #   调试日志
│   │   │   ├── intent/
│   │   │   │   ├── mod.rs            #   三层意图分类（规则→上下文→LLM 兜底）
│   │   │   │   └── rules.rs          #   正则规则引擎 + JSON 外部规则加载
│   │   │   ├── llm/
│   │   │   │   ├── api_format.rs     #   ApiFormat 枚举（认证头、版本头）
│   │   │   │   ├── api_client.rs     #   HTTP 客户端 + 指数退避 + 429 Retry-After
│   │   │   │   ├── adapters.rs       #   Anthropic ↔ OpenAI 消息格式转换
│   │   │   │   ├── registry.rs       #   模型能力注册表（9 种思考参数变体）
│   │   │   │   └── token_count.rs    #   tiktoken BPE 精确 Token 计数
│   │   │   ├── providers/
│   │   │   │   ├── anthropic.rs      #   Anthropic Messages API
│   │   │   │   └── openai.rs         #   OpenAI Chat Completions API
│   │   │   ├── tools/
│   │   │   │   ├── mod.rs            #   工具系统中枢 + WorkMode 过滤
│   │   │   │   ├── file_tools/       #   文件工具（13 个文件）
│   │   │   │   ├── shell_tools/      #   Shell + 后台任务工具（9 个文件）
│   │   │   │   ├── agent_tools/      #   子代理、技能、压缩、方案审批、模式切换
│   │   │   │   ├── task_tools/       #   轻量待办 + 持久化任务 CRUD
│   │   │   │   ├── search_tools/     #   Glob + Grep 搜索
│   │   │   │   ├── notebook_tools/   #   Jupyter Notebook cell 编辑
│   │   │   │   ├── system_tools/     #   系统信息 + 工作区设置
│   │   │   │   └── framework/        #   工具注册表、权限、渐进式披露
│   │   │   ├── orchestration/
│   │   │   │   ├── scheduler.rs      #   基于依赖图的并行任务调度器
│   │   │   │   ├── subagents.rs      #   子 Agent 生命周期 + 事件持久化
│   │   │   │   ├── agent_runs.rs     #   主 Agent 运行记录 + 检查点
│   │   │   │   ├── agent_run_repository.rs  # Agent 运行 SQLite 仓储
│   │   │   │   └── tasks.rs          #   任务 CRUD + 依赖管理
│   │   │   ├── rollback/
│   │   │   │   ├── snapshot.rs       #   快照 + 快照树数据结构
│   │   │   │   ├── patch.rs          #   补丁系统（Create/Update/Delete/Rename）
│   │   │   │   ├── replay.rs         #   重放引擎 + 原子文件回滚
│   │   │   │   ├── store.rs          #   快照 SQLite 持久化
│   │   │   │   ├── gc.rs             #   三阶段垃圾回收
│   │   │   │   ├── journal.rs        #   操作日志
│   │   │   │   ├── session_manager.rs #  会话快照管理器
│   │   │   │   └── multi_agent/      #   沙箱 + 分支合并引擎
│   │   │   ├── session/
│   │   │   │   ├── mod.rs            #   会话持久化入口
│   │   │   │   ├── repository.rs     #   会话 SQLite 仓储 + 消息加载
│   │   │   │   ├── resource_repository.rs  # 附件/资源 SQLite 仓储
│   │   │   │   └── memory.rs         #   三级压缩 + 记忆 Agent
│   │   │   ├── db/
│   │   │   │   ├── mod.rs            #   SQLite 连接管理（全局 Mutex）
│   │   │   │   └── schema.rs         #   17 张表 schema 定义 + 增量迁移
│   │   │   ├── config.rs             #   AgentConfig + RuntimeSettings + 原子写入
│   │   │   ├── constants.rs          #   全局常量
│   │   │   ├── error.rs              #   分层错误类型（AgentError / ApiError / DbError）
│   │   │   ├── state.rs              #   SessionManager + SessionContext
│   │   │   ├── models.rs             #   核心数据模型
│   │   │   ├── events.rs             #   Tauri 事件名常量（domain:action 规范）
│   │   │   └── traits.rs             #   LlmProvider trait 抽象
│   │   └── main.rs
│   ├── model_registry.json           # 模型能力注册表
│   ├── intent_rules.json             # 意图分类外部规则
│   └── Cargo.toml
├── doc/                              # 架构文档
└── package.json
```

## 🔧 核心架构（已重构）

### 新的 Agent 管线

```
用户输入 → 多层意图分类（规则 → 上下文 → LLM）
           → 根据 WorkMode 动态加载工具集（Chat / Edit / Plan）
           → 动态上下文注入（意图标签 + 全局记忆 + 项目映射 + 技能列表）
           → 模块化子 Agent 并行执行（自研调度器）
           → 结果聚合与持久化（快照、记忆、会话标题）
```

### 双轴模式系统

```
Audience 轴（谁在用）         WorkMode 轴（在干什么）
  User ───────── Developer      Chat ────── Edit ────── Plan
  ↑ 只有用户手动切换                ↑ 用户手动 + Agent 自动切

  Audience → UI 渲染 + 交流风格     WorkMode → 工具集 + 系统提示词
```

提示词采用三层组装：`BASE_SYSTEM_PROMPT`（通用规则 + OS 检测）+ `Audience 风格` + `WorkMode 规则`。模式切换不重启 Pipeline，下一轮 LLM 请求自动使用新提示词和工具集。

### 视图引用方案（数据库）

```
session_messages 表（唯一数据源，只增不减）
  └── message_id + content_json + seq + source（chat/compact）

session_memory 表（LLM 活动视图索引）
  └── message_ids: ["id1", "id2", ...]  ← 不存消息内容，只存 ID 列表

压缩：摘要写入 session_messages（source='compact'）→ 更新 message_ids → 原始消息完整保留
回滚：隐藏检查点后的消息 → 更新 message_ids → 原始消息从 session_messages 恢复
```

### 三级上下文压缩

```
L1 micro（每轮常驻）：截断旧的工具调用结果，保留最近 20 条
L2 mid（>80% 上限）：移除早期 thinking 块，保留最近 5 个
L3 auto（>95% 上限）：LLM 摘要 + 保存 transcript 文件
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
| 文件写入 | WriteFile, EditFile, ApplyPatch | 写文件、搜索替换编辑、应用 diff |
| 目录 | ListDirectory, SearchRepo | 列目录、生成仓库地图 |
| 搜索 | SearchText, FindFiles | 文本搜索(grep)、文件名搜索(glob) |
| 符号 | FindSymbol, ReadSymbol, FindReferences, CodeSearch | 符号定位、读定义、查引用 |
| Shell | RunCommand, StartBackgroundCommand, CheckBackgroundCommand | 命令执行、后台服务、状态检查 |
| Git | RunGitCommand | 只读 Git 操作 |
| 任务 | CreateTask, UpdateTask, DeleteTask, ListTasks, GetTask, SummarizeTasks | 持久化任务图 CRUD |
| 待办 | UpdateTodos | 轻量待办清单 |
| 子代理 | RunSubagent, RunSubagentsSequentially | 委派子代理、启动调度器 |
| 规划 | ProposePlan, SwitchWorkMode | 方案审批、模式切换 |
| 会话 | CompactConversation, ConsolidateMemory | 手动压缩、记忆整理 |
| 系统 | GetSystemInfo, SetWorkspace | 系统信息、工作区设置 |
| 搜索 | SearchTools | 延迟工具搜索激活 |
| Notebook | EditNotebook | Jupyter Notebook cell 编辑 |

## 🛡️ 安全特性

- **沙箱限制** — 会话绑定工作目录，路径遍历自动拦截
- **Shell 安全** — 递归列目录（dir /s、tree）强制排除 node_modules 等依赖目录
- **权限审批** — Shell 等敏感操作需用户确认，30 秒超时自动拒绝
- **循环检测** — Agent 循环超 30 轮暂停确认，绝对上限 500 轮
- **429 限流** — 解析 Retry-After 头按服务器建议等待
- **快照回滚** — 所有文件操作可追溯可撤销，原子化写入防崩溃

## 🙏 致谢

- **小米大模型团队** — 感谢小米模型的「创造者百万亿 Token 激励计划」与「Agent 生态共建计划」
- **[Claude Code](https://github.com/anthropics/claude-code)** — 架构设计的灵感来源

## 📄 许可证

MIT License

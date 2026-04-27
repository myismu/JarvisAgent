# JarvisAgent

<div align="center">

**一个强大的 AI 编程助手桌面应用**

基于 Tauri 2.0 + Vue 3 构建，支持多种主流 LLM 模型，具备深度思考、子代理委派、方案审批等企业级 Agent 能力

[English](#english) | [中文](#中文)

</div>

---

## 中文

### ✨ 特性

- 🤖 **多模型支持** - 支持 DeepSeek、Claude、GPT、Gemini、Qwen、豆包、MIMO 等 20+ 主流 LLM 模型
- 🧠 **深度思考模式** - 支持 Extended Thinking / Reasoning 模式，让 AI 展示推理过程
- 🔄 **智能意图识别** - 自动区分闲聊、项目操作、记忆查询等不同意图，按需加载工具
- 📁 **沙箱工作目录** - 支持会话级别的目录限制，确保操作安全
- 🛠️ **丰富的工具集** - 文件读写、Shell 命令、Git 操作、代码搜索、任务管理等 20+ 内置工具
- 🤝 **子代理委派** - 主代理编排任务，子代理在干净上下文中执行，避免污染主对话
- 📋 **方案审批机制** - 复杂任务需提交方案，用户预览确认后执行
- 💾 **会话持久化** - 自动保存对话历史，支持多会话管理与切换
- 🧩 **记忆系统** - 全局记忆与项目记忆，长期记住用户偏好与项目上下文
- 🎨 **现代 UI** - 类 IDE 风格界面，无边框窗口，支持明暗主题切换
- 🔄 **API 自动重试** - 指数退避重试机制，网络波动时自动恢复
- ⏹️ **中途取消** - 支持随时中断正在执行的 Agent 任务
- 🖼️ **多模态理解** - 支持图片输入，自动压缩优化

### 🏗️ 技术栈

| 层级 | 技术 | 说明 |
|------|------|------|
| 前端 | Vue 3 + TypeScript | 响应式 UI，Composition API |
| 桌面框架 | Tauri 2.0 | Rust 后端，轻量高性能 |
| 后端 | Rust + Tokio | 异步运行时，SSE 流式处理 |
| HTTP | Reqwest | 流式 API 调用，支持 OpenAI / Anthropic 格式 |
| 构建 | Vite | 极速 HMR 开发体验 |

### 🚀 快速开始

#### 环境要求

- Node.js >= 18
- Rust >= 1.70
- pnpm >= 8 (推荐) 或 npm

#### 安装依赖

```bash
# 安装前端依赖
pnpm install

# Rust 依赖会在首次构建时自动安装
```

#### 开发模式

```bash
pnpm tauri dev
```

#### 构建发布

```bash
pnpm tauri build
```

### ⚙️ 配置

首次运行时，点击右上角设置按钮配置：

1. **API Key** - 你的 LLM API 密钥
2. **Base URL** - API 端点地址（自动补全路径）
3. **API Format** - 选择 `openai` 或 `anthropic` 格式
4. **主模型** - 用于主代理和子代理的模型
5. **工具模型** - 用于意图分类和记忆管理的轻量模型（可选更便宜的模型）
6. **深度思考** - 开启/关闭思考模式

支持多预设（Profile）管理，可为不同场景切换配置。

#### 支持的模型

| 提供商 | 模型 | API 格式 | 思考模式 | 视觉 |
|--------|------|----------|----------|------|
| DeepSeek | V4 Pro / V4 Flash | openai | ✅ reasoning_effort | ❌ |
| DeepSeek | V3 Chat | openai | ❌ | ❌ |
| DeepSeek | R1 Reasoner | openai | 始终推理 | ❌ |
| Anthropic | Opus 4.5 / Sonnet 4.5 | anthropic | ✅ thinking | ✅ |
| Anthropic | 3.7 Sonnet | anthropic | ✅ thinking | ✅ |
| Anthropic | 3.5 Sonnet / Haiku | anthropic | ❌ | ✅ |
| OpenAI | GPT-4o / 4o mini | openai | ❌ | ✅ |
| OpenAI | GPT-4.1 / 4.1 mini | openai | ❌ | ❌ |
| OpenAI | o3 / o3-mini / o4-mini | openai | ✅ reasoning_effort | ❌ |
| Google | Gemini 2.5 Pro / Flash | openai | ✅ thinkingBudget | ✅ |
| Google | Gemini 2.0 Flash | openai | ❌ | ✅ |
| Alibaba | Qwen3-235B / 32B / 14B | openai | ✅ enable_thinking | ❌ |
| Alibaba | Qwen Plus / Turbo | openai | ❌ | ✅ |
| ByteDance | 豆包 Seed 2.0 Pro / Lite | openai | ✅ thinking | ✅ |
| XiaoMi | MIMO V2 Flash | anthropic | ❌ | ❌ |

> 💡 模型能力注册表位于 `src-tauri/model_registry.json`，可自行扩展新模型。

### 📁 项目结构

```
JarvisAgent/
├── src/                          # Vue 前端源码
│   ├── components/
│   │   ├── chat/                 # 聊天相关组件
│   │   │   ├── ChatArea.vue      # 对话主区域
│   │   │   ├── TerminalInput.vue # 输入框（支持图片粘贴、拖拽）
│   │   │   └── AgentPanel.vue    # Agent 步骤可视化面板
│   │   ├── common/               # 通用组件
│   │   │   ├── PermissionModal.vue   # 权限确认弹窗
│   │   │   └── PlanPreviewPanel.vue  # 方案预览面板
│   │   ├── layout/               # 布局组件
│   │   │   ├── Sidebar.vue       # 侧边栏（会话列表）
│   │   │   └── TitleBar.vue      # 自定义标题栏
│   │   └── settings/
│   │       └── SettingsPanel.vue # 设置面板（多预设管理）
│   ├── composables/              # Vue Composables
│   │   ├── useJarvis.ts          # 核心 Agent 交互逻辑
│   │   ├── useTheme.ts           # 主题切换
│   │   └── useWindow.ts          # 窗口控制
│   ├── types/                    # TypeScript 类型定义
│   └── assets/                   # 静态资源
├── src-tauri/                    # Rust 后端源码
│   ├── src/
│   │   ├── core/
│   │   │   ├── mod.rs            # 核心 Agent 循环（意图分类→工具调用→流式响应）
│   │   │   ├── adapters.rs       # OpenAI / Anthropic 双格式适配器
│   │   │   ├── background.rs     # 后台任务管理器
│   │   │   ├── cancellation.rs   # 任务取消机制
│   │   │   ├── config.rs         # 配置管理（多预设、自动迁移）
│   │   │   ├── constants.rs      # 常量定义（路径、阈值）
│   │   │   ├── memory.rs         # 上下文压缩与记忆管理
│   │   │   ├── models.rs         # 数据模型（请求/响应/消息）
│   │   │   ├── prompts.rs        # 系统提示词
│   │   │   ├── registry.rs       # 模型能力注册表
│   │   │   ├── sessions.rs       # 会话持久化
│   │   │   ├── tasks.rs          # 任务看板管理
│   │   │   └── tools/            # 工具实现
│   │   │       ├── mod.rs            # 工具注册与路由分发
│   │   │       ├── file_tools.rs     # 文件读写、搜索、骨架提取
│   │   │       ├── shell_tools.rs    # Shell 命令、Git、后台任务
│   │   │       ├── system_tools.rs   # 系统信息、工作区管理
│   │   │       ├── task_tools.rs     # 任务 CRUD
│   │   │       ├── agent_tools.rs    # 子代理、技能加载、压缩、记忆
│   │   │       └── permission.rs     # 沙箱检查、权限审批
│   │   ├── lib.rs                # Tauri 入口（状态注册、命令绑定）
│   │   └── main.rs
│   ├── model_registry.json       # 模型能力注册表
│   ├── prompt/                   # 系统提示词模板
│   └── tauri.conf.json           # Tauri 配置
└── package.json
```

### 🔧 核心架构

#### Agent 循环

```
用户输入 → 意图分类 → 加载对应工具集 → Agent 循环（思考→工具调用→观察）→ 流式输出
                ↓
        ┌───────────────┐
        │ GENERAL_CHAT   │ → 无工具，直接回复
        │ PROJECT_ACTION │ → 完整工具集 + 子代理
        │ MEMORY_QUERY   │ → 记忆检索工具
        │ DANGEROUS_ACTION│ → 需用户确认
        └───────────────┘
```

#### 子代理委派机制

主代理作为高级编排者，将具体任务委派给子代理执行：

```
用户请求 → 主代理分析 → task_create 规划 → task 委派子代理 → 结果汇总
                                                    ↓
                                          干净上下文环境
                                          共享文件系统
                                          独立对话历史
```

子代理在独立的上下文中运行，拥有完整的文件和 Shell 工具，但不共享主对话历史，避免上下文污染。支持只读模式，限制子代理的写入能力。

#### 方案审批流程

复杂任务执行流程：

```
1. AI 提交方案 (propose_plan)
2. 前端弹出预览面板（Markdown 渲染）
3. 用户审阅并决策（同意/拒绝）
4. 同意后创建任务执行
```

#### 上下文管理

- **自动压缩** - 对话超过阈值时自动触发 micro_compact，保留近期上下文
- **手动压缩** - 通过 `compact` 工具主动清理
- **记忆归档** - `dream` 工具将碎片记忆提炼为结构化用户画像

### 🛠️ 内置工具一览

| 类别 | 工具 | 说明 |
|------|------|------|
| 文件 | `read_file` | 读取文件，支持行号范围读取 |
| 文件 | `read_file_skeleton` | 提取文件结构骨架（类/函数签名+行号） |
| 文件 | `write_file` | 写入文件 |
| 文件 | `edit_file` | 搜索替换修改文件片段 |
| 文件 | `list_directory` | 列出目录内容 |
| 文件 | `search_repo` | 全局关键词搜索（自动忽略编译产物） |
| Shell | `run_shell` | 执行 PowerShell 命令 |
| Shell | `background_run` | 后台执行长周期命令（如 dev server） |
| Shell | `check_background` | 检查后台任务状态 |
| Shell | `git_command` | 低风险 Git 操作（status/diff/log） |
| 系统 | `get_system_info` | 获取系统信息 |
| 系统 | `set_workspace` | 设置工作目录 |
| 任务 | `task_create` | 创建任务记录 |
| 任务 | `task_update` | 更新任务状态 |
| 任务 | `task_list` | 列出所有任务 |
| 任务 | `task_get` | 获取任务详情 |
| 任务 | `task_summary` | 生成任务全景报告 |
| 代理 | `task` | 委派子代理执行任务 |
| 代理 | `load_skill` | 加载专业技能知识 |
| 代理 | `compact` | 手动压缩上下文 |
| 代理 | `dream` | 触发记忆整理 |
| 代理 | `propose_plan` | 提交方案审批 |

### 🛡️ 安全特性

- **沙箱限制** - 会话可绑定工作目录，所有文件操作限制在沙箱内，路径遍历攻击自动拦截
- **权限审批** - 敏感操作（如 Shell 命令）需用户确认
- **循环检测** - Agent 循环超过 30 轮暂停等待确认，绝对上限 500 轮
- **危险操作拦截** - 自动识别并拦截潜在危险指令
- **Git 安全** - 仅允许只读 Git 操作，禁止修改历史或推送
- **路径安全** - 自动检测并拒绝包含 `..` 的路径遍历

### 📝 开发

```bash
# 开发模式（热更新）
pnpm tauri dev

# 类型检查
pnpm build

# 构建发布
pnpm tauri build
```

#### 数据存储

应用数据存储在运行目录下：

```
<运行目录>/
├── config.json          # 配置文件（多预设）
├── .jarvis_workspace    # 工作区路径记录
├── .sessions/           # 会话数据
├── .images/             # 图片缓存
├── .tasks/              # 任务数据
├── .logs/               # 运行日志
├── .plans/              # 方案记录
├── .transcripts/        # 对话转录
├── skills/              # 技能插件
├── global_memory.md     # 全局记忆
└── thoughts_and_plans.md # 思考记录
```

#### 扩展技能

在 `skills/` 目录下创建技能文件夹，添加 `SKILL.md` 文件：

```markdown
---
name: my-skill
description: 技能描述
---

技能的具体知识内容（Markdown 格式）
```

Agent 会通过 `load_skill` 工具按需加载技能知识。

### 🤝 贡献

欢迎提交 Issue 和 Pull Request！

1. Fork 本仓库
2. 创建特性分支 (`git checkout -b feature/amazing-feature`)
3. 提交更改 (`git commit -m 'Add amazing feature'`)
4. 推送到分支 (`git push origin feature/amazing-feature`)
5. 提交 Pull Request

### 📄 许可证

MIT License

---

## English

### ✨ Features

- 🤖 **Multi-Model Support** - 20+ major LLM models including DeepSeek, Claude, GPT, Gemini, Qwen, Doubao, MIMO
- 🧠 **Deep Thinking Mode** - Extended Thinking / Reasoning mode to show AI's reasoning process
- 🔄 **Smart Intent Recognition** - Automatically distinguishes between chat, project actions, memory queries, etc., loading tools on demand
- 📁 **Sandbox Working Directory** - Session-level directory restrictions for safe operations
- 🛠️ **Rich Toolset** - 20+ built-in tools: file read/write, Shell commands, Git operations, code search, task management
- 🤝 **Sub-agent Delegation** - Main agent orchestrates tasks; sub-agents execute in clean context without polluting the main conversation
- 📋 **Plan Approval Mechanism** - Complex tasks require plan submission and user confirmation
- 💾 **Session Persistence** - Auto-save conversation history with multi-session management
- 🧩 **Memory System** - Global and project memory for long-term user preference retention
- 🎨 **Modern UI** - IDE-style interface with frameless window, light/dark theme support
- 🔄 **Auto Retry** - Exponential backoff retry for API calls, automatic recovery from network issues
- ⏹️ **Mid-task Cancellation** - Interrupt running Agent tasks at any time
- 🖼️ **Multimodal** - Image input support with automatic compression

### 🏗️ Tech Stack

| Layer | Technology | Description |
|-------|-----------|-------------|
| Frontend | Vue 3 + TypeScript | Reactive UI, Composition API |
| Desktop | Tauri 2.0 | Rust backend, lightweight & performant |
| Backend | Rust + Tokio | Async runtime, SSE streaming |
| HTTP | Reqwest | Streaming API calls, OpenAI / Anthropic format support |
| Build | Vite | Fast HMR development experience |

### 🚀 Quick Start

#### Requirements

- Node.js >= 18
- Rust >= 1.70
- pnpm >= 8 (recommended) or npm

#### Install Dependencies

```bash
pnpm install
```

#### Development Mode

```bash
pnpm tauri dev
```

#### Build for Production

```bash
pnpm tauri build
```

### ⚙️ Configuration

On first run, click the settings button to configure:

1. **API Key** - Your LLM API key
2. **Base URL** - API endpoint address (auto-completes path)
3. **API Format** - Choose `openai` or `anthropic` format
4. **Main Model** - Model for main agent and sub-agents
5. **Utility Model** - Lightweight model for intent classification and memory management
6. **Deep Thinking** - Enable/disable thinking mode

Supports multiple profiles for different scenarios.

#### Supported Models

| Provider | Model | API Format | Thinking | Vision |
|----------|-------|------------|----------|--------|
| DeepSeek | V4 Pro / V4 Flash | openai | ✅ reasoning_effort | ❌ |
| DeepSeek | V3 Chat | openai | ❌ | ❌ |
| DeepSeek | R1 Reasoner | openai | Always reasoning | ❌ |
| Anthropic | Opus 4.5 / Sonnet 4.5 | anthropic | ✅ thinking | ✅ |
| Anthropic | 3.7 Sonnet | anthropic | ✅ thinking | ✅ |
| Anthropic | 3.5 Sonnet / Haiku | anthropic | ❌ | ✅ |
| OpenAI | GPT-4o / 4o mini | openai | ❌ | ✅ |
| OpenAI | GPT-4.1 / 4.1 mini | openai | ❌ | ❌ |
| OpenAI | o3 / o3-mini / o4-mini | openai | ✅ reasoning_effort | ❌ |
| Google | Gemini 2.5 Pro / Flash | openai | ✅ thinkingBudget | ✅ |
| Google | Gemini 2.0 Flash | openai | ❌ | ✅ |
| Alibaba | Qwen3-235B / 32B / 14B | openai | ✅ enable_thinking | ❌ |
| Alibaba | Qwen Plus / Turbo | openai | ❌ | ✅ |
| ByteDance | Doubao Seed 2.0 Pro / Lite | openai | ✅ thinking | ✅ |
| XiaoMi | MIMO V2 Flash | anthropic | ❌ | ❌ |

> 💡 Model capability registry is at `src-tauri/model_registry.json` — extend it with new models.

### 🔧 Core Architecture

#### Agent Loop

```
User Input → Intent Classification → Load Tool Set → Agent Loop (Think → Tool Call → Observe) → Stream Output
```

Intent types:
- `GENERAL_CHAT` — No tools, direct text response
- `PROJECT_ACTION` — Full toolset + sub-agent delegation
- `MEMORY_QUERY` — Memory retrieval tools
- `DANGEROUS_ACTION` — Requires user confirmation

#### Sub-agent Delegation

The main agent orchestrates tasks and delegates execution to sub-agents running in clean, isolated contexts:

```
User Request → Main Agent Analysis → task_create Planning → task Delegation → Result Aggregation
```

Sub-agents run in independent contexts with full file and Shell tools, but don't share the main conversation history. Read-only mode is available to restrict write capabilities.

#### Plan Approval Flow

```
1. AI submits plan (propose_plan)
2. Frontend shows preview panel (Markdown rendered)
3. User reviews and decides (approve/reject)
4. On approval, tasks are created and executed
```

### 🛠️ Built-in Tools

| Category | Tool | Description |
|----------|------|-------------|
| File | `read_file` | Read file with optional line range |
| File | `read_file_skeleton` | Extract file structure (classes/functions + line numbers) |
| File | `write_file` | Write file content |
| File | `edit_file` | Search-and-replace file editing |
| File | `list_directory` | List directory contents |
| File | `search_repo` | Global keyword search (auto-ignores build artifacts) |
| Shell | `run_shell` | Execute PowerShell commands |
| Shell | `background_run` | Run long-duration commands in background |
| Shell | `check_background` | Check background task status |
| Shell | `git_command` | Read-only Git operations (status/diff/log) |
| System | `get_system_info` | Get system information |
| System | `set_workspace` | Set working directory |
| Task | `task_create/update/list/get/summary` | Task board management |
| Agent | `task` | Delegate to sub-agent |
| Agent | `load_skill` | Load skill knowledge |
| Agent | `compact` | Manual context compression |
| Agent | `dream` | Trigger memory consolidation |
| Agent | `propose_plan` | Submit plan for approval |

### 🛡️ Security

- **Sandbox** - Session-bound working directory, all file operations restricted, path traversal blocked
- **Permission Approval** - Sensitive operations (e.g., Shell commands) require user confirmation
- **Loop Detection** - Agent pauses after 30 loops for confirmation, absolute limit of 500
- **Git Safety** - Read-only Git operations only, no history modification or push
- **Path Safety** - Auto-detect and reject `..` path traversal

### 📝 Development

```bash
# Development (hot reload)
pnpm tauri dev

# Type check
pnpm build

# Production build
pnpm tauri build
```

#### Data Storage

Application data is stored in the runtime directory:

```
<runtime-dir>/
├── config.json          # Configuration (multi-profile)
├── .jarvis_workspace    # Workspace path record
├── .sessions/           # Session data
├── .images/             # Image cache
├── .tasks/              # Task data
├── .logs/               # Runtime logs
├── .plans/              # Plan records
├── .transcripts/        # Conversation transcripts
├── skills/              # Skill plugins
├── global_memory.md     # Global memory
└── thoughts_and_plans.md # Thinking records
```

#### Extending Skills

Create a skill folder under `skills/` with a `SKILL.md` file:

```markdown
---
name: my-skill
description: Skill description
---

Specific knowledge content for the skill (Markdown format)
```

The Agent loads skill knowledge on demand via the `load_skill` tool.

### 🤝 Contributing

Issues and Pull Requests are welcome!

1. Fork this repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Submit a Pull Request

### 📄 License

MIT License

---

<div align="center">

Made with ❤️ by JarvisAgent Team

</div>

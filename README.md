# JarvisAgent

<div align="center">

**一个 AI 驱动的桌面端编程助手**

基于 Tauri 2.0 + Vue 3 + Rust 构建，完整 Agent 自主循环，支持 20+ 主流 LLM 模型，具备快照版本控制、多 Agent 沙箱、方案审批等企业级能力

</div>

---

## 中文

### ✨ 特性

- 🤖 **多模型支持** — DeepSeek、Claude、GPT、Gemini、Qwen、豆包、MIMO 等 20+ 主流 LLM
- 🧠 **深度思考模式** — Extended Thinking / Reasoning，展示 AI 推理过程
- 🔄 **完整 Agent 循环** — 意图分类 → 工具加载 → 上下文注入 → 自主决策执行 → SSE 流式输出
- 🧩 **统一 Provider 抽象** — `LlmProvider` trait 抹平 OpenAI / Anthropic API 差异，扩展新格式只需实现一个 trait
- 📸 **快照引擎** — 文件级树形版本控制，原子化回滚，分支管理，多 Agent 沙箱并行与合并
- 🏗️ **检查点系统** — 树形检查点记录 FileOperation 变更，支持分支切换与精确回滚
- 🤝 **子代理委派** — 主代理编排任务，子代理在干净上下文中独立执行，避免污染主对话
- 📋 **方案审批机制** — 复杂任务先提交方案，用户预览编辑后批准执行
- 💾 **会话持久化** — 自动保存对话历史，多会话管理，支持撤回上一轮消息
- 🧩 **记忆系统** — 全局记忆 + 项目记忆，长期保留用户偏好与项目上下文
- 🎨 **现代 UI** — 类 IDE 毛玻璃界面（Glassmorphism），无边框窗口，明暗主题切换
- 🔄 **增量 Markdown 渲染** — 30fps 节流，稳定内容缓存，仅尾部实时重渲染，大幅降低 CPU 占用
- ⏹️ **中途取消** — 随时中断正在执行的 Agent 任务
- 🖼️ **多模态理解** — 图片输入，自动压缩优化

### 🏗️ 技术栈

| 层级 | 技术 | 说明 |
|------|------|------|
| 前端框架 | Vue 3 + TypeScript | Composition API + `<script setup>` |
| 状态管理 | Pinia | 4 个 Store：session / chat / agent / permission |
| 桌面框架 | Tauri 2.0 | Rust 后端，轻量高性能 |
| 后端运行时 | Rust + Tokio | 异步运行时，SSE 流式处理 |
| HTTP 客户端 | Reqwest | 流式 API 调用，OpenAI / Anthropic 双格式 |
| Markdown | marked | GFM + 增量渲染 |
| 构建 | Vite 6 | 极速 HMR 开发体验 |

### 🚀 快速开始

#### 环境要求

- Node.js >= 18
- Rust >= 1.70
- pnpm >= 8

#### 安装与运行

```bash
pnpm install          # 安装前端依赖
pnpm tauri dev        # 开发模式（热更新）
pnpm tauri build      # 生产构建
cargo test            # Rust 测试（在 src-tauri/ 下运行）
```

### ⚙️ 配置

首次运行点击右上角设置按钮配置：

1. **API Key** — LLM API 密钥
2. **Base URL** — API 端点地址（自动补全路径）
3. **API Format** — `openai` 或 `anthropic` 格式
4. **主模型** — 主代理和子代理使用的模型
5. **工具模型** — 意图分类和记忆管理的轻量模型
6. **深度思考** — 开启/关闭 Extended Thinking

支持多预设（Profile）管理，不同场景快速切换。

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

> 💡 模型注册表位于 `src-tauri/model_registry.json`，可自行扩展。

### 📁 项目结构

```
JarvisAgent/
├── src/                              # Vue 3 前端
│   ├── main.ts                       # 应用入口
│   ├── App.vue                       # 根布局（TitleBar + Sidebar + ChatArea + AgentPanel）
│   ├── assets/global.css             # Glassmorphism 设计系统（亮/暗双主题）
│   ├── types/index.ts                # 全部 TypeScript 类型定义
│   ├── stores/                       # Pinia 状态管理
│   │   ├── session.ts                #   会话生命周期 + 消息缓冲区
│   │   ├── chat.ts                   #   核心交互：发送/取消/撤回/增量渲染
│   │   ├── agent.ts                  #   Agent/子代理运行状态追踪
│   │   └── permission.ts             #   权限请求 + 方案审批状态
│   ├── composables/
│   │   ├── useAgentEvents.ts         #   后端事件监听中枢 → 分发到各 Store
│   │   ├── useTheme.ts               #   亮/暗主题切换
│   │   └── useWindow.ts              #   Tauri 窗口控制
│   ├── utils/
│   │   ├── markdown.ts               #   Markdown 渲染、工具详情、token 用量
│   │   ├── html.ts                   #   HTML 转义
│   │   └── timeline.ts               #   时间格式化、文件操作图标
│   ├── services/
│   │   └── snapshotService.ts        #   快照 API 封装（三级缓存）
│   └── components/
│       ├── layout/                   # TitleBar, Sidebar
│       ├── chat/                     # ChatArea, TerminalInput, AgentPanel, MessageBubble, ThinkingStatus, WelcomeScreen
│       ├── common/                   # PermissionModal, PlanPreviewPanel, ConfirmModal, RollbackConfirmModal
│       ├── checkpoint/               # CheckpointTimeline（检查点时间线）
│       ├── snapshot/                 # SnapshotTimeline, DiffViewer, LivePreview
│       └── settings/                 # SettingsPanel（多预设管理）
├── src-tauri/                        # Rust 后端
│   ├── src/
│   │   ├── lib.rs                    # Tauri 入口：状态注册 + 30+ 命令绑定
│   │   ├── core/
│   │   │   ├── mod.rs                # 模块导出
│   │   │   ├── agent/
│   │   │   │   ├── pipeline.rs       # Agent 管线主循环
│   │   │   │   ├── stream.rs         # SSE 流式解析
│   │   │   │   ├── context.rs        # 动态上下文注入
│   │   │   │   └── tools_runner.rs   # 工具调用执行
│   │   │   ├── providers/            # LLM Provider 实现
│   │   │   │   ├── mod.rs
│   │   │   │   ├── anthropic.rs      # Anthropic Messages API
│   │   │   │   └── openai.rs         # OpenAI Chat Completions API
│   │   │   ├── traits.rs             # LlmProvider trait 抽象
│   │   │   ├── api_format.rs         # ApiFormat 枚举（auth header, version header）
│   │   │   ├── api_client.rs         # HTTP 客户端 + 重试
│   │   │   ├── error.rs              # AgentError 分层错误类型
│   │   │   ├── intent.rs             # 意图分类（GENERAL_CHAT / PROJECT_ACTION / MEMORY_QUERY / DANGEROUS_ACTION）
│   │   │   ├── tools/                # 工具实现
│   │   │   │   ├── mod.rs            #   工具注册 + 按需加载 + 路由分发
│   │   │   │   ├── file_tools.rs     #   文件读写、搜索、骨架提取
│   │   │   │   ├── shell_tools.rs    #   Shell 命令、后台任务
│   │   │   │   ├── system_tools.rs   #   系统信息
│   │   │   │   ├── task_tools.rs     #   任务 CRUD
│   │   │   │   ├── agent_tools.rs    #   子代理、技能加载、记忆
│   │   │   │   ├── tool_search.rs    #   代码搜索
│   │   │   │   └── permission.rs     #   沙箱检查、权限审批
│   │   │   ├── snapshot_engine/      # 快照引擎（树形版本控制）
│   │   │   ├── snapshot_manager/     # 快照管理器（会话级别注册）
│   │   │   ├── checkpoint.rs         # 检查点系统
│   │   │   ├── commands/             # Tauri 命令处理函数
│   │   │   ├── config.rs             # 配置管理
│   │   │   ├── memory.rs             # 上下文压缩与记忆管理
│   │   │   ├── registry.rs           # 模型能力注册表
│   │   │   └── state.rs              # 全局会话状态管理
│   │   └── main.rs
│   ├── model_registry.json           # 模型能力注册表
│   └── Cargo.toml
├── doc/
│   └── frontend-architecture.md      # 前端架构文档
├── CLAUDE.md                         # Claude Code 项目指南
└── package.json
```

### 🔧 核心架构

#### Agent 管线

```
用户输入 → 意图分类 → 按需加载工具集 → 动态上下文注入 → Agent 自主循环（思考→工具调用→观察）→ SSE 流式输出
                    ↓
            ┌──────────────────┐
            │ GENERAL_CHAT      │ → 无工具，直接回复
            │ PROJECT_ACTION    │ → 完整工具集 + 子代理
            │ MEMORY_QUERY      │ → 记忆检索
            │ DANGEROUS_ACTION  │ → 需用户确认
            └──────────────────┘
```

#### LLM Provider 抽象

通过 `LlmProvider` trait 统一不同 API 格式的差异：

```
chatStore.sendToJarvis()
  → invoke("ask_jarvis")
    → run_pipeline()
      → LlmProvider::stream_chat()    ← trait 多态
        ├── AnthropicProvider         ← Messages API, thinking blocks
        └── OpenAIProvider            ← Chat Completions API, reasoning_effort
```

`ApiFormat` 枚举自动提供各格式的认证头、版本头、SSE 解析逻辑，新增模型格式只需实现 trait。

#### 快照引擎

```
代码变更 → Patch（create/update/delete/rename）→ Snapshot（版本快照）
         → 树形分支管理 → 原子化回滚 → Journal 日志
         → 多 Agent 沙箱 → 分支合并（自动 + 手动冲突解决）
```

每次 AI 修改文件自动记录快照，支持随时回滚到任意历史版本，多 Agent 并行工作在独立分支上，完成后合并。

#### 子代理委派

```
用户请求 → 主代理分析规划 → task_create 拆分 → task 委派子代理
                                                    ↓
                                          干净上下文环境
                                          共享文件系统
                                          独立对话历史
                                          支持只读模式
```

#### 前端事件架构

```
Rust emit("jarvis-content/text") ──→ useAgentEvents.listen()
                                   ├──→ sessionStore（写入 buffer）
                                   ├──→ chatStore.triggerRender()（33ms 节流）
                                   ├──→ agentStore（更新运行状态）
                                   └──→ permissionStore（弹出确认弹窗）
```

所有后端事件集中在 `useAgentEvents.ts` 监听并分发到各 Pinia Store，渲染采用增量 Markdown 解析（稳定内容缓存 + 尾部实时重渲染，30fps）。

### 🛠️ 内置工具一览

| 类别 | 工具 | 说明 |
|------|------|------|
| 文件 | `read_file` | 读取文件，支持行号范围 |
| 文件 | `read_file_skeleton` | 提取代码骨架（类/函数签名 + 行号） |
| 文件 | `write_file` | 写入文件 |
| 文件 | `edit_file` | 搜索替换精确修改 |
| 文件 | `list_directory` | 列出目录内容 |
| Shell | `run_shell` | 执行 PowerShell 命令 |
| Shell | `background_run` | 后台执行长周期命令（如 dev server） |
| Shell | `check_background` | 检查后台任务状态 |
| Shell | `git_command` | 只读 Git 操作（status/diff/log） |
| 搜索 | `search_repo` | 全局关键词搜索 |
| 搜索 | `tool_search` | 高级代码搜索 |
| 系统 | `get_system_info` | 获取系统信息 |
| 系统 | `set_workspace` | 设置工作目录 |
| 任务 | `task_create / update / list / get / summary` | 任务看板 CRUD |
| 代理 | `task` | 委派子代理执行 |
| 代理 | `load_skill` | 加载专业技能知识 |
| 代理 | `compact` | 手动压缩上下文 |
| 代理 | `dream` | 触发记忆整理 |
| 代理 | `propose_plan` | 提交方案审批 |

### 🛡️ 安全特性

- **沙箱限制** — 会话绑定工作目录，路径遍历自动拦截
- **权限审批** — Shell 等敏感操作需用户确认
- **循环检测** — Agent 循环超 30 轮暂停确认，绝对上限 500 轮
- **Git 安全** — 仅允许只读操作，禁止修改历史或推送
- **检查点回滚** — 所有文件操作可追溯可撤销

### 📝 开发

```bash
pnpm tauri dev         # 开发模式（热更新）
pnpm build             # 前端类型检查
pnpm tauri build       # 生产构建
cargo test             # Rust 测试
```

#### 数据存储

```
<运行目录>/
├── config.json           # 配置（多预设）
├── .sessions/            # 会话数据 + 快照
├── .tasks/               # 任务数据
├── .checkpoints/         # 检查点记录
├── .snapshots/           # 快照数据
├── .agent_runs/          # Agent 运行记录
├── .logs/                # 运行日志
├── .plans/               # 方案记录
├── .transcripts/         # 对话转录
├── skills/               # 技能插件
└── global_memory.md      # 全局记忆
```

### 🙏 致谢

- **小米大模型团队** — 感谢小米模型的「创造者百万亿 Token 激励计划」与面向 Agent 框架团队的「Agent 生态共建计划」提供的小米模型 Standard Plan，助力本项目开发进度
- **[learn-claude-code-main](https://github.com/anthropics/claude-code)** — 通过该 GitHub 开源项目入门学习了 Claude Code 基础架构，本项目基于该开源项目的架构讲解思路实现

### 🤝 贡献

欢迎提交 Issue 和 Pull Request。

1. Fork 本仓库
2. 创建特性分支 (`git checkout -b feature/amazing-feature`)
3. 提交更改 (`git commit -m 'Add amazing feature'`)
4. 推送到分支 (`git push origin feature/amazing-feature`)
5. 提交 Pull Request

### 📄 许可证

MIT License


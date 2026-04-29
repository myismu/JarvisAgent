# 前端项目结构文档

本文档只描述 `E:\demo\JarvisAgent\src` 前端源码目录，面向前端开发者与 Agent，用于快速理解 Vue 端的目录分工、状态流转、组件关系与常见修改入口。

## 1. 前端技术栈

当前前端是 Tauri 桌面应用的 Vue 端，主要技术栈：

- Vue 3：组件与 Composition API。
- Pinia：全局状态管理。
- TypeScript：类型约束与前后端数据结构描述。
- Tauri API：通过 `invoke` 调用后端命令，通过 `listen` 接收后端事件。
- CSS 变量：在 `assets/global.css` 中维护全局主题、玻璃态、暗色模式、间距与动效变量。

## 2. `src/` 顶层结构

```text
src/
├── main.ts                      # Vue 应用入口，创建 App 并挂载 Pinia
├── App.vue                      # 前端主布局与全局弹窗挂载点
├── vite-env.d.ts                # Vite 类型声明
├── assets/                      # 静态资源与全局样式
├── components/                  # Vue 组件，按功能域拆分
├── composables/                 # 可复用组合式逻辑
├── stores/                      # Pinia 状态管理
├── services/                    # 前端服务封装，主要包装 Tauri invoke
├── types/                       # 前端共享类型定义
└── utils/                       # 纯工具函数与渲染辅助逻辑
```

## 3. 应用入口与主布局

```text
src/
├── main.ts
└── App.vue
```

### `main.ts`

职责：

- 创建 Vue 应用。
- 安装 Pinia。
- 引入全局样式 `assets/global.css`。
- 挂载到 `#app`。

### `App.vue`

职责：

- 组织整个桌面窗口的主布局。
- 初始化后端事件监听：`useAgentEvents().initListeners()`。
- 挂载全局级 UI：权限弹窗、计划预览面板、设置面板。
- 管理侧栏折叠状态、Agent 面板显隐状态、顶部运行状态灯。
- 通过 `usePreferences()` 持久化 UI 偏好。

主布局关系：

```text
App.vue
├── TitleBar                     # 顶部标题栏
├── Sidebar                      # 左侧会话/入口侧栏
├── ChatArea                     # 中间聊天展示区
├── TerminalInput                # 底部输入区
├── AgentPanel                   # 右侧 Agent 执行流程面板
├── PermissionModal              # 工具权限确认弹窗
├── PlanPreviewPanel             # 方案审批面板
└── SettingsPanel                # 设置面板
```

## 4. 全局样式与资源：`assets/`

```text
assets/
├── global.css                   # 全局样式、CSS 变量、主题、基础布局样式
└── vue.svg                      # Vue 默认资源，可按需清理或替换
```

`global.css` 是前端视觉系统的基础，通常包含：

- 背景色、文本色、边框色等 CSS 变量。
- 玻璃态效果变量。
- 圆角、阴影、过渡动画。
- 暗色模式相关样式。
- 全局重置与基础滚动条样式。

修改 UI 主题、整体字体、背景、暗色效果时优先查看此文件。

## 5. 组件目录：`components/`

```text
components/
├── layout/                      # 桌面窗口布局组件
├── chat/                        # 聊天、输入、Agent 执行展示组件
├── common/                      # 通用弹窗与确认类组件
├── settings/                    # 设置面板
├── snapshot/                    # 快照展示组件
└── checkpoint/                  # 检查点展示组件
```

### 5.1 布局组件：`components/layout/`

```text
layout/
├── TitleBar.vue                 # 自定义标题栏
└── Sidebar.vue                  # 左侧侧栏，会话入口与设置入口
```

职责说明：

- `TitleBar.vue`：负责顶部窗口标题区域，通常也承载窗口拖拽、标题信息、全局操作入口。
- `Sidebar.vue`：负责左侧导航/会话区域，并通过事件通知 `App.vue` 打开设置面板。

### 5.2 聊天与 Agent 展示：`components/chat/`

```text
chat/
├── ChatArea.vue                 # 聊天消息主区域
├── TerminalInput.vue            # 用户输入框与发送入口
├── MessageBubble.vue            # 单条消息气泡
├── AgentPanel.vue               # 右侧 Agent 执行流程总面板
├── AgentTurn.vue                # 单轮 Agent 回合展示
├── ExecutionPanel.vue           # 工具调用、执行日志、结果展示
├── ThinkingStatus.vue           # thinking/思考状态展示
└── WelcomeScreen.vue            # 无会话或初始状态欢迎页
```

核心关系：

```text
TerminalInput.vue
→ 调用后端 ask_jarvis 等命令
→ 后端推送事件
→ useAgentEvents.ts 更新 Pinia
→ ChatArea.vue / AgentPanel.vue 响应状态变化重新渲染
```

常见修改入口：

- 修改消息列表样式：看 `ChatArea.vue`、`MessageBubble.vue`。
- 修改用户输入体验：看 `TerminalInput.vue`。
- 修改 Agent 执行过程展示：看 `AgentPanel.vue`、`AgentTurn.vue`、`ExecutionPanel.vue`。
- 修改 thinking 展示：看 `ThinkingStatus.vue`。
- 修改空状态/欢迎页：看 `WelcomeScreen.vue`。

### 5.3 通用组件：`components/common/`

```text
common/
├── PermissionModal.vue          # 权限确认弹窗
├── PlanPreviewPanel.vue         # 计划/方案审批面板
├── ConfirmModal.vue             # 通用确认弹窗
└── RollbackConfirmModal.vue     # 回滚确认弹窗
```

职责说明：

- `PermissionModal.vue`：展示后端请求的工具权限，由用户批准或拒绝。
- `PlanPreviewPanel.vue`：展示 Agent 生成的计划文档，供用户审阅和确认。
- `ConfirmModal.vue`：通用确认交互组件。
- `RollbackConfirmModal.vue`：面向检查点/快照回滚的确认组件。

涉及危险操作、权限审批、计划审批、回滚确认时优先查看这里以及 `stores/permission.ts`。

### 5.4 设置组件：`components/settings/`

```text
settings/
└── SettingsPanel.vue            # 设置面板
```

职责说明：

- 展示和编辑用户配置。
- 通常会通过 Tauri `invoke` 读取或保存后端配置。
- 与 `stores` 或 composables 配合维护前端设置状态。

### 5.5 快照组件：`components/snapshot/`

```text
snapshot/
├── SnapshotTimeline.vue         # 快照时间线
├── DiffViewer.vue               # Diff 内容展示
└── LivePreview.vue              # 快照/变更实时预览
```

职责说明：

- 展示后端快照系统的数据。
- 支持查看文件变更、差异内容、快照时间线和实时预览。
- 与 `services/snapshotService.ts` 配合调用后端快照命令。

### 5.6 检查点组件：`components/checkpoint/`

```text
checkpoint/
└── CheckpointTimeline.vue       # 检查点时间线展示
```

职责说明：

- 展示会话检查点。
- 支持用户理解变更历史和回滚点。
- 回滚确认通常会和 `RollbackConfirmModal.vue` 配合。

## 6. 组合式逻辑：`composables/`

```text
composables/
├── useAgentEvents.ts            # 后端事件监听与分发中心
├── usePreferences.ts            # 前端 UI 偏好持久化
├── useTheme.ts                  # 主题切换/主题状态逻辑
└── useWindow.ts                 # Tauri 窗口相关操作封装
```

### `useAgentEvents.ts`

这是前端最重要的桥接层，职责：

- 使用 `@tauri-apps/api/event` 的 `listen` 监听后端事件。
- 使用 `@tauri-apps/api/core` 的 `invoke` 主动拉取后端状态。
- 将流式文本、thinking、工具调用、权限请求、计划文档、主 Agent run、子 Agent run 等事件分发到 Pinia。
- 处理 Vite HMR 下重复监听的问题，避免事件重复注册。

典型数据流：

```text
后端 emit 事件
→ useAgentEvents.ts 捕获 payload
→ 归一化字段
→ 更新 session/chat/agent/permission store
→ 组件自动响应并刷新 UI
```

修改后端事件 payload、事件名称、前端渲染行为时，必须检查此文件。

### `usePreferences.ts`

职责：

- 维护本地 UI 偏好。
- 例如侧栏折叠状态、Agent 面板显隐状态。
- 被 `App.vue` 使用，用于启动时恢复界面状态。

### `useTheme.ts`

职责：

- 维护主题相关逻辑。
- 通常与 `assets/global.css` 中的 CSS 变量配合。

### `useWindow.ts`

职责：

- 封装 Tauri 窗口控制逻辑。
- 适合放置最小化、最大化、关闭、拖拽等窗口行为相关函数。

## 7. 状态管理：`stores/`

```text
stores/
├── session.ts                   # 会话视图状态与流式缓冲
├── chat.ts                      # 聊天渲染控制与滚动行为
├── agent.ts                     # Agent/子 Agent/任务状态
└── permission.ts                # 权限请求与计划文档状态
```

### `session.ts`

职责：

- 维护当前会话 ID 与工作目录。
- 为每个会话维护独立 `SessionViewState`。
- 保存消息历史、流式内容缓冲、thinking 缓冲、工具缓冲、运行状态。
- 维护 Token 输入/输出统计。
- 判断当前会话或任意会话是否正在运行。

核心状态包括：

- `activeSessionId`
- `workingDirectory`
- `sessionViews`
- `currentSessionView`
- `currentSessionStatus`
- `isCurrentSessionRunning`
- `isAnySessionRunning`

### `chat.ts`

职责：

- 控制聊天区域渲染刷新。
- 管理滚动到底部等 UI 行为。
- 与 `ChatArea.vue` 配合，让流式输出期间的滚动和渲染更稳定。

### `agent.ts`

职责：

- 保存主 Agent run 与事件。
- 保存子 Agent run 与事件。
- 保存 Todo/任务列表。
- 控制右侧 Agent 面板显隐。
- 根据当前会话筛选主 Agent、子 Agent 和中断可恢复的 Agent run。

核心状态包括：

- `agentRuns`
- `agentRunEventsByRun`
- `subAgentRuns`
- `subAgentEventsByRun`
- `todos`
- `focusedTaskId`
- `showAgentPanel`

### `permission.ts`

职责：

- 保存后端发来的权限请求。
- 管理权限弹窗展示状态。
- 保存和展示计划文档、方案审批状态。
- 与 `PermissionModal.vue`、`PlanPreviewPanel.vue` 配合。

## 8. 服务封装：`services/`

```text
services/
└── snapshotService.ts           # 快照相关 invoke 调用封装
```

`services` 目录用于存放前端侧服务封装，当前主要是快照相关能力。

`SnapshotTimeline.vue`、`DiffViewer.vue`、`LivePreview.vue` 等快照组件如果需要请求后端快照数据，应优先通过 `snapshotService.ts`，避免在组件中散落重复的 `invoke` 逻辑。

## 9. 类型定义：`types/`

```text
types/
└── index.ts                     # 前端共享类型定义
```

职责：

- 定义前端组件、store、事件桥共享的数据结构。
- 描述后端事件 payload、会话、权限请求、计划文档、Agent run、子 Agent run、Todo 等类型。

开发约定：

- 新增后端事件时，优先在这里补充类型。
- 修改后端 payload 字段时，同步修改这里和 `useAgentEvents.ts`。
- 组件 props 或 store 状态复用的数据结构应尽量引用这里的类型，避免重复声明。

## 10. 工具函数：`utils/`

```text
utils/
├── agentTurnState.ts            # 单轮 Agent 状态更新
├── agentTurnRender.ts           # Agent 回合渲染辅助
├── historyRender.ts             # 会话历史渲染辅助
├── markdown.ts                  # Markdown 转 HTML/渲染辅助
├── timeline.ts                  # 时间线数据处理
└── html.ts                      # HTML 字符串处理辅助
```

### `agentTurnState.ts`

职责：

- 创建和重置当前 Agent 回合状态。
- 追加 Agent 文本、thinking、执行日志。
- 标记工具调用状态。
- 将后端事件应用到当前回合展示结构。

### `agentTurnRender.ts`

职责：

- 将 Agent 回合状态转换成适合组件展示的数据。
- 辅助 `AgentTurn.vue`、`ExecutionPanel.vue` 等组件渲染。

### `historyRender.ts`

职责：

- 处理会话历史消息的 HTML/展示结构。
- 用于恢复历史会话或追加历史消息。

### `markdown.ts`

职责：

- Markdown 渲染相关逻辑。
- 聊天消息、计划文档、Agent 输出中需要 Markdown 展示时优先查看。

### `timeline.ts`

职责：

- 时间线数据整理。
- 供快照或检查点时间线组件使用。

### `html.ts`

职责：

- HTML 字符串处理辅助。
- 用于消息渲染、转义或安全展示相关逻辑。

## 11. 前端核心数据流

### 11.1 用户发送消息

```text
TerminalInput.vue
→ Tauri invoke 调用后端命令
→ session store 标记运行状态/记录用户输入
→ 后端开始 Agent 流式执行
```

### 11.2 接收后端事件

```text
后端 emit
→ useAgentEvents.ts listen
→ 根据事件类型更新 store
→ ChatArea.vue / AgentPanel.vue / PermissionModal.vue 等组件响应更新
```

### 11.3 工具权限审批

```text
后端请求权限
→ useAgentEvents.ts 收到权限事件
→ permission.ts 保存请求
→ PermissionModal.vue 展示
→ 用户批准/拒绝
→ 前端 invoke 通知后端
```

### 11.4 计划审批

```text
后端生成计划文档
→ useAgentEvents.ts 收到计划事件
→ permission.ts 保存计划文档
→ PlanPreviewPanel.vue 展示
→ 用户审阅后继续或调整
```

### 11.5 快照/检查点展示

```text
SnapshotTimeline.vue / CheckpointTimeline.vue
→ snapshotService.ts 或直接 invoke
→ 后端返回快照/检查点数据
→ 组件展示时间线、Diff、预览或回滚确认
```

## 12. 常见修改入口速查

| 修改目标 | 优先查看文件 |
| --- | --- |
| 修改整体布局 | `App.vue` |
| 修改顶部标题栏 | `components/layout/TitleBar.vue` |
| 修改左侧栏 | `components/layout/Sidebar.vue` |
| 修改聊天列表 | `components/chat/ChatArea.vue`、`components/chat/MessageBubble.vue` |
| 修改输入框 | `components/chat/TerminalInput.vue` |
| 修改 Agent 执行面板 | `components/chat/AgentPanel.vue`、`components/chat/AgentTurn.vue`、`components/chat/ExecutionPanel.vue` |
| 修改 thinking 展示 | `components/chat/ThinkingStatus.vue` |
| 修改权限弹窗 | `components/common/PermissionModal.vue`、`stores/permission.ts` |
| 修改计划审批面板 | `components/common/PlanPreviewPanel.vue`、`stores/permission.ts` |
| 修改设置面板 | `components/settings/SettingsPanel.vue` |
| 修改快照展示 | `components/snapshot/`、`services/snapshotService.ts` |
| 修改检查点展示 | `components/checkpoint/CheckpointTimeline.vue` |
| 修改后端事件处理 | `composables/useAgentEvents.ts` |
| 修改会话状态 | `stores/session.ts` |
| 修改聊天滚动/刷新 | `stores/chat.ts` |
| 修改 Agent run/子 Agent 状态 | `stores/agent.ts` |
| 修改共享类型 | `types/index.ts` |
| 修改 Markdown 渲染 | `utils/markdown.ts` |
| 修改 Agent 回合状态 | `utils/agentTurnState.ts`、`utils/agentTurnRender.ts` |
| 修改全局样式/主题变量 | `assets/global.css` |

## 13. Agent 修改前检查清单

1. 先判断改动属于组件、store、composable、service、types 还是 utils。
2. 如果涉及后端事件，必须同时检查 `types/index.ts` 和 `composables/useAgentEvents.ts`。
3. 如果涉及会话运行状态或流式输出，优先检查 `stores/session.ts`。
4. 如果涉及 Agent 执行过程、子 Agent、Todo，优先检查 `stores/agent.ts`。
5. 如果涉及权限或计划审批，优先检查 `stores/permission.ts` 和 `components/common/`。
6. 如果涉及 UI 主题，不要在组件中硬编码颜色，优先使用 `assets/global.css` 中的变量。
7. 如果新增组件，应放到对应功能域目录，而不是直接堆在 `components/` 根目录。
8. 如果新增可复用逻辑，应优先放到 `composables/` 或 `utils/`，避免复制到多个组件。

## 14. 推荐阅读顺序

### 新开发者

1. `main.ts`：了解应用如何启动。
2. `App.vue`：了解页面主布局。
3. `stores/session.ts`：了解会话状态。
4. `composables/useAgentEvents.ts`：了解后端事件如何进入前端。
5. `components/chat/ChatArea.vue` 与 `components/chat/TerminalInput.vue`：了解聊天主流程。
6. `components/chat/AgentPanel.vue`：了解 Agent 执行过程展示。
7. `types/index.ts`：了解核心数据结构。

### Agent 执行前端任务时

1. 先定位影响的 UI 区域。
2. 再找到对应组件。
3. 检查该组件依赖的 store/composable/type。
4. 修改后确认是否需要同步更新类型、事件处理或全局样式。
5. UI 改动完成后应运行前端并实际查看主路径和边界状态。

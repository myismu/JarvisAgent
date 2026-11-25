# JarvisAgent v0.5.1

本次更新聚焦「跨端双向类型契约全量收口」「前后端大模块领域化治理」与「前端组件测试体系落地」，并补充若干实用交互与安全增强。

## 功能与体验改进

- **用户消息操作栏重构**：新增一键复制与原地编辑重发，对话修改与重跑更顺手
- **沙箱只读审批增强**：沙箱会话支持跨目录只读审批持久化，并进一步收紧只读白名单边界
- **真实缺陷修复**：在契约对齐与模块拆分过程中，顺手修复了 5 处前后端字段漂移（含 `DiffViewer`、`AgentPanel`、`TodoPanel`、`chat.ts`）以及 2 处组件卸载时事件监听未注销泄漏（`approval-mode-changed`、`bg-task-done`）

## 架构治理与工程质量

- **跨端双向强类型契约 100% 闭环**：通过 `ts-rs` 将全部 115 个 Tauri 命令与 36 类后端 `app.emit` 推送事件（新增 `infra/types/events.rs`，全仓动态 `json!` 事件清零）自动导出到 `src/types/generated/contracts.ts`，并由单测门禁锁死一致性
- **前端领域化拆解**：将 1162 行的 `useAgentEvents.ts` 按事件域拆为 6 个子模块；抽离 `TerminalInput`、`SettingsPanel`、`ChatArea`、`AgentPanel` 的核心状态与副作用到领域 `composables/`
- **后端大文件拆解与 Clippy 门禁收紧**：将 `schema.rs` 与 `repository.rs` 分别拆为领域目录（对外接口零破坏）；移除 `Cargo.toml` 中全局 `too_many_arguments = "allow"` 放行，阻断后续新增超长参数函数；全仓通过 `cargo fmt --all -- --check` 与 `cargo clippy --all-targets -- -D warnings`
- **前端组件测试体系落地**：引入 `@vue/test-utils` 与 `happy-dom`，补齐核心 Store、事件流与 5 个高频交互组件（`RollbackConfirmModal`、`PermissionModal`、`TerminalInput`、`PlanPreviewPanel`、`SettingsPanel`）的自动化测试，前端 Vitest 用例扩充至 16 个文件 / 79 个用例

## 安装

Windows 安装包见下方 Assets。更新由内置更新器自动检查。

# JarvisAgent v0.6.1

本次更新是面向 v0.6.0 后的全栈架构深水区治理与性能优化版本，聚焦稳定性止血、后端并发解耦、前端流式渲染提速、跨端强类型契约与结构化错误收口。

## 稳定性与并发性能治理

- **配置持久化加固**：将 `app-config.json` 读写下沉至基础设施层，引入进程内互斥锁、临时文件原子替换与 JSON 损坏自动备份（`.corrupt.<timestamp>`），杜绝多命令并发覆盖与异常静默清零。
- **SQLite 读写并发与阻塞隔离**：基于 WAL 模式引入轻量只读连接池，分离高频只读查询与写通道，并将历史分页、归档统计、冷会话初始化与快照 GC 下沉至 `spawn_blocking` 阻塞线程池；后台任务进程树查杀移出异步锁并优化为单次 PowerShell 快照遍历。
- **缓存防击穿与有界回收**：`SessionManager` 与 `SnapshotManagerRegistry` 全面补齐按会话粒度的双检锁（Double-Check Lock）及基于最近访问时间的空闲 LRU 回收机制（活跃执行或外部引用中的会话受严格保护），配合后台定期清理避免长时运行内存累积。
- **调试日志异步落盘**：将主循环调试日志写入改为有界异步通道后台线程，队列满时非阻塞背压计数，应用退出自动排空刷新；以零分配字节计数器替代请求体体积统计的冗余全量序列化。

## 跨端契约、引擎复用与前端渲染优化

- **流式 Markdown 增量渲染与界面流畅度提升**：重构 `StreamingMarkdown.vue` 为 `O(Δ)` 增量边界扫描 + `shallowRef` / `v-memo` 静态块缓存，大幅降低长文流式输出 CPU 开销；合并聊天区滚动调度至单一 `requestAnimationFrame`；对不可变历史消息与终态快照启用 `markRaw` 跳过深层代理；修复快速切会话时渲染节流锁死、监控窗口监听泄漏及过度订阅问题。
- **前端强类型 IPC 层与结构化 `CommandError`**：建立 `src/api/` 强类型命令/事件契约层，清零 `invoke<any>` 与事件 `on<any>`；后端新增带 `ts-rs` 导出的 `ErrorCode` 与 `CommandError`，将 会话 / 项目 / 配置 / 权限 / 回滚 / MCP 六大核心域共 77 个命令统一迁移并完成全端可读错误提取。
- **子代理引擎复用与架构守护**：抽离 `core/agent/subagent_runner.rs`，统一复用双协议请求体构建与主代理阶梯重试（1s/3s/5s）/ 429 `Retry-After` 限流策略；彻底切断 `infra` / `core` 对 `command` 的反向依赖并加入架构单测守卫。
- **组件解耦与测试矩阵扩充**：将 `Sidebar.vue`、`ChatArea.vue` 核心状态分别下沉至 `useSessionSidebar`、`useChatScroll` 与 `UserMessageBubble`，将 `AgentPanel.vue` 权限状态由 2s 轮询升级为纯事件驱动；清理无用前端依赖并隔离浏览器与构建配置 TypeScript 类型域；全端单测扩展至后端 756 个用例 + 前端 26 个测试文件 / 121 个用例。

## 安装

Windows 安装包见下方 Assets。更新由内置更新器自动检查。

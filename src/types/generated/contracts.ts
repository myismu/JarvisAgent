// 本文件由 Rust 端 ts-rs 自动生成，请勿手工修改。
// 若 Rust 结构体字段有变，请在 src-tauri/ 下运行：
//   UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date

export type ProjectMeta = { id: string, name: string, path: string, createdAt: number, updatedAt: number,
/**
 * 项目下的会话数
 */
sessionCount: number, };

export type SessionMeta = { id: string, title: string, createdAt: number, updatedAt: number, messageCount: number, isSmartNamed: boolean, profileId?: string | null, totalInputTokens: number, totalOutputTokens: number,
/**
 * 会话累计缓存命中 token（来自各次请求的 `prompt_cache_hit_tokens` 等字段）。
 *
 * 0 等价于"从未报告"——展示层据此显示 `--` 而不是 `0%`。
 */
totalCacheHitTokens: number,
/**
 * 会话累计缓存未命中 token。与命中数一样，0 表示未报告。
 *
 * 刻意**不**用 `total_input_tokens - total_cache_hit_tokens` 推导：
 * 各家的 `input_tokens` 口径不同（Anthropic 的 `input_tokens` 本身就不含缓存读取），
 * 相减会得出错误的未命中数。这里只忠实累加厂商报告的值。
 */
totalCacheMissTokens: number, titleSource: string,
/**
 * 所属项目 ID，None 表示无项目独立会话
 */
projectId?: string,
/**
 * 工作目录（从 projects 表派生，仅读）
 */
workingDirectory?: string,
/**
 * 深度思考档位（**会话级布尔**）：`true` = 本会话开启思考，`false` = 关闭。
 *
 * v14 起只有布尔，**没有"未表态"这个状态**——"跟随设置默认"在会话创建时
 * （`pipeline::start_run` 首次解析）就被固化成本列的值，此后设置页怎么改都不会
 * 回溯修改它。这是与 `profile_id`（随会话走的模型预设）对称的"随会话走的推理档"。
 */
thinkingMode: boolean,
/**
 * 归档时间（**毫秒**，Unix epoch）。`None` = 未归档。
 *
 * 列名 `deleted_at` 是「删除」时代的遗留，语义早已是归档：v27 起归档项目会把
 * 名下会话统一打上**同一个**时间戳（见 `repository::archive_project`），
 * 归档页据此展示「何时归档」。
 *
 * 刻意**不加** `skip_serializing_if`：前端要靠 `deletedAt: null` 与「字段缺失」区分
 * 「未归档」与「后端没给这个字段」，省略会让两者在前端混成一种状态。
 */
deletedAt?: number | null,
/**
 * 所属项目名（从 `projects.name` 派生，仅读）。
 *
 * 归档页要回答「这条会话原本属于哪个项目」——项目归档后已不在项目列表里，
 * 前端拿不到名字就没法分组展示。无项目（独立会话）时为 `None`。
 */
projectName?: string, };

export type BatchOpFailure = { id: string,
/**
 * 失败原因（面向用户的中文短句，直接展示）
 */
reason: string, };

export type BatchOpResult = { succeededIds: Array<string>, failed: Array<BatchOpFailure>, restoredProjectIds: Array<string>, };

export type UsageSource = "main" | "subagent" | "reflection" | "compact" | "session_name" | "memory_curator";

export type UsageSourceAuditRow = { source: UsageSource, requestCount: number, inputTokens: number, outputTokens: number,
/**
 * 已上报的命中/未命中之和（未上报的请求不参与，也不按 0 计入）
 */
cacheHitTokens: number, cacheMissTokens: number,
/**
 * 缓存字段未上报的请求数
 */
unreportedRequests: number,
/**
 * 这些未上报请求对应的输入 token 合计（让“不知道”也有量级）
 */
unreportedInputTokens: number, };

export type UsageBreakdown = { sessionId: string, inputTokens: number, outputTokens: number, cacheHitTokens: number, cacheMissTokens: number,
/**
 * 明细行数（= 请求次数）
 */
requestCount: number, unreportedRequests: number, unreportedInputTokens: number,
/**
 * 是否存在明细（`false` = 历史会话，数字不可信）
 */
hasDetail: boolean, sources: Array<UsageSourceAuditRow>, };

export type UsageSourceView = {
/**
 * 累计输入 token
 */
input: number,
/**
 * 累计输出 token
 */
output: number,
/**
 * 累计缓存命中 token。0 有两种可能：真的没命中、厂商没报（后者看 `cache_reported`）
 */
cacheHit: number,
/**
 * 累计缓存未命中 token
 */
cacheMiss: number,
/**
 * 累计请求次数（= 调用模型接口的次数，不是 agent loop 轮数）
 */
requestCount: number,
/**
 * 其中**上报了缓存字段**的请求数。
 *
 * 推导关系：`cache_reported = request_count - unreported_requests`。
 * 两者是同一批请求的两种数法，不是独立读数——同一行的请求要么报了缓存、
 * 要么没报，没有第三种。因此不另加一列 SQL（另加 = 多一个可能与差值打架的口径）。
 * 与 `request_count` 的差值即「厂商没给缓存字段」的请求量，用来解释
 * 某个来源为什么算不出命中率，而不是把未上报直接画成 0%。
 */
cacheReported: number, };

export type SessionUsageBreakdownView = {
/**
 * 主 Agent 主循环
 */
main: UsageSourceView,
/**
 * 子 Agent（RunSubagent / 任务图调度出去的执行体）
 */
subagent: UsageSourceView,
/**
 * 系统内部调用（反思、压缩、自动命名、记忆整理）
 */
system: UsageSourceView,
/**
 * 缓存字段未上报的请求数（整会话口径）
 */
unreportedRequestCount: number,
/**
 * 这些未上报请求对应的输入 token 合计（让"不知道"也有量级）
 */
unreportedInput: number,
/**
 * 是否存在明细。`false` = 无明细行（历史会话），三层全零**不可当真实分布展示**
 */
hasDetail: boolean, };

export type SessionUsageView = { sessionId: string, inputTokens: number, outputTokens: number, cacheHitTokens: number, cacheMissTokens: number,
/**
 * 按来源分层的明细。`has_detail = false` 时三层全零，只有上面的总计有意义
 */
breakdown: SessionUsageBreakdownView, };

export type SessionThinkingSnapshot = { sessionId: string, thinkingEnabled: boolean, resolvedEnabled: boolean, reason: string, noticeI18nKey?: string | null, };

export type ShellMode = "auto" | "powershell";

export type ShellSettings = { mode: ShellMode,
/**
 * 用户显式指定的 `bash.exe` 路径。留空表示自动探测。
 *
 * 为什么要可配：Git for Windows 允许装到任意目录（本机就在 `E:\StudyTools\Git`），
 * 硬编码 `C:\Program Files\Git` 在真实环境里必然失效。
 */
gitBashPath: string | null, };

export type ShellResolution = {
/**
 * 当前平台是否支持切换（只有 Windows 有自动 / PowerShell 两种选择）
 */
platformSupported: boolean,
/**
 * 用户选择的模式：`auto` / `powershell`
 */
mode: ShellMode,
/**
 * 用户显式配置的 bash 路径（可能为空）
 */
gitBashPath: string | null,
/**
 * 实际探测到的 bash.exe 路径（探测失败为 None）
 */
detectedBashPath: string | null,
/**
 * 需要提示给用户的问题（例如手填路径不存在）。正常状态为 None。
 */
error: string | null, };

export type AgentConfig = {
/**
 * API 格式 (anthropic 或 openai)
 */
apiFormat: string,
/**
 * API 密钥
 */
apiKey: string,
/**
 * API 基础 URL
 */
baseUrl: string,
/**
 * 主对话模型 ID（主代理 + 子代理共用此模型）
 */
mainModel: string,
/**
 * 意图分类器 / 记忆 Agent 使用的模型 ID（可用更便宜的模型）
 */
utilityModel: string,
/**
 * 模型生成的温度参数
 */
temperature?: number | null,
/**
 * 模型生成的 Top P 参数
 */
topP?: number | null,
/**
 * 模型生成的 Top K 参数
 */
topK?: number | null,
/**
 * Reflection 模式: "always" | "smart" | "off"
 */
reflectionMode: string,
/**
 * 用户自定义 max_tokens（覆盖模型注册表默认值），None 表示使用注册表值
 */
maxTokens?: number | null, };

export type ModelProfile = { id: string, name: string, config: AgentConfig, };

export type AppConfig = { activeProfileId: string, globalProfileId: string, profiles: Array<ModelProfile>, };

export type ImageCompressConfig = { maxWidth: number, maxHeight: number, quality: number, };

export type CustomWindowState = { width: number, height: number, x: number, y: number, maximized: boolean, fullscreen: boolean, decorated: boolean, updatedAt: number, };

export type UiPreferences = { fontSize: number, codeFontSize: number, autoScroll: boolean, defaultExpandThinking: boolean, agentPanelPosition: string,
/**
 * 【已退役·保留字段不删】紧凑模式已从前端整体拆除。
 *
 * 1) 前端不再发送该字段：设置项开关、状态、文案已全部删除，`save_ui_preferences`
 *    全量替换时这里只会回落为默认值 `false`；
 * 2) 保留字段是为了老 app-config.json 能照常反序列化——结构体缺字段会直接报错，
 *    而 `#[serde(default)]` 已兜住新配置里的缺省，二者缺一不可；
 * 3) 与上面的 `code_font_size` 同一处理方式：前端不再发送、后端仍保留字段，
 *    只为兼容历史配置文件。后端逻辑不读它（只负责透传存储），
 *    可随下一次配置结构迁移一并删除。
 */
compactMode: boolean, sidebarCollapsed: boolean,
/**
 * 侧边栏「项目」总区块是否折叠（一次收起所有项目分组）。
 *
 * 与 `sidebar_collapsed`（整个侧边栏收起）是两个不同层次：本字段只管
 * 会话列表里的项目分组。这组状态早期存在前端 localStorage，换机器或
 * 清缓存就丢；现已并入 UI 偏好统一落库。
 */
sidebarProjectsCollapsed: boolean,
/**
 * 侧边栏「独立会话」区块是否折叠
 */
sidebarStandaloneCollapsed: boolean,
/**
 * 被折叠的项目分组 id 列表。
 *
 * 只存「折叠的」：默认展开，新增项目自动可见。用 id 而非名称——
 * 项目可重命名，id 才是稳定标识。
 */
sidebarCollapsedProjectIds: Array<string>, agentPanelVisible: boolean, agentAudience: string, agentWorkMode: string,
/**
 * 权限档位："request_approval"（请求审批，默认）/ "auto_approve"（帮我批准）
 */
agentApprovalMode: string,
/**
 * 深度思考默认档位（**设置态，三值**）：`"follow_global"`（跟随全局）/ `"on"` / `"off"`。
 *
 * 语义（2026-09-19 重构后）：
 * - `follow_global`：**强制思考模型 → 开，非强制思考模型 → 关**（判据是模型能力
 *   `thinking_forced`，与 `agent_audience` 已解耦）；
 * - `on` / `off`：无条件开关。
 *
 * 注意：它**只在新建会话时被解析一次**并固化进 `sessions.thinking_mode`（布尔），
 * 此后修改本字段**不会影响任何已有会话**——这是与"工作模式/权限档位"完全一致的
 * 三值分治语义（设置态只喂新会话）。
 *
 * 该字段原先挂在 `profiles[].config.thinkingDefault`（模型预设级），但裁决层每轮
 * 现读的是 `active_config()`——即"当前激活预设"，于是改任一副预设都会倒灌所有
 * 会话。本次重构把它提到全局设置层，与权限/工作模式并列，并改为只在建会话时生效。
 */
thinkingDefault: string, locale: string,
/**
 * 「崩溃保护（实时保存）」开关——**纯全局、默认关**。
 *
 * 开：主 Agent 流式期间按攒批粒度（200ms / 1KB）把半截正文写进
 * `agent_run_events`，进程崩溃/断电后能把"跑到一半的那一轮"恢复出来。
 * 关：只在每轮收尾写一次——此时进程崩溃会丢掉**当前这一轮**已输出的内容
 *（用户取消、上游断流不受影响，它们进程还活着，走内存态收尾）。
 *
 * 为什么默认关：正常路径下"崩溃丢半轮"是可接受的（用户重说一句即可），
 * 而开启意味着给每个 SSE 帧加一条 SQLite 写的成本；把选择权交给用户，
 * 默认保持最省的状态。
 *
 * 为什么是**全局**而非会话级：它防的是"进程突然没了"这种应用级故障，
 * 与具体会话无关；做成会话级只会让用户在多会话间反复拨动却无收益。
 */
crashProtection: boolean,
/**
 * 图片压缩档位："eco"（省流）/ "standard"（标准）/ "hd"（高清）。
 *
 * 档位是**用户的选择意图**，宽高数值是它的实现细节，两者一起落库：
 * - 档位决定界面高亮哪一格，也决定了将来调整某档参数时该档用户跟随新参数；
 * - 数值是压缩算法的直接输入，`TerminalInput.vue` 不需要再查表换算。
 *
 * 两者**必须同时写入**（前端 watcher 负责同步），只写其一会造成"显示档位与
 * 实际压缩参数不一致"。`normalize_image_tier()` 负责在读取时自愈。
 */
imageCompressTier: string,
/**
 * 图片压缩最大宽度（像素）。前端「省流/标准/高清」三档的数值形态，默认标准档 1568。
 */
imageMaxWidth: number,
/**
 * 图片压缩最大高度（像素）
 */
imageMaxHeight: number,
/**
 * 图片压缩质量 (0.0 ~ 1.0)。
 *
 * 刻意不暴露到界面：JPEG 质量只影响体积和肉眼看感，**不影响 token 数**
 * （token 只由宽高决定），做成档位只会增加用户负担。
 */
imageQuality: number,
/**
 * 未发送的输入草稿：`sessionId → 原文`。
 *
 * 草稿是输入框的**编辑态**，不是会话的业务数据，因此不放进 `sessions` 表：
 * 那张表是会话列表查询的主表，加长 TEXT 字段会拖累每次列表查询的行宽；
 * 另外它也不需要按键级写库——只在切会话和发送完成时各写一次。
 *
 * 键用 `__new__` 表示「尚未落库的新会话」（`activeSessionId` 为 `None`）。
 * 与 `sidebarCollapsedProjectIds` 同理，**整体替换**写入而不是原地改：偏好对象
 * 是快照式落库的，原地改不利于前端 `watch` 比对。
 */
inputDrafts: { [key in string]?: string },
/**
 * 向后兼容：读取旧 agent_display_mode 字段
 */
agentDisplayMode: string | null, agentMessageOpacity: number, userMessageOpacity: number, };

export type PromptMeta = { path: string, displayName: string, description: string,
/**
 * UI 分组："base" / "audience" / "mode" / "os" / "subagent"
 */
category: string,
/**
 * true = 进 system（新会话生效）；false = 进动态上下文（下一轮生效）
 */
goesIntoSystem: boolean,
/**
 * true = 可选文件：没有出厂默认，未配置时为空、不会被注入
 */
optional: boolean,
/**
 * 磁盘上是否有用户自定义版
 */
customized: boolean,
/**
 * 当前生效版（磁盘版或内置版）的字符数
 */
sizeChars: number,
/**
 * 仅 os 分类有意义：是否为当前编译平台（UI 标注"本机"）
 */
isPlatformActive: boolean, };

export type PromptDetail = { path: string, displayName: string, description: string, goesIntoSystem: boolean,
/**
 * true = 可选文件：没有出厂默认，未配置时为空
 */
optional: boolean,
/**
 * 当前生效版（磁盘有则磁盘版，否则内置版）——编辑器初始内容
 */
currentContent: string,
/**
 * 内置出厂版原文（"恢复默认"的预览参照）
 */
embeddedContent: string, customized: boolean, };

export type ToolMeta = { name: string,
/**
 * 一句话简述（`ToolDef.description`），列表上直接显示
 */
description: string, category: string,
/**
 * true = 按需工具（需 GetToolCatalog → DiscoverTools 三步才能用）。
 * UI 据此分组：核心工具常驻 schema，按需工具按需发现。
 */
deferred: boolean,
/**
 * 是否启用。未在配置里出现过的工具默认启用。
 */
enabled: boolean,
/**
 * 完整 JSON Schema（含参数名 / 类型 / 必填 / 参数说明），供界面上"展开看详情"。
 *
 * 直接透传注册表里那份 —— 它就是模型实际收到的东西，所以界面上看到的和模型
 * 看到的一定一致，不会出现"文档说一套、模型收另一套"。
 */
schema: Record<string, unknown>, };

export type SkillMeta = { name: string, description: string, path: string, bodyTokens: number, active: boolean, };

export type SkillDetail = { name: string, description: string, path: string, body: string, };

export type ModelCapabilities = {
/**
 * 是否支持流式输出
 */
streaming: boolean,
/**
 * 是否支持深度思考模式（可通过参数控制）
 */
thinking: boolean,
/**
 * 控制思考的参数名。取值与 `apply_thinking_for_model()` 的分支一一对应：
 *
 * - `"reasoning_effort"` —— 顶层 `reasoning_effort`（OpenAI / Kimi / 小米旧写法）
 * - `"thinking"`         —— `{type, budget_tokens}`（DeepSeek、Anthropic 老形态）
 * - `"thinking_type"`    —— **只发** `{type}`，不带 `budget_tokens`
 *   （智谱 GLM-5.x、小米 MiMo-V2.5：官方只定义 type）
 * - `"thinkingBudget"`   —— Gemini 2.x 的 `thinking_budget` 整数
 * - `"enable_thinking"`  —— 顶层布尔
 * - `"extra_thinking"` / `"extra_enable_thinking"` —— `extra_body` 里的布尔
 * - `"extra_chain_of_thought"` —— `extra_body:{chain_of_thought}`
 * - `"thinking_enable"`  —— `{enable}`（腾讯混元）
 * - `"enable_thought"`   —— `parameters:{enable_thought}`（MiniMax）
 */
thinkingParam?: string,
/**
 * 是否支持温度参数（部分推理模型开启思考后不可调）
 */
temperature: boolean,
/**
 * 是否支持视觉/多模态
 */
vision: boolean,
/**
 * 最大输出 token 数
 */
maxTokens: number,
/**
 * 最大上下文窗口 token 数
 */
maxContextTokens?: number,
/**
 * 备注说明
 */
notes: string,
/**
 * 是否强制开启思考（用户不可关闭，适用于 thinking + tool_calls 必须共存的模型如 DeepSeek）
 */
thinkingForced: boolean,
/**
 * 缓存命中字段的写法（**可选覆盖**，省略 = 运行时按候选表自动探测）：
 * `"deepseek" | "nested" | "flat" | "anthropic" | "none"`
 */
cacheUsageStyle?: string,
/**
 * 该模型支持的「思考深度档位」（可选，空 = 不声明）。
 *
 * 两条出口共用同一份档位声明，各自映射到自己的参数名：
 * - OpenAI 出口 → 顶层 `reasoning_effort`
 * - Anthropic 出口 → `output_config.effort`
 *
 * 有了它，"关思考"才能在真正支持 `none` 的模型上写成 `none`；没有声明的
 * 模型维持原行为（省略参数），不会引入新风险。
 */
thinkingEffortValues: Array<string>,
/**
 * **仅 Anthropic 出口**：该模型接受哪种思考形态（省略 = `"budget"`）。
 *
 * - `"budget"`            —— 老形态 `{type: enabled|disabled, budget_tokens}`
 * - `"adaptive_optional"` —— 省略即不思考；开启只能传 `{type:"adaptive"}`，关闭传 `disabled`
 * - `"adaptive_default"`  —— **省略即思考**；关闭需显式 `{type:"disabled"}`
 * - `"adaptive_only"`     —— 恒开、**不可关闭**（传 `disabled` 会 400），只能省略或传 `adaptive`
 */
thinkingMode?: string,
/**
 * 模型生命周期状态（**可选，省略 = `"active"`**）。
 *
 * - `"active"`       —— 官方在售，正常可用（省略字段即此值）
 * - `"deprecated"`   —— 官方已公告即将下线，**当前仍可调用**（只告警，不拦）
 * - `"retired"`      —— 官方已宣布退役/关停，请求必然失败（预设层禁止保存）
 * - `"unverifiable"` —— 官方当前目录查不到该 ID，但**无退役公告**
 * - `"alias"`        —— 旧名，请求被路由到新模型，仍可用
 *
 * **`unverifiable` 不等于 `retired`。** 前者只是"我们查不到"，不能据此
 * 禁止用户使用——这与前端 `utils/thinking.ts` 立的「未知 ≠ 不支持」是同一条
 * 原则：把检索失败当成事实结论，会把"我们没查到"变成"用户不能用"。
 */
status: string,
/**
 * 状态补充说明（**只放事实**：何时退役、依据哪份官方公告）。
 *
 * 刻意**不记录"建议换成哪个型号"**：建议会被时间淘汰——被建议的型号自己
 * 以后也可能退役，届时这条提示就从"有用"变成"误导"，还得持续维护。
 * 界面统一引导用户去厂商官方文档查最新型号。
 */
statusNote?: string, };

export type ModelRegistryEntry = {
/**
 * 模型 ID（用于精确或模糊匹配）
 */
id: string,
/**
 * 服务商名称
 */
provider: string,
/**
 * 用户友好的显示名称
 */
displayName: string,
/**
 * 推荐的 API 格式
 */
apiFormat: string,
/**
 * 能力描述
 */
capabilities: ModelCapabilities, };

export type ResolvePermissionResult = { needsResume: boolean, planTasksCreated?: boolean, resumeWith?: string, };

export type PendingPermissionItem = { id: string, message: string, allowSession: boolean, kind: string, warning?: string | null, };

export type SessionAllowance = { kind: string, scope: string, label: string, };

export type PermissionStateView = { allowanceCount: number, allowances: Array<SessionAllowance>, pendingCount: number, pending: Array<PendingPermissionItem>, };

export type SessionPermissionSettingsView = { approvalMode: string, readOnly: boolean, allowances: Array<SessionAllowance>, };

export type PermissionResolvedPayload = { id: string, sessionId: string, decision: string, decisionText: string, };

export type JarvisResult = { status: string, content: string, input_tokens: number, output_tokens: number, session_input_tokens: number, session_output_tokens: number,
/**
 * 会话累计缓存命中 / 未命中 token。
 *
 * 两者都为 0 表示该会话从未有请求上报缓存字段（前端据此显示 `--`，而不是 `0%`）。
 */
session_cache_hit_tokens: number, session_cache_miss_tokens: number,
/**
 * 后端为用户消息分配的 UUID，前端用于关联撤回按钮
 */
user_message_id?: string,
/**
 * 本轮创建的文件检查点 id（有文件编辑的轮次由 finalize 收尾时创建）。
 *
 * 与 `user_message_id` 同一设计动机：发消息那一刻检查点尚不存在，
 * 靠返回值让前端实时回填撤回信息（「会话和代码撤回」立即出现），
 * 不必等刷新走 `get_session_messages` 重查库。
 */
checkpoint_id?: string,
/**
 * 本轮是否有文件编辑补丁（与刷新路径 `rollback_info.has_file_edits` 同口径；
 * 检查点创建失败时它仍为 true，前端据此展示 both 菜单）
 */
checkpoint_has_patches?: boolean,
/**
 * break_loop 时的工具执行结果摘要（前端用于 toolBuffer，避免丢失工具执行日志）
 */
tool_execution_summary?: string,
/**
 * 本轮状态标注的 **i18n key**（前端 `t(key)` 后渲染在气泡下方的小字里）。
 *
 * ## 为什么是 key 而不是文案
 *
 * 这里曾经是 `notice: Option<String>`，后端**直接写中文句子**
 * （"用户已取消执行…" / "上游长时间未返回数据…"）。后果：界面语言切成英文后，
 * 用户一取消生成、一遇到超时就冒出中文小字 —— 后端产出的用户可见文案
 * **绕开了前端那套语言包**。现在后端只说"发生了什么"（key），
 * 文案由前端出；与 `thinking_notice_i18n_key` 同一口径。
 *
 * ⚠️ 用词口径（写文案时仍受此约束）：只描述**可观测事实**（等待时长/用户动作），
 * 不断言服务端状态——"上游服务已停止响应/已失联"这类措辞被明确否决
 * （静默原因在链路上不可区分）。另注意区分两套机制：流空闲超时（stream.rs）
 * 与 plan 模式「规划看门狗」（pipeline.rs::update_plan_watchdog）不是一回事。
 *
 * 与 `content` 的分工：content 是模型正文（渲染在回复气泡内），
 * notice 是运行状态说明（渲染在气泡**下方**的小字里）。
 * 中断类信息走 notice，避免混进正文看起来像模型自己说的话。
 */
notice_i18n_key?: string,
/**
 * 状态标注的补充数据（可选，**原样展示、不翻译**）。
 *
 * 只有一种用法：句子里需要塞入无法翻译的内容时（如具体的错误文本）。
 * 前端以 `notice.detail` 这个 key 包一层再拼到主句后面。
 */
notice_detail?: string,
/**
 * 本轮最终采用的深度思考状态（由 `core::session::thinking::decide` 裁决）。
 *
 * 前端用它渲染开关状态与"被模型强制夹紧"的提示，**不再自行判断**。
 */
thinking_enabled?: boolean,
/**
 * 裁决原因（`ThinkingReason` 的 Debug 名，如 `ClampedByForced`）
 */
thinking_reason?: string,
/**
 * 需要提示用户时的 i18n key（前端 `t(key)` 后展示）
 */
thinking_notice_i18n_key?: string, };

export type ContextSectionSnapshot = { key: string, label: string, chars: number, estimatedTokens: number, tokenCountMethod: string, itemCount: number, content: string, truncated: boolean,
/**
 * 原始 JSON 数据（仅 messages 和 tools section 有值）
 */
rawContent?: string, };

export type CacheHitPoint = { loopCount: number, hitTokens: number, missTokens: number,
/**
 * 命中的字段名（服务商未报告时为 None）
 */
source?: string, };

export type SessionContextSnapshot = { sessionId: string, runId?: string | null, loopCount: number, model: string, intent: string, apiFormat: string, createdAt: number, totalChars: number, estimatedTokens: number, providerInputTokens?: number | null, providerOutputTokens?: number | null, providerTotalTokens?: number | null,
/**
 * 缓存命中 / 未命中的输入 token。
 * `None` = 该 provider（或中转链路）未报告该字段，**与 0 命中是两回事**。
 */
cacheHitTokens?: number | null, cacheMissTokens?: number | null,
/**
 * 命中的字段名（如 `prompt_cache_hit_tokens` / `cached_tokens` / `cache_read_input_tokens`）
 */
cacheSource?: string | null,
/**
 * 本会话逐 loop 的缓存命中记录（保留最近 N 条，用于渲染趋势）
 */
cacheHistory: Array<CacheHitPoint>, driftPercent?: number | null, maxContextTokens?: number | null, maxOutputTokens: number, messageCount: number, toolSchemaCount: number, toolCallCount: number, toolResultCount: number,
/**
 * 图片块数量（Image block 数）。
 *
 * 与 `tool_call_count` / `tool_result_count` 同族：都是"这类内容块出现了几次"的
 * 顶层统计字段。它此前以 `attachments` 分区的形式存在，但那个分区的字符数/token 数
 * 与 `messages` 分区重复计入（图片块本来就在 messages 的 JSON 里），导致总量虚高；
 * 而"块数"也不是可加和的尺寸量。故收敛为顶层计数，分区只保留
 * system / messages / tools 三项（三者之和即总量）。
 *
 * `#[serde(default)]` 是硬要求：旧快照 JSON 里没有这个键，缺 default 会让
 * `repository::get_context_snapshot` 整条反序列化失败，监控面板直接空白。
 */
imageCount: number, sections: Array<ContextSectionSnapshot>, };

export type InterruptKind = "stream_timeout" | "user_cancel" | "pipeline_error" | "app_closed" | "loop_limit" | "plan_limit";

export type HistoryTurnTokens = { input: number, output: number, };

export type HistoryTextBlock = { id: string, loop: number, kind: string, content: string, status: string, timestamp: number, };

export type HistoryThinkingBlock = { id: string, loop: number, content: string, status: string, timestamp: number, };

export type HistoryToolCallView = { id: string, loop: number, name: string, status: string, input?: string, output?: string, error?: string, logs: Array<string>, timestamp: number, updatedAt: number, };

export type HistoryExecutionLog = { id: string, loop: number, content: string, timestamp: number, };

export type HistoryTurnSnapshot = { version: number, status: string, textBlocks: Array<HistoryTextBlock>, thinkingBlocks: Array<HistoryThinkingBlock>, toolCalls: Array<HistoryToolCallView>, logs: Array<HistoryExecutionLog>, tokens?: HistoryTurnTokens | null,
/**
 * 气泡下方的小字说明 —— 只给 **i18n key**，文案由前端出。
 *
 * 与 `text_blocks` 的区别是展示位置：notice 渲染在回复气泡**之外**的下方，
 * 属于状态标注；而 text_blocks 是模型正文，渲染在气泡内。
 * 中断/取消这类"运行状态"信息一律走 notice，避免混进正文显得突兀。
 *
 * 注意：这里曾经是 `notice: Option<String>`、装后端拼好的**中文句子** ——
 * 界面切英文后中断轮次仍冒中文。现在后端只发 key（如 `notice.userCancel`），
 * 文案统一由前端语言包提供，与运行期 `JarvisResult::notice_i18n_key` 同源。
 */
noticeI18nKey?: string,
/**
 * 本轮**净执行用时**（毫秒）：run 的 `started_at → finished_at`，再扣掉等待用户决策的时间。
 *
 * `None` = 拿不到可信口径（旧数据没有 run 关联、或该轮尚未收尾）。
 * 前端只在有值时显示「用时 N 秒」，不再自己按条目时间戳推算——
 * 那种推算会因「文本块时间戳 = 块开始时刻」而把最后一段长回答整段漏掉。
 */
durationMs?: number | null, createdAt: number, };

export type SessionMessage = { role: string, id: string, snapshot?: HistoryTurnSnapshot, snapshotId?: string, messageId?: string, userContent?: string, rollbackCheckpointId?: string, rollbackMode?: string, };

export type PagedSessionMessages = { messages: Array<SessionMessage>,
/**
 * 是否还有更早的历史可加载（向上翻页游标）
 */
hasMore: boolean,
/**
 * 本页最老消息的 seq（下次请求的游标）；页为空时为 None
 */
oldestSeq?: number | null,
/**
 * 本页最新消息的 seq；页为空时为 None。
 *
 * 与 `oldest_seq` 的区别：`oldest_seq` 是**上滑游标**（向更早翻页），
 * `newest_seq` 是**本页窗口的最新边界**（首屏时即会话最新端；向前翻页时
 * 仍是那一页的尾部，不是会话最新端）。前端用它与后端 `total_count` 比较，
 * 判断本地窗口是否已经落后于数据库（stale）。
 */
newestSeq?: number | null,
/**
 * 该会话**可见消息**的总条数（与分页查询同一套可见性条件）。
 *
 * 注意口径是「消息条数」而不是「轮数」：`limit` 是轮数，一页可能装几十条消息。
 * 空会话为 0。
 */
totalCount: number, };

export type PlanTask = { subject: string, description: string, dependsOn: Array<number>, canParallelWith: Array<number>, };

export type PlanDocument = { id: string, sessionId: string, title: string, content: string, status: string, path?: string | null, createdAt: number, updatedAt: number, decidedAt?: number | null,
/**
 * 用户拒绝方案时填写的修改意见（与 content 分离，不覆盖原始方案）
 */
rejectionFeedback?: string,
/**
 * 结构化任务分解。审批通过时据此自动建任务图。
 *
 * 存量方案没有这个字段（serde default 为空数组），审批时走兼容分支，
 * 由模型按正文里的任务分解自行 `CreateTask`。
 */
tasks?: Array<PlanTask>, };

export type TodoStatus = "pending" | "in_progress" | "completed";

export type TodoProgress = { done: number, total: number, };

export type TodoItem = { id: string,
/**
 * Imperative form describing what needs to be done.
 */
content: string,
/**
 * Present continuous form shown while the item is in progress.
 */
activeForm: string, status: TodoStatus,
/**
 * 子任务调度进度（仅当该条目正在被 `RunSubagentsSequentially` 驱动时有值）。
 *
 * 存在意义：主 Agent 在调度器里阻塞期间无法调用 `UpdateTodos`，这条进度由调度器
 * 直接回写，否则用户在几十个子任务跑完前看不到任何变化。
 */
progress?: TodoProgress, };

export type TodoGroup = { id: string, items: Array<TodoItem>, };

export type TaskStatus = "pending" | "in_progress" | "completed";

export type Task = { id: number, subject: string, description: string, status: TaskStatus, blockedBy: Array<number>, blocks: Array<number>, owner: string,
/**
 * 进行中时显示的动态文本（如 "Fixing authentication bug"）
 */
activeForm?: string,
/**
 * 任意附加元数据
 */
metadata?: unknown,
/**
 * 该任务由哪种子代理执行（`RunSubagentsSequentially` 调度时读取）。
 *
 * `None` = 沿用历史行为（implementation 型）。任务整体以 JSON 存进
 * `session_tasks.task_json`，所以新增字段**不需要 DB 迁移**。2026-09-20 新增。
 */
subagentType?: string, };

export type AgentRunStatus = "running" | "completed" | "failed" | "cancelled" | "interrupted" | "recovering" | "closed";

export type AgentRun = { runId: string, sessionId: string, status: AgentRunStatus, messageId?: string | null, loopCount: number, inputTokens: number, outputTokens: number, startedAt: number, updatedAt: number, finishedAt?: number | null, lastSafePoint?: string | null, summary?: string | null, resumable: boolean, resumedFromRunId?: string | null,
/**
 * 本 run **因何中断**（`None` = 未中断 / 正常结束），取值见 [`InterruptKind`] 的
 * `as_str()`。
 *
 * 存在的意义：把"中断类型"从 `summary` 的魔法字符串里解放出来——此前
 * "是否因应用关闭而中断"靠比对 `summary` 的固定文案判断，改一句话就会静默
 * 破坏恢复链。本列让恢复链的判断依据是**枚举值**，文案可以随便改。
 *
 * 与 `session_messages.interrupt_kind` 的关系：那是**消息级**（哪条消息被中断
 * 收尾写下，供渲染与发送拼回标记），本列是 **run 级**（这次运行为何结束）。
 * 前者服务"单条消息怎么显示"，后者服务"整次运行怎么恢复/判定"。
 */
interruptKind?: string | null,
/**
 * 本 run 的**等待耗时**（毫秒）：工具执行前等用户审批累计消耗的时间。
 *
 * - `None` = **未统计等待**（早于 v26 迁移的旧行，或收尾路径未走统计逻辑）；
 * - `Some(ms)` = 真实等待毫秒数（可能是 0：全程无需审批）。
 *
 * 为什么不把 0 当成“没等过”：模型耗时与人类确认耗时必须分开统计，
 * 否则审批慢会被算进执行耗时，评测结论失真。
 */
waitMs?: number | null, };

export type AgentRunLoopEvent = { eventId: string, runId: string, sessionId: string,
/**
 * 第几轮（从 1 起），与用户可见的"第 N 轮"一致
 */
loopIndex: number,
/**
 * 本轮响应：结构化 `ContentBlock` 数组（text / thinking / tool_use）
 */
respBlocks: Array<Record<string, unknown>>,
/**
 * 本轮工具执行结果：结构化 `ContentBlock`（ToolResult）
 */
toolResults: Array<Record<string, unknown>>,
/**
 * 'streaming' | 'complete' | 'interrupted'
 */
status: string, error?: string | null, inputTokens: number, outputTokens: number, model?: string | null,
/**
 * 本轮反思审查的判定：`"ok"` / `"not_ok"`；`None` = 本轮未触发反思。
 *
 * 反思是**逐轮**的（每个 loop 的工具执行后最多触发一次，见
 * `pipeline.rs` 的 `should_reflect`），所以"每轮一行"的本表正是它的归属，
 * 不需要另开一张表。
 *
 * 这三个字段**只由 [`mark_loop_reflection`] 写入**，不参与 `upsert_loop_event`
 * 的覆盖集合 —— loop 收尾会重复写同一行（帧级通道 + 收尾覆盖），
 * 若把反思列也纳入覆盖，后一次写入会把已记录的判定抹成 NULL。
 */
reflectionJudgment?: string | null,
/**
 * 反思给出的原因，仅 `judgment = "not_ok"` 时有值
 */
reflectionReason?: string | null,
/**
 * 反思给出的修正建议，仅 `judgment = "not_ok"` 时有值
 */
reflectionSuggestion?: string | null, startedAt: number, updatedAt: number, };

export type ResumeAgentRunPlan = { sessionId: string, prompt: string, };

export type SubAgentStatus = "running" | "completed" | "failed" | "cancelled";

export type SubAgentPhase = "starting" | "waiting_model" | "streaming" | "thinking" | "calling_tool" | "processing_tool_result" | "finalizing";

export type SubAgentRun = { runId: string, sessionId: string, taskId?: number | null, label: string, agentRole: string, promptPreview: string, readOnly: boolean, status: SubAgentStatus, phase: SubAgentPhase, loopCount: number, maxLoops: number, currentTool?: string | null, currentToolInput?: string | null, inputTokens: number, outputTokens: number, startedAt: number, updatedAt: number, finishedAt?: number | null, error?: string | null, summary?: string | null, };

export type SubAgentTraceEvent = { eventId: string, runId: string, sessionId: string, loopIndex: number, eventType: string, payload: unknown, model?: string | null, inputTokens?: number | null, outputTokens?: number | null, timestamp: number, };

export type SubAgentTraceRunSummary = { runId: string, sessionId: string, parentRunId?: string | null, label: string, startedAt: number, loopCount: number, };

export type BackgroundTask = { id: string, session_id?: string | null, command: string, status: string, result?: string | null, port?: number | null, task_type?: string | null, pid?: number | null, pids: Array<number>, };

export type Notification = { task_id: string, session_id?: string | null, status: string, command: string, result: string, port?: number | null, task_type?: string | null, };

export type OpType = "edit" | "write" | "create" | "delete" | "rename";

export type FileOperation = { opType: OpType, path: string, oldContentHash?: string | null, backupPath?: string | null, newContentHash?: string | null, diffSummary?: string | null, };

export type Checkpoint = { id: string, sessionId: string, parentId?: string | null, branchName: string, agentId?: string | null, workspaceId?: string | null, createdAt: number, triggerMessage: string, operations: Array<FileOperation>, metadata: { [key in string]?: string }, };

export type Branch = { name: string, sessionId: string, headCheckpointId?: string | null, createdAt: number, agentId?: string | null, description: string, isActive: boolean, };

export type BranchInfo = { name: string, headCheckpointId?: string | null, checkpointCount: number, isActive: boolean, };

export type CheckpointTree = { sessionId: string, branches: Array<BranchInfo>, checkpoints: Array<Checkpoint>, };

export type RollbackRecallResult = { restoredFiles: Array<string>, recalledText: string,
/**
 * 回滚后**没能恢复**的对象（快照无内容 **且** 回收站副本缺失）。
 *
 * 非空 = 工作区与目标快照**不完全一致**，前端必须提示用户。
 */
restoreWarnings: Array<string>, };

export type RollbackPreviewResult = { targetCheckpointId: string, checkpointLabel: string, files: Array<PatchSummary>,
/**
 * 目标是否是"会话开始前"（撤销全部代码改动）。
 */
initialState: boolean,
/**
 * 被撤回消息是第几轮（从 1 开始）。
 */
roundNumber: number,
/**
 * 从被撤回消息起、到会话结束共有多少轮。
 */
affectedRounds: number, };

export type DiffLine = { "type": "Context", content: string, } | { "type": "Addition", content: string, } | { "type": "Deletion", content: string, };

export type DiffHunk = { old_start: number, old_lines: number, new_start: number, new_lines: number, lines: Array<DiffLine>, };

export type TextDiff = { hunks: Array<DiffHunk>, };

export type Patch = { "type": "create_file", path: string, content: string, } | { "type": "delete_file", path: string,
/**
 * 原文件内容 hash（`snapshot_content` 表）。
 *
 * `None` = 内容读不出来（**目录 / 二进制 / 大文件**）—— 这类对象在快照里
 * **没有内容**，回滚时无法靠快照重建，只能靠 `trash_path` 指向的本体。
 */
content_hash: string | null,
/**
 * 该对象在回收站里的实际位置（`data/trash/<会话 id>/<时间戳>_<名字>`）。
 *
 * 与 `content_hash` 是**两条互补的恢复通道**：文本文件两条都有效（快照是主通道），
 * 目录 / 二进制 / 大文件只有这一条有效。回滚时由 `replay.rs` 的恢复阶段搬回原位。
 * `#[serde(default)]` 兼容本字段引入之前落库的旧快照（那类删除当时无从恢复）。
 */
trash_path: string | null, } | { "type": "update_file", path: string, old_content: string, new_content: string, diff: TextDiff | null,
/**
 * 大文件内容去重引用：(old_hash, new_hash)
 * 内容超过阈值时写入 snapshot_content 表，此处仅保留 hash
 */
content_hash: [string, string] | null, } | { "type": "rename_file", old_path: string, new_path: string, };

export type PatchSummary = { path: string, operation: string, linesAdded: number, linesRemoved: number, };

export type FileInfo = { hash: string, size: number, };

export type WorkspaceState = { files: { [key in string]?: FileInfo }, };

export type Workspace = { files: { [key in string]?: string },
/**
 * 回滚时需要删除的孤儿文件路径（如 RenameFile 留下的旧路径）
 */
delete_paths: Array<string>, };

export type Snapshot = { id: string, parentId?: string | null, branchName: string, patches: Array<Patch>, message?: string | null, isCheckpoint: boolean, workspaceState?: WorkspaceState | null, agentId?: string | null, workspaceId?: string | null, createdAt: number, metadata: { [key in string]?: string }, };

export type SnapshotBranch = { name: string, sessionId: string, headSnapshotId: string, createdAt: number, agentId?: string | null, description: string, isActive: boolean, };

export type SnapshotNode = { id: string, message?: string | null, timestamp: number, isCheckpoint: boolean, agentId?: string | null, children: Array<SnapshotNode>, };

export type BranchView = { name: string, description: string, agentId?: string | null, isActive: boolean, root: SnapshotNode, };

export type SnapshotTreeView = { branches: Array<BranchView>, currentBranch: string, currentSnapshotId: string, };

export type SnapshotSummary = { id: string, message?: string | null, timestamp: number, isCheckpoint: boolean, agentId?: string | null, patchCount: number, patchSummary: Array<PatchSummary>, };

export type SandboxStatus = "active" | "completed" | "published" | "abandoned";

export type AgentSandbox = { sandboxId: string, agentId: string, workspaceId: string, branchName: string, baseSnapshotId: string, workspacePath: string, status: SandboxStatus, createdAt: number, description: string, };

export type SandboxComparison = { sandboxId: string, agentId: string, filesChanged: number, linesAdded: number, linesRemoved: number, snapshotCount: number, lastSnapshotId: string, lastMessage?: string | null, };

export type ConflictType = "both_modified" | "source_deleted" | "target_deleted" | "both_created" | "both_renamed";

export type ConflictResolution = { "type": "keep_source" } | { "type": "keep_target" } | { "type": "keep_both", new_path: string, } | { "type": "manual", resolved_content: string, } | { "type": "custom", content: string, };

export type Conflict = { path: string, conflictType: ConflictType, sourceContent?: string | null, targetContent?: string | null, baseContent?: string | null, resolution?: ConflictResolution | null, };

export type MergeResult = { success: boolean, targetBranch: string, sourceBranch: string, mergedSnapshotId?: string | null, conflicts: Array<Conflict>, autoResolved: number, manualRequired: number, };

export type ChatTurnStartEvent = { sessionId: string, loopCount: number, };

export type ChatTurnEndEvent = { has_tool: boolean, sessionId: string, loopCount: number, };

export type ChatContentEvent = { content: string, sessionId: string, loopCount: number, };

export type ChatThinkingEvent = { content: string, sessionId: string, loopCount?: number, isSubAgent?: boolean, };

export type ChatStreamEvent = { content: string, sessionId: string, loopCount?: number, isSubAgent?: boolean, };

export type ChatToolStartEvent = { sessionId: string, loopCount: number, toolCallId: string, tool: string, };

export type ChatToolDebugEvent = { kind?: string, status?: string, tool?: string, toolCallId?: string, content?: string, sessionId: string, loopCount: number, };

export type AgentStepEvent = { "type": "tool_call", tool: string, content: string, sessionId: string, loopCount: number, } | { "type": "tool_result", tool: string, toolCallId: string, content: string, error: string | null, sessionId: string, loopCount: number, } | { "type": "tool_error", tool: string, toolCallId: string, error: string | null, content: string, outputSummary?: string, sessionId: string, loopCount: number, } | { "type": "notice_clear", kinds: Array<string>, sessionId: string, loopCount?: number, } | { "type": "waiting_hint", silentSecs: number, remainSecs: number, sessionId: string, loopCount: number, } | { "type": "retry", attempt: number, max: number, waitSecs: number, sessionId: string, } | { "type": "reflection", sessionId: string, loopCount: number, judgment: string, reason?: string, suggestion?: string, } | { "type": "interrupted", reason: string, sessionId: string, loopCount: number, } | { "type": "cancelled", sessionId: string, loopCount: number, } | { "type": "task_scheduled", taskId: number, subject: string, sessionId: string, } | { "type": "task_completed", taskId: number, status: string, sessionId: string, } | { "type": "subagent_start", task: string, sessionId: string, isSubAgent: boolean, } | { "type": "subagent_end", sessionId: string, isSubAgent: boolean, };

export type PermissionRequestEvent = { id: string, message: string, sessionId: string, kind: string, allowSession: boolean, warning: string | null, };

export type PermissionResolvedEvent = { id: string, sessionId: string, decision: string, decisionText: string, };

export type PlanProposalEvent = { id: string, title: string, content: string, sessionId: string, };

export type PlanProposalStreamEvent = { content: string, sessionId: string, };

export type ApprovalModeChangedEvent = { sessionId: string, mode: string, };

export type AgentReadOnlyChangedEvent = { sessionId: string, enabled: boolean, };

export type AgentWorkModeChangedEvent = { sessionId: string, from: string, to: string, reason: string, };

export type SessionAllowancesChangedEvent = { sessionId: string, };

export type SessionThinkingModeChangedEvent = { sessionId: string, thinkingEnabled: boolean, };

export type SessionMessagesChangedEvent = { sessionId: string, count: number, lastSeq: number | null, reason: string, };

export type BgCompactingChangedEvent = { sessionId: string, compacting: boolean, };

export type BackgroundFailedEvent = { sessionId: string | null, taskId: string, command: string, result: string, port: number | null, taskType: string | null, };

export type CheckpointCreatedEvent = { sessionId: string, checkpointId: string | null, hasPatches: boolean, canRollback: boolean, message: string, };

export type SnapshotCreatedEvent = { sessionId: string, snapshotId: string, };

export type MemoryUpdatedEvent = { sessionId: string, summary: string, };

export type SubagentTraceUpdatedEvent = { runId: string, loopIndex: number, };

export type TodoUpdateEvent = { todos: Array<TodoItem>, groups: Array<TodoGroup>, sessionId: string, };

export type UserQuestionOption = { label: string, description?: string, recommended: boolean, };

export type UserQuestionItem = { id: string, header?: string, question: string, multiSelect: boolean, options: Array<UserQuestionOption>, allowCustom: boolean, };

export type UserQuestionRequestEvent = { id: string, sessionId: string, title?: string, questions: Array<UserQuestionItem>, };

export type UserQuestionResolvedEvent = { id: string, sessionId: string,
/**
 * `answered` / `skipped` / `interrupted`
 */
status: string, };

export type UserQuestionAnswerItem = { questionId: string, selectedOptions: Array<string>, customText?: string, };

export type McpIsolation = "global" | "session";

export type McpTransportKind = "stdio" | "streamable_http";

export type McpToolHints = { readOnlyHint?: boolean, destructiveHint?: boolean, idempotentHint?: boolean, openWorldHint?: boolean, };

export type McpServerConfig = {
/**
 * 本地 stdio 服务：可执行命令（如 `"npx"`、`"uvx"`、`"node"`）
 */
command?: string,
/**
 * 本地 stdio 服务：命令行参数
 */
args: Array<string>,
/**
 * 本地 stdio 服务：附加环境变量（常含 API Key，前端默认遮罩）
 */
env: { [key in string]?: string },
/**
 * 本地 stdio 服务：可选工作目录
 */
cwd?: string,
/**
 * 远程 Streamable HTTP 服务：端点 URL
 */
url?: string,
/**
 * 远程 Streamable HTTP 服务：附加请求头（如 `Authorization`，前端默认遮罩）
 */
headers: { [key in string]?: string },
/**
 * 是否启用该服务（默认 true）
 */
enabled: boolean,
/**
 * 是否信任该服务（默认 false；打开后才享受"帮我批准/完全批准"档位自动放行）
 */
trusted: boolean,
/**
 * 是否随应用启动后台拉起（默认 false，即懒启动：首次用到时才拉起）
 */
autoStart: boolean,
/**
 * 隔离级别（默认 global，预留 session）
 */
isolation: McpIsolation,
/**
 * 启动握手超时（秒，默认 60）
 */
startupTimeoutSec: number,
/**
 * 单次调用超时（秒，默认 60）
 */
callTimeoutSec: number,
/**
 * 空闲自动休眠时长（分钟，默认 30；设为 0 表示不自动休眠）
 */
idleTimeoutMin: number,
/**
 * 被用户单独关闭的原始工具名列表
 */
disabledTools: Array<string>, };

export type McpServerStatus = "disabled" | "idle" | "starting" | "ready" | "failed" | "stopped";

export type McpServerSummary = { name: string, transport: McpTransportKind, status: McpServerStatus, enabled: boolean, trusted: boolean, autoStart: boolean, toolCount: number, resourceCount: number, promptCount: number,
/**
 * 是否已有本地能力缓存（用于前端标识"待首次连接"）
 */
hasCachedCapabilities: boolean, lastError?: string,
/**
 * 原始配置（用于编辑表单回显）
 */
config: McpServerConfig, };

export type McpToolView = {
/**
 * 暴露给大模型的命名空间工具名（如 `mcp__fs__read_file`）
 */
qualifiedName: string,
/**
 * MCP Server 声明的原始工具名
 */
name: string, serverName: string, description: string, enabled: boolean, inputSchema: Record<string, unknown>, hints: McpToolHints, };

export type McpTestConfigResult = { ok: boolean, toolCount: number, resourceCount: number, promptCount: number, message: string, };

export type McpImportItemResult = {
/**
 * 原始 JSON 中的 key
 */
originalName: string,
/**
 * 最终落盘的规范服务名
 */
savedName: string,
/**
 * 是否导入成功
 */
ok: boolean,
/**
 * 提示或失败原因（如名称已自动清洗、字段非法等）
 */
message?: string, };

export type McpResourceItemView = { serverName: string, uri: string, name: string, description?: string, mimeType?: string, };

export type McpResourceReadResult = { uri: string, content: string, mimeType?: string, isBinary: boolean, };

export type CachedPromptArgument = { name: string, description?: string, required: boolean, };

export type McpPromptItemView = { serverName: string, name: string, description?: string, arguments: Array<CachedPromptArgument>, };

export type RenderedPromptMessage = { role: string, text: string, };

export type McpPromptRenderResult = { name: string, description?: string, messages: Array<RenderedPromptMessage>,
/**
 * 拼合后的纯文本草稿（可直接填入聊天输入框）
 */
combinedText: string, };

export type McpServerStatusEvent = { serverName: string, status: McpServerStatus, toolCount: number, resourceCount: number, promptCount: number, lastError?: string, };

export type ResolveUserQuestionResult = { resolved: boolean, status: string, };

export type ErrorCode = "INVALID_INPUT" | "NOT_FOUND" | "CONFLICT" | "PERMISSION_DENIED" | "CONFIG_ERROR" | "DATABASE_ERROR" | "NETWORK_ERROR" | "RATE_LIMITED" | "CANCELLED" | "INTERNAL_ERROR";

export type CommandError = { code: ErrorCode, message: string, retryable: boolean, };

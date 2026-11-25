// 本文件由 Rust 端 ts-rs 自动生成，请勿手工修改。
// 若 Rust 结构体字段有变，请在 src-tauri/ 下运行：
//   UPDATE_TS_BINDINGS=1 cargo test ts_contracts_are_up_to_date

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

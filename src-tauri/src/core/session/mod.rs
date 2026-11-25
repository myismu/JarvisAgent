//! # 会话持久化模块 (Session Persistence)
//!
//! 将对话历史持久化到 SQLite，支持多会话管理。
//! 会话元数据、消息、步骤和计划文档由 `crate::core::session::repository` 统一读写。
//!
//! 主要功能：
//! - 会话 CRUD：创建、加载、保存、删除、重命名
//! - 图片管理：base64 图片存入附件目录，保存时清理内联数据
//! - 计划文档：会话级 PlanDocument 的增删改查
//! - 自动标题：从首条用户消息截取标题
//! - Token 统计：累计 input/output token 用量
//!
//! 存储结构：`<agent_home>/jarvis.sqlite3`

pub mod memory;
pub mod repository;
pub mod resource_repository;
pub mod thinking;
pub mod usage;

use crate::infra::types::models::{
    Content, ContentBlock, ImageSource, Message, MessageSource, PlanDocument, SessionMemory,
};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// 标题来源：默认（截取首条消息）、自动（LLM 生成）、手动（用户修改）
const DEFAULT_TITLE_SOURCE: &str = "default";
const AUTO_TITLE_SOURCE: &str = "auto";
const MANUAL_TITLE_SOURCE: &str = "manual";

/// 剥离 Windows 扩展长度路径前缀 `\\?\`。
///
/// `canonicalize()` 在 Windows 上会产生该前缀（如 `\\?\C:\Users\...`），
/// 文件系统操作不受影响，但展示难看、且与用户输入的普通路径不一致。
/// 项目路径入库与会话工作区绑定前统一调用。
pub fn strip_extended_path_prefix(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

/// 将 base64 图片数据解码后写入数据库，返回文件名。
///
/// 图片本体存在 `<agent_home>/jarvis.sqlite3` 的 `session_attachments` 表
/// （`data` 列为 BLOB 原始字节），**不落磁盘**。
/// 返回的文件名形如 `{session_id}_{8位uuid}.{ext}`，仅作为该行的主键，
/// 被写入消息块的 `file_path`，供后续 `load_image_data` 取回。
pub fn save_image_to_file(session_id: &str, media_type: &str, data: &str) -> String {
    let ext = if media_type.contains("jpeg") || media_type.contains("jpg") {
        "jpg"
    } else if media_type.contains("gif") {
        "gif"
    } else if media_type.contains("webp") {
        "webp"
    } else {
        "png"
    };
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let filename = format!("{}_{}.{}", session_id, id, ext);
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(data) {
        let _ = resource_repository::save_attachment(session_id, &filename, media_type, &decoded);
    }
    filename
}

/// 按文件名从数据库取回图片原始字节，重新 base64 编码后返回
pub fn load_image_data(filename: &str) -> Option<String> {
    let (_, bytes) = resource_repository::load_attachment(filename)
        .ok()
        .flatten()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// 按文件名删除数据库中的图片记录
pub fn delete_image_file(filename: &str) {
    let _ = resource_repository::delete_attachment(filename);
}

/// 项目元信息
#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub path: String,
    #[ts(type = "number")]
    pub created_at: u64,
    #[ts(type = "number")]
    pub updated_at: u64,
    /// 项目下的会话数
    #[serde(default)]
    pub session_count: usize,
}

/// 会话元信息（用于列表展示，不含完整消息体）
#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    #[ts(type = "number")]
    pub created_at: u64,
    #[ts(type = "number")]
    pub updated_at: u64,
    pub message_count: usize,
    #[serde(default)]
    pub is_smart_named: bool,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub profile_id: Option<String>,
    #[serde(default)]
    #[ts(type = "number")]
    pub total_input_tokens: u64,
    #[serde(default)]
    #[ts(type = "number")]
    pub total_output_tokens: u64,
    /// 会话累计缓存命中 token（来自各次请求的 `prompt_cache_hit_tokens` 等字段）。
    ///
    /// 0 等价于"从未报告"——展示层据此显示 `--` 而不是 `0%`。
    #[serde(default)]
    #[ts(type = "number")]
    pub total_cache_hit_tokens: u64,
    /// 会话累计缓存未命中 token。与命中数一样，0 表示未报告。
    ///
    /// 刻意**不**用 `total_input_tokens - total_cache_hit_tokens` 推导：
    /// 各家的 `input_tokens` 口径不同（Anthropic 的 `input_tokens` 本身就不含缓存读取），
    /// 相减会得出错误的未命中数。这里只忠实累加厂商报告的值。
    #[serde(default)]
    #[ts(type = "number")]
    pub total_cache_miss_tokens: u64,
    #[serde(default = "default_title_source")]
    pub title_source: String,
    /// 所属项目 ID，None 表示无项目独立会话
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<String>,
    /// 工作目录（从 projects 表派生，仅读）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub working_directory: Option<String>,
    /// 深度思考档位（**会话级布尔**）：`true` = 本会话开启思考，`false` = 关闭。
    ///
    /// v14 起只有布尔，**没有"未表态"这个状态**——"跟随设置默认"在会话创建时
    /// （`pipeline::start_run` 首次解析）就被固化成本列的值，此后设置页怎么改都不会
    /// 回溯修改它。这是与 `profile_id`（随会话走的模型预设）对称的"随会话走的推理档"。
    #[serde(default)]
    pub thinking_mode: bool,
    /// 归档时间（**毫秒**，Unix epoch）。`None` = 未归档。
    ///
    /// 列名 `deleted_at` 是「删除」时代的遗留，语义早已是归档：v27 起归档项目会把
    /// 名下会话统一打上**同一个**时间戳（见 `repository::archive_project`），
    /// 归档页据此展示「何时归档」。
    ///
    /// 刻意**不加** `skip_serializing_if`：前端要靠 `deletedAt: null` 与「字段缺失」区分
    /// 「未归档」与「后端没给这个字段」，省略会让两者在前端混成一种状态。
    #[serde(default)]
    #[ts(optional, type = "number | null")]
    pub deleted_at: Option<u64>,
    /// 所属项目名（从 `projects.name` 派生，仅读）。
    ///
    /// 归档页要回答「这条会话原本属于哪个项目」——项目归档后已不在项目列表里，
    /// 前端拿不到名字就没法分组展示。无项目（独立会话）时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_name: Option<String>,
}

/// 批量操作中单个 id 的失败原因。
///
/// 批量恢复/批量删除**不允许整批回滚**：用户勾了 10 条、其中 1 条已被别的窗口
/// 处理掉，剩下的 9 条仍应完成。失败项逐条回报，界面才能精确提示「哪几条没成功」。
#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BatchOpFailure {
    pub id: String,
    /// 失败原因（面向用户的中文短句，直接展示）
    pub reason: String,
}

/// 批量操作结果。
///
/// 三个字段各自独立，不从彼此推导：
/// - `succeeded_ids`：真正被改动成功的目标 id；
/// - `failed`：未成功的目标 id 及原因；
/// - `restored_project_ids`：**副作用**结果——恢复会话时连带复活的归档项目。
///   它与前两者没有必然数量关系（一次恢复可能复活 0 个、也可能复活 1 个），
///   所以必须单独回报，不能靠前端猜。
#[derive(Serialize, Deserialize, Debug, Clone, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BatchOpResult {
    pub succeeded_ids: Vec<String>,
    pub failed: Vec<BatchOpFailure>,
    pub restored_project_ids: Vec<String>,
}

/// 一次 pipeline 运行产生的用量增量，由 `save_session` 累加进会话累计值。
///
/// ⚠️ 2026-11 起**生产路径不再产生非零增量**：主循环与子 Agent 的每一次请求
/// 都在当下由 `usage::record_request_usage` 写入明细并累加累计值。
/// 回合收尾再补交一次就是重复计数（明细行数会少于累计口径能解释的量）。
/// 参数本身保留：`save_session` 有 19 个调用点，现有调用一律传 `None`。
///
/// 抽成结构体而不是四元组：四个字段都是 u64，元组传错顺序编译器不会拦，
/// 而"把未命中数写进命中列"这种错只能靠人眼发现。
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsageDelta {
    pub input: u64,
    pub output: u64,
    /// 缓存命中 / 未命中；厂商未报告时保持 0（0 在展示层等价于"未报告"）
    pub cache_hit: u64,
    pub cache_miss: u64,
}

/// 会话累计用量的**当前值**（不是增量），累加完成后回给调用方做事件推送。
///
/// 与 `TokenUsageDelta` 分开是因为两者语义相反、不可互换：把「增量」当「总量」
/// 发出去，前端会把累计值覆盖成某一轮的零头，数字会莫名其妙地缩水。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionTokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_hit: u64,
    pub cache_miss: u64,
}

/// 一次请求用量的**来源**：这批 token 是被谁烧掉的。
///
/// 明细表 `request_usage.source` 列存的就是 [`UsageSource::as_str`] 的返回值。
/// 枚举 ↔ 字符串的转换**只**走 `as_str` / `from_str` 一对函数（与 `MessageSource`
/// 同一套口径），不另立第二套映射——两套映射迟早会漂移，而漂移之后
/// “子 Agent 的账算进了主循环”这类错误不会报错，只会静静地算错数。
///
/// ⚠️ 本枚举**确实**派生了 `Serialize`（事件载荷与命令回包需要它），
/// 因此多了一套 serde 映射；`usage_tests::usage_source_serde_matches_as_str`
/// 把两者钉死，新增变体时忘改任何一边都会变红。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UsageSource {
    /// 主 Agent 的主循环（每个 loop 一次模型请求）
    Main,
    /// 子 Agent（独立循环，不经过主流程的逐请求路径）
    Subagent,
    /// 反思机制（回合结束后的自我评估请求）
    Reflection,
    /// 上下文压缩（摘要生成请求）
    Compact,
    /// 会话自动命名（utility 模型请求）
    SessionName,
    /// 记忆管理员（会话结束后提炼长期记忆的请求）
    MemoryCurator,
}

impl UsageSource {
    /// 落库形态（`request_usage.source` 列的值）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Subagent => "subagent",
            Self::Reflection => "reflection",
            Self::Compact => "compact",
            Self::SessionName => "session_name",
            Self::MemoryCurator => "memory_curator",
        }
    }

    /// 从落库形态还原。
    ///
    /// **未知值一律报错**，不静默降级：聚合结果会被当成“账本”给用户看，
    /// 把拼错的值兑成 `Main` 等于把子 Agent 的开销报成主循环的，
    /// 错得安静且无法从数字上察觉。
    #[allow(clippy::should_implement_trait)] // 与全仓既有调用口径保持一致
    pub fn from_str(raw: &str) -> Result<Self, String> {
        match raw {
            "main" => Ok(Self::Main),
            "subagent" => Ok(Self::Subagent),
            "reflection" => Ok(Self::Reflection),
            "compact" => Ok(Self::Compact),
            "session_name" => Ok(Self::SessionName),
            "memory_curator" => Ok(Self::MemoryCurator),
            other => Err(format!(
                "未知的 request_usage.source 取值「{}」——可能漏了迁移，或数据被外部改写",
                other
            )),
        }
    }

    /// 全部取值（测试与前端选项列表用；顺序即展示顺序）。
    pub fn all() -> [Self; 6] {
        [
            Self::Main,
            Self::Subagent,
            Self::Reflection,
            Self::Compact,
            Self::SessionName,
            Self::MemoryCurator,
        ]
    }
}

/// **一次**模型请求的用量明细（`request_usage` 表的一行）。
///
/// 为什么不要无脑用元组/参数列表：字段多达十几个，且 `input/output`、
/// `cache_hit/cache_miss` 都是 `u64`——元组传错顺序编译器不拦，
/// 只有人眼能从数字上看出“命中数写进未命中列”。
///
/// 构造用 builder（[`RequestUsageRecord::new`] + 链式方法）：
/// 各个调用点需要的字段不一样（主循环有 run/loop，压缩/命名没有），
/// 全字段构造函数会逼着每个调用点写一长串 `None`，改天加字段就是全局改。
#[derive(Debug, Clone)]
pub struct RequestUsageRecord {
    pub session_id: String,
    /// 所属 run（`agent_runs.run_id`）。压缩/命名等不属于任何 run 时为 `None`。
    pub run_id: Option<String>,
    /// run 内的循环序号。子 Agent 用**自己**的循环序号，与父 run 无关。
    pub loop_index: Option<u32>,
    /// 这次请求的发起方（主循环 / 子 Agent / 压缩……）
    pub source: UsageSource,
    /// 触发这次请求的工具调用 id（子 Agent 溯源用：哪个 `tool_use` 起的子任务）
    pub parent_tool_use_id: Option<String>,
    /// 执行者角色（如 `main` / `subagent:code-review`），仅作展示与排查用
    pub agent_role: Option<String>,
    /// 关联的会话任务 id（任务系统的 task_id）
    pub task_id: Option<String>,
    /// 实际使用的模型 id（代理/预设切换后以实际下发的为准）
    pub model_id: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// 缓存命中 / 未命中：`None` = **厂商这次没上报**，与 `Some(0)` 严格区分
    pub cache_hit_tokens: Option<u64>,
    pub cache_miss_tokens: Option<u64>,
    /// 缓存读数的口径来源（如 `deepseek` / `anthropic` / `context`），排查用
    pub cache_source: Option<String>,
    /// 落库时间（**毫秒**）。`None` = 写入时取当前时间（测试可显式指定）
    pub created_at: Option<u64>,
}

impl RequestUsageRecord {
    /// 新记录：只有会话与来源是必填项，其余字段按场景链式补。
    pub fn new(session_id: impl Into<String>, source: UsageSource) -> Self {
        Self {
            session_id: session_id.into(),
            run_id: None,
            loop_index: None,
            source,
            parent_tool_use_id: None,
            agent_role: None,
            task_id: None,
            model_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_hit_tokens: None,
            cache_miss_tokens: None,
            cache_source: None,
            created_at: None,
        }
    }

    /// 归属到某个 run 的第几个 loop。
    pub fn run(mut self, run_id: impl Into<String>, loop_index: Option<u32>) -> Self {
        self.run_id = Some(run_id.into());
        self.loop_index = loop_index;
        self
    }

    /// 触发这次请求的工具调用 id（子 Agent 溯源链）。
    pub fn parent_tool_use_id(mut self, value: impl Into<String>) -> Self {
        self.parent_tool_use_id = Some(value.into());
        self
    }

    /// 执行者角色。
    pub fn agent_role(mut self, value: impl Into<String>) -> Self {
        self.agent_role = Some(value.into());
        self
    }

    /// 关联的会话任务 id。
    pub fn task_id(mut self, value: impl Into<String>) -> Self {
        self.task_id = Some(value.into());
        self
    }

    /// 实际使用的模型 id。
    pub fn model_id(mut self, value: impl Into<String>) -> Self {
        self.model_id = Some(value.into());
        self
    }

    /// 本次请求的输入/输出 token（厂商上报的**本次**值，不是累计）。
    pub fn tokens(mut self, input: u64, output: u64) -> Self {
        self.input_tokens = input;
        self.output_tokens = output;
        self
    }

    /// 缓存读数：两个值必须同真同假（厂商要么都报，要么都不报）；
    /// 只给一个时按“未上报”处理，宁可丢一个读数，也不编一个 0 出来。
    pub fn cache(
        mut self,
        hit: Option<u64>,
        miss: Option<u64>,
        source: Option<&str>,
    ) -> Self {
        match (hit, miss) {
            (Some(hit), Some(miss)) => {
                self.cache_hit_tokens = Some(hit);
                self.cache_miss_tokens = Some(miss);
                self.cache_source = source.map(|s| s.to_string());
            }
            _ => {
                self.cache_hit_tokens = None;
                self.cache_miss_tokens = None;
                self.cache_source = None;
            }
        }
        self
    }

    /// 显式指定落库时间（毫秒）；不调则写入时取当前时间。
    pub fn at(mut self, created_at: u64) -> Self {
        self.created_at = Some(created_at);
        self
    }
}

/// 按来源汇总的一段账（对应 `request_usage` 的一行 `GROUP BY source`）。
///
/// `unreported_*` 两个字段是**可空语义的携带者**：
/// 展示层要能说“这 3 次请求厂商没报缓存，涉及 12k 输入 token”，
/// 而不是把它们的缓存当成 0 算进命中率分母。
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename = "UsageSourceAuditRow", rename_all = "camelCase")]
pub struct UsageSourceBreakdown {
    pub source: UsageSource,
    #[ts(type = "number")]
    pub request_count: u64,
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// 已上报的命中/未命中之和（未上报的请求不参与，也不按 0 计入）
    #[ts(type = "number")]
    pub cache_hit_tokens: u64,
    #[ts(type = "number")]
    pub cache_miss_tokens: u64,
    /// 缓存字段未上报的请求数
    #[ts(type = "number")]
    pub unreported_requests: u64,
    /// 这些未上报请求对应的输入 token 合计（让“不知道”也有量级）
    #[ts(type = "number")]
    pub unreported_input_tokens: u64,
}

/// 一个会话的用量账本（按来源分组 + 总计）。
///
/// `has_detail = false` 表示**该会话没有任何明细行**（早于 v28 的历史会话），
/// 此时数字全为 0。调用方必须靠这个标志区分“真的没用过 token”与“没明细可拆”——
/// 把后者当 0 展示会让用户以为历史会话白嫆。此时若要展示总量，
/// 应回退到 `sessions.total_*`（本结构体不重复携带，避免两处口径打架）。
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageBreakdown {
    pub session_id: String,
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    #[ts(type = "number")]
    pub cache_hit_tokens: u64,
    #[ts(type = "number")]
    pub cache_miss_tokens: u64,
    /// 明细行数（= 请求次数）
    #[ts(type = "number")]
    pub request_count: u64,
    #[ts(type = "number")]
    pub unreported_requests: u64,
    #[ts(type = "number")]
    pub unreported_input_tokens: u64,
    /// 是否存在明细（`false` = 历史会话，数字不可信）
    pub has_detail: bool,
    pub sources: Vec<UsageSourceBreakdown>,
}

impl UsageBreakdown {
    /// 没有任何明细时的空账本（`has_detail = false`）。
    pub fn empty(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_hit_tokens: 0,
            cache_miss_tokens: 0,
            request_count: 0,
            unreported_requests: 0,
            unreported_input_tokens: 0,
            has_detail: false,
            sources: Vec::new(),
        }
    }
}

// ============================================================================
// 用量**视图**（账本的投影）
//
// 这一组结构是给前端与命令回包用的，和上面的账本结构（`UsageBreakdown` /
// `UsageSourceBreakdown`）刻意分成两套：
// - **账本是权威**：字段与 `request_usage` 明细表一一对应，只由 SQL 聚合产出，
//   数字能追溯到具体请求行，是排查"账对不对"的唯一依据；
// - **视图是投影**：只做改名（`input_tokens` → `input`）、分层折叠（六个来源 →
//   主/子/系统三桶）和派生字段（`cache_reported`）三件事，**不产生新数字**。
//
// 为什么不把字段直接改成前端口径：账本还要被 `repository`、审计与测试读取，
// 若让展示口径反向侵入账本，将来前端改个字段名就会牵动 SQL 聚合层。
// 投影层做厚一点，两边的演进互不牵连。
// ============================================================================

/// 单一来源的用量视图（对应前端 `UsageSourceBreakdown`）。
///
/// 全部字段都是**会话级累计绝对值**（不是单次请求的增量），
/// 接收方整值覆盖即可，任何自行累加都会让数字翻倍。
#[derive(Debug, Clone, Default, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageSourceView {
    /// 累计输入 token
    #[ts(type = "number")]
    pub input: u64,
    /// 累计输出 token
    #[ts(type = "number")]
    pub output: u64,
    /// 累计缓存命中 token。0 有两种可能：真的没命中、厂商没报（后者看 `cache_reported`）
    #[ts(type = "number")]
    pub cache_hit: u64,
    /// 累计缓存未命中 token
    #[ts(type = "number")]
    pub cache_miss: u64,
    /// 累计请求次数（= 调用模型接口的次数，不是 agent loop 轮数）
    #[ts(type = "number")]
    pub request_count: u64,
    /// 其中**上报了缓存字段**的请求数。
    ///
    /// 推导关系：`cache_reported = request_count - unreported_requests`。
    /// 两者是同一批请求的两种数法，不是独立读数——同一行的请求要么报了缓存、
    /// 要么没报，没有第三种。因此不另加一列 SQL（另加 = 多一个可能与差值打架的口径）。
    /// 与 `request_count` 的差值即「厂商没给缓存字段」的请求量，用来解释
    /// 某个来源为什么算不出命中率，而不是把未上报直接画成 0%。
    #[ts(type = "number")]
    pub cache_reported: u64,
}

impl UsageSourceView {
    /// 把一段来源账（某一来源的 `GROUP BY` 汇总行）累加进本视图。
    ///
    /// `system` 桶由四个来源（反思/压缩/命名/记忆整理）合成，靠它折叠；
    /// 逐字段显式相加而不是实现 `Add`：字段里有一个是**派生值**，
    /// 通用加法容易让人以为可以随便加两段账而不检查口径。
    fn accumulate(&mut self, src: &UsageSourceBreakdown) {
        self.input += src.input_tokens;
        self.output += src.output_tokens;
        self.cache_hit += src.cache_hit_tokens;
        self.cache_miss += src.cache_miss_tokens;
        self.request_count += src.request_count;
        // 派生字段同样按"请求数相减"累加，理由见字段文档
        self.cache_reported += src.request_count.saturating_sub(src.unreported_requests);
    }
}

/// 会话按来源分层的用量视图（对应前端 `SessionUsageBreakdown`）。
///
/// 三层是**展示口径**而非存储口径：账本按 `UsageSource` 存六个来源，
/// 用户只关心"主对话 / 子 Agent / 系统内部"这三件事。
#[derive(Debug, Clone, Default, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionUsageBreakdownView {
    /// 主 Agent 主循环
    pub main: UsageSourceView,
    /// 子 Agent（RunSubagent / 任务图调度出去的执行体）
    pub subagent: UsageSourceView,
    /// 系统内部调用（反思、压缩、自动命名、记忆整理）
    pub system: UsageSourceView,
    /// 缓存字段未上报的请求数（整会话口径）
    #[ts(type = "number")]
    pub unreported_request_count: u64,
    /// 这些未上报请求对应的输入 token 合计（让"不知道"也有量级）
    #[ts(type = "number")]
    pub unreported_input: u64,
    /// 是否存在明细。`false` = 无明细行（历史会话），三层全零**不可当真实分布展示**
    pub has_detail: bool,
}

/// 一个会话的用量视图（账本 → 视图的投影结果）。
///
/// 调用方拿到的应该永远是这个结构：账本（[`UsageBreakdown`]）本身是内部口径，
/// 会随明细表结构调整；视图字段一旦放出就是前端契约，改它要同步改 TS 类型。
#[derive(Debug, Clone, Default, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionUsageView {
    pub session_id: String,
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    #[ts(type = "number")]
    pub cache_hit_tokens: u64,
    #[ts(type = "number")]
    pub cache_miss_tokens: u64,
    /// 按来源分层的明细。`has_detail = false` 时三层全零，只有上面的总计有意义
    pub breakdown: SessionUsageBreakdownView,
}

impl SessionUsageView {
    /// 账本 → 视图的折叠：四个总计透传，来源逐行归入三桶。
    ///
    /// 折叠规则刻意写成 `match` **穷举六个变体**，不留 `_ =>` 兜底：
    /// 新增 `UsageSource` 变体时，这里是唯一需要人为决策"它算谁的开销"的地方。
    /// 若留兜底，新变体就会悄悄落进某个桶（大概率是 `system`），
    /// 表现为"主对话的账对不上总数"，而且不报错、只能靠人肉对数字才发现。
    /// 编译失败一次，比账错三个月划算。
    pub fn from_breakdown(b: &UsageBreakdown) -> Self {
        let mut breakdown = SessionUsageBreakdownView {
            unreported_request_count: b.unreported_requests,
            unreported_input: b.unreported_input_tokens,
            has_detail: b.has_detail,
            ..Default::default()
        };

        for src in &b.sources {
            match src.source {
                // 主 Agent 主循环独占一桶：它是用户唯一直接操作的那条线
                UsageSource::Main => breakdown.main.accumulate(src),
                // 子 Agent 独占一桶：它常是单次任务里花钱最多的部分，必须单独可见
                UsageSource::Subagent => breakdown.subagent.accumulate(src),
                // 剩下四个都是"用户没直接发起"的系统内部请求，合成系统开销：
                // 反思 = 回合结束后的自我评估
                // 压缩 = 上下文摘要生成
                // 命名 = 会话自动标题
                // 记忆整理 = 会话结束后提炼长期记忆
                UsageSource::Reflection
                | UsageSource::Compact
                | UsageSource::SessionName
                | UsageSource::MemoryCurator => breakdown.system.accumulate(src),
            }
        }

        Self {
            session_id: b.session_id.clone(),
            input_tokens: b.input_tokens,
            output_tokens: b.output_tokens,
            cache_hit_tokens: b.cache_hit_tokens,
            cache_miss_tokens: b.cache_miss_tokens,
            breakdown,
        }
    }
}

/// 获取当前时间戳（**毫秒**，Unix epoch）。
///
/// 2026-09-19 起全项目 DB 时间戳统一为毫秒（v16 迁移已把存量秒值 ×1000）。
/// 此前本函数返回秒、而 agent_runs 等运行时表用毫秒，两套口径并存导致
/// 侧边栏会话时间显示错误（前端 `new Date(ts)` 按毫秒解析秒值）。
fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn default_title_source() -> String {
    DEFAULT_TITLE_SOURCE.to_string()
}

fn is_default_title_source(title_source: &str) -> bool {
    title_source == DEFAULT_TITLE_SOURCE
}

fn set_auto_title_source(meta: &mut SessionMeta) {
    meta.is_smart_named = true;
    meta.title_source = AUTO_TITLE_SOURCE.to_string();
}

fn set_manual_title_source(meta: &mut SessionMeta) {
    meta.is_smart_named = false;
    meta.title_source = MANUAL_TITLE_SOURCE.to_string();
}

/// 从消息列表中提取标题（取第一条用户消息的前 30 个字符）
fn extract_title(messages: &[Message]) -> String {
    for msg in messages {
        if let Message::User { content } = msg {
            let text = match content {
                Content::Single(s) => s.clone(),
                Content::Multiple(blocks) => {
                    // 从多块内容中提取第一个文本块
                    blocks
                        .iter()
                        .find_map(|b| {
                            if let crate::infra::types::models::ContentBlock::Text { text } = b {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default()
                }
            };
            // Context 块不是 Text，上面的 find_map 已经跳过，
            // 这里拿到的 text 就是用户原文
            let user_input = text.trim().to_string();
            if !user_input.is_empty() {
                let title: String = user_input
                    .chars()
                    .take(crate::infra::types::constants::MAX_SESSION_TITLE_LEN)
                    .collect();
                return if user_input.chars().count()
                    > crate::infra::types::constants::MAX_SESSION_TITLE_LEN
                {
                    format!("{}...", title)
                } else {
                    title
                };
            }
        }
    }
    "新会话".to_string()
}

/// 列出所有会话（按更新时间倒序）
pub fn list_sessions() -> Vec<SessionMeta> {
    repository::list_sessions().unwrap_or_default()
}

/// 创建新会话，返回元信息
pub fn create_session(project_id: Option<String>) -> SessionMeta {
    let now = now_ts();
    let working_directory = project_id
        .as_ref()
        .and_then(|pid| repository::get_project_path(pid).ok().flatten());
    let meta = SessionMeta {
        id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
        title: "新会话".to_string(),
        created_at: now,
        updated_at: now,
        message_count: 0,
        is_smart_named: false,
        profile_id: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_hit_tokens: 0,
        total_cache_miss_tokens: 0,
        title_source: default_title_source(),
        project_id,
        working_directory,
        // 新会话的思考档位先落一个保守占位 `false`；
        // 真正的初值在第一条消息时由 `pipeline::start_run` 按"设置默认 + 模型能力"解析后写入。
        thinking_mode: false,
        // 新建的会话一定不在归档列表里，也没有「项目名」这个派生值可填
        deleted_at: None,
        project_name: None,
    };
    let memory = SessionMemory::default();
    let _ = repository::upsert_session(&meta, &memory);
    let _ = repository::set_last_active_session_id(&meta.id);
    meta
}

/// 保证会话消息 ID、sources、interrupt_kinds 与消息数组长度一致。
pub fn normalize_message_ids(memory: &mut SessionMemory) {
    let msg_count = memory.messages.len();
    let id_count = memory.message_ids.len();

    // 多余的 ID 从头部删除（压缩摘要插入在数组开头，尾部截断会删错）
    if id_count > msg_count {
        let excess = id_count - msg_count;
        memory.message_ids.drain(0..excess);
    }

    // 同步 sources：多余部分从头部删除
    if memory.sources.len() > msg_count {
        let excess = memory.sources.len() - msg_count;
        memory.sources.drain(0..excess);
    }

    // 同步 interrupt_kinds：与 sources 同尺（头部删、尾部补 None）
    if memory.interrupt_kinds.len() > msg_count {
        let excess = memory.interrupt_kinds.len() - msg_count;
        memory.interrupt_kinds.drain(0..excess);
    }

    // 填充空的 message_id
    for message_id in &mut memory.message_ids {
        if message_id.trim().is_empty() {
            *message_id = uuid::Uuid::new_v4().to_string();
        }
    }

    // 补齐缺失的 message_id（数组长度不一致时）
    // 使用索引生成固定前缀 id，避免随机 UUID 覆盖已有 id 导致消息对应关系错乱
    let prefix = uuid::Uuid::new_v4().simple().to_string()[..6].to_string();
    let missing = msg_count.saturating_sub(memory.message_ids.len());
    for _ in 0..missing {
        let index = memory.message_ids.len();
        memory.message_ids.push(format!(
            "auto:{}{}:{}",
            prefix,
            index,
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        ));
    }

    // 补齐缺失的 sources。
    // 缺失一律按 `Chat` 补：本函数是长度失衡的修复路径，而每条消息的来源都由
    // `append_message` 与消息同步写入，走到这里说明有路径绕过了它 —— 那种情况下
    // 绝大多数是普通对话（注入与占位都有显式的来源标注）。**不要**改成 `Placeholder`：
    // 那会让消息从模型上下文与界面同时消失，把"数组失衡"升级成"内容丢失"。
    while memory.sources.len() < msg_count {
        memory.sources.push(MessageSource::Chat);
    }

    // 补齐缺失的 interrupt_kinds（非中断消息一律 None）
    while memory.interrupt_kinds.len() < msg_count {
        memory.interrupt_kinds.push(None);
    }

    // 最终对齐：如果仍有差异（不应发生），告警并强制对齐
    if memory.message_ids.len() != msg_count {
        eprintln!(
            "[MEMORY] message_ids 与 messages 长度不一致 ({} vs {})，强制对齐",
            memory.message_ids.len(),
            msg_count
        );
        memory.message_ids.truncate(msg_count);
        while memory.message_ids.len() < msg_count {
            memory.message_ids.push(uuid::Uuid::new_v4().to_string());
        }
    }
    memory.sources.truncate(msg_count);
    while memory.sources.len() < msg_count {
        memory.sources.push(MessageSource::Chat);
    }
    memory.interrupt_kinds.truncate(msg_count);
    while memory.interrupt_kinds.len() < msg_count {
        memory.interrupt_kinds.push(None);
    }
}

pub fn append_message(
    memory: &mut SessionMemory,
    message: Message,
    source: MessageSource,
) -> String {
    append_message_with_kind(memory, message, source, None)
}

/// 追加一条**带中断类型**的消息（中断收尾专用）。
///
/// `interrupt_kind` 与 `messages` / `sources` **严格平行**（见 `SessionMemory::interrupt_kinds`）；
/// 传 `None` 与 `append_message` 完全等价。中断收尾三条路径（用户取消 / 执行报错 /
/// 流空闲超时）、规划看门狗落库、崩溃恢复重建都走这里。
///
/// `source` 取枚举而不再是字符串：取值与两个可见域由 [`MessageSource`] 统一约束，
/// 调用点写错值会编译不过，而不是落库之后才在某个白名单里静默失配。
pub fn append_message_with_kind(
    memory: &mut SessionMemory,
    message: Message,
    source: MessageSource,
    interrupt_kind: Option<&str>,
) -> String {
    normalize_message_ids(memory);
    let message_id = uuid::Uuid::new_v4().to_string();
    memory.messages.push(message);
    memory.message_ids.push(message_id.clone());
    memory.sources.push(source);
    memory
        .interrupt_kinds
        .push(interrupt_kind.map(|k| k.to_string()));
    message_id
}

pub fn pop_message(memory: &mut SessionMemory) -> Option<(Message, String)> {
    normalize_message_ids(memory);
    let message = memory.messages.pop()?;
    let message_id = memory
        .message_ids
        .pop()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    memory.sources.pop();
    memory.interrupt_kinds.pop();
    Some((message, message_id))
}

pub fn restore_message(
    memory: &mut SessionMemory,
    message: Message,
    message_id: String,
    source: MessageSource,
) {
    restore_message_with_kind(memory, message, message_id, source, None)
}

/// 恢复一条消息（含可选中断类型）。恢复链若还原的是中断消息，
/// 必须把 kind 一并带回，否则界面侧会丢掉 notice、模型侧也会丢掉标记。
pub fn restore_message_with_kind(
    memory: &mut SessionMemory,
    message: Message,
    message_id: String,
    source: MessageSource,
    interrupt_kind: Option<&str>,
) {
    normalize_message_ids(memory);
    memory.messages.push(message);
    memory.message_ids.push(message_id);
    memory.sources.push(source);
    memory
        .interrupt_kinds
        .push(interrupt_kind.map(|k| k.to_string()));
}

pub fn reset_message_ids(memory: &mut SessionMemory) {
    // 只清理超出 messages 范围的多余 id，保留已有配对
    memory.message_ids.truncate(memory.messages.len());
    memory.sources.truncate(memory.messages.len());
    memory.interrupt_kinds.truncate(memory.messages.len());
    normalize_message_ids(memory);
}

/// 保存会话数据到 SQLite。
///
/// 持久化时的内容过滤规则（**注意与历史注释不同**，实际行为以此为准）：
/// - User 消息：保留 Text / Image / **ToolResult** / Context 块；
/// - Assistant 消息：保留 Text / Thinking / **ToolUse** 块；
/// - 仅丢弃空文本、空思考块与未知块。
///
/// 工具调用与工具结果**一直是保留的**——早期注释称"会过滤掉工具调用和工具结果"
/// 已与实现不符。保留它们是必须的：Assistant(ToolUse) 与其后的 ToolResult 成对出现，
/// 否则中断恢复后会构造出残缺配对（依赖 `fix_broken_tool_call_pairs` 兜底）。
pub fn save_session(
    id: &str,
    memory: &SessionMemory,
    token_usage_delta: Option<TokenUsageDelta>,
) -> SessionMeta {
    let meta = match repository::get_session_meta(id) {
        Ok(m) => m,
        Err(_) => {
            return SessionMeta {
                id: id.to_string(),
                title: String::new(),
                created_at: 0,
                updated_at: 0,
                message_count: 0,
                is_smart_named: false,
                profile_id: None,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cache_hit_tokens: 0,
                total_cache_miss_tokens: 0,
                title_source: default_title_source(),
                project_id: None,
                working_directory: None,
                thinking_mode: false,
                deleted_at: None,
                project_name: None,
            };
        }
    };
    let mut meta = meta;

    // ── 入库前对账（第三刀）：内存三平行 Vec（消息/编号/来源）必须等长。──
    // 失衡状态曾原样入库（10b68285：messages=0 / ids=7），下游全部失准。
    // normalize_message_ids 本可静默修复，但那无痕可查——先报出来再修，
    // 让"哪条路径写出了失衡数据"在日志里有据可查。
    if memory.messages.len() != memory.message_ids.len()
        || memory.messages.len() != memory.sources.len()
    {
        eprintln!(
            "[JARVIS][严重] save_session 三 Vec 失衡（session {}）：messages={} message_ids={} sources={}——以 messages 为准强制对齐后入库，请排查写出方",
            id,
            memory.messages.len(),
            memory.message_ids.len(),
            memory.sources.len()
        );
    }

    let mut normalized_memory = memory.clone();
    normalize_message_ids(&mut normalized_memory);

    let filtered_triples: Vec<(String, Message, MessageSource, Option<String>)> = normalized_memory
        .messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| {
            let message_id = normalized_memory.message_ids[idx].clone();
            // `normalize_message_ids` 已保证 sources 与 messages 等长，这里的兜底只为防御；
            // 缺来源按 `Chat` 补（理由同 `normalize_message_ids` 的补齐分支）
            let source = normalized_memory
                .sources
                .get(idx)
                .copied()
                .unwrap_or(MessageSource::Chat);
            // 中断类型随消息平行带出（`None` = 非中断消息）
            let interrupt_kind = normalized_memory
                .interrupt_kinds
                .get(idx)
                .cloned()
                .flatten();
            let filtered_message = match msg {
                Message::User { content } => match content {
                    Content::Single(_) => Some(msg.clone()),
                    Content::Multiple(blocks) => {
                        let filtered_blocks: Vec<ContentBlock> = blocks
                            .iter()
                            .filter(|b| {
                                matches!(
                                    b,
                                    ContentBlock::Text { .. }
                                        | ContentBlock::Image { .. }
                                        | ContentBlock::ToolResult { .. }
                                        | ContentBlock::Context { .. }
                                )
                            })
                            .map(|b| {
                                if let ContentBlock::Image { source } = b {
                                    // 落库前把图片本体转存进 `session_attachments` 表，消息记录里只留文件名。
                                    // 内存中的 base64 不回写磁盘，真正的字节在数据库。
                                    let file_path = if source.file_path.is_some() {
                                        source.file_path.clone()
                                    } else if !source.data.is_empty() {
                                        let fp = save_image_to_file(
                                            id,
                                            &source.media_type,
                                            &source.data,
                                        );
                                        Some(fp)
                                    } else {
                                        None
                                    };
                                    ContentBlock::Image {
                                        source: ImageSource {
                                            r#type: source.r#type.clone(),
                                            media_type: source.media_type.clone(),
                                            data: String::new(),
                                            file_path,
                                        },
                                    }
                                } else {
                                    b.clone()
                                }
                            })
                            .collect();
                        if filtered_blocks.is_empty() {
                            None
                        } else if filtered_blocks.len() == 1 {
                            if let ContentBlock::Text { text } = &filtered_blocks[0] {
                                Some(Message::User {
                                    content: Content::Single(text.clone()),
                                })
                            } else {
                                Some(Message::User {
                                    content: Content::Multiple(filtered_blocks),
                                })
                            }
                        } else {
                            Some(Message::User {
                                content: Content::Multiple(filtered_blocks),
                            })
                        }
                    }
                },
                Message::Assistant { content } => match content {
                    Content::Single(text) => {
                        if text.trim().is_empty() {
                            None
                        } else {
                            Some(Message::Assistant {
                                content: Content::Single(text.clone()),
                            })
                        }
                    }
                    Content::Multiple(blocks) => {
                        let text_blocks: Vec<ContentBlock> = blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } if !text.trim().is_empty() => {
                                    Some(b.clone())
                                }
                                ContentBlock::Thinking { thinking, .. }
                                    if !thinking.trim().is_empty() =>
                                {
                                    Some(b.clone())
                                }
                                ContentBlock::ToolUse { .. } => Some(b.clone()),
                                _ => None,
                            })
                            .collect();
                        if text_blocks.is_empty() {
                            None
                        } else {
                            Some(Message::Assistant {
                                content: Content::Multiple(text_blocks),
                            })
                        }
                    }
                },
            };
            // ⚠️ 带中断类型的消息**不得因正文为空被丢弃**：无产出中断
            // （纯思考期中止、应用关闭占位）正文本就是空的——阶段二起气泡锚点
            // 与小字都由 `interrupt_kind` 提供，丢弃会让该轮次从界面彻底消失。
            let filtered_message =
                filtered_message.or_else(|| interrupt_kind.as_ref().map(|_| msg.clone()));
            filtered_message.map(|message| (message_id, message, source, interrupt_kind))
        })
        .collect();
    let (filtered_message_ids, filtered_messages, filtered_sources, filtered_kinds): (
        Vec<String>,
        Vec<Message>,
        Vec<MessageSource>,
        Vec<Option<String>>,
    ) = filtered_triples.into_iter().fold(
        (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        |(mut ids, mut msgs, mut srcs, mut kinds), (id, msg, src, kind)| {
            ids.push(id);
            msgs.push(msg);
            srcs.push(src);
            kinds.push(kind);
            (ids, msgs, srcs, kinds)
        },
    );

    if let Some(delta) = token_usage_delta {
        meta.total_input_tokens = meta.total_input_tokens.saturating_add(delta.input);
        meta.total_output_tokens = meta.total_output_tokens.saturating_add(delta.output);
        meta.total_cache_hit_tokens = meta.total_cache_hit_tokens.saturating_add(delta.cache_hit);
        meta.total_cache_miss_tokens = meta
            .total_cache_miss_tokens
            .saturating_add(delta.cache_miss);
    }

    let new_count = filtered_messages.len();
    if new_count > meta.message_count {
        meta.updated_at = now_ts();
    }
    meta.message_count = new_count;
    if is_default_title_source(&meta.title_source) && !normalized_memory.messages.is_empty() {
        meta.title = extract_title(&normalized_memory.messages);
    }

    let filtered_memory = SessionMemory {
        messages: filtered_messages,
        message_ids: filtered_message_ids,
        sources: filtered_sources.clone(),
        interrupt_kinds: filtered_kinds.clone(),
        snapshot_seq: normalized_memory.snapshot_seq,
        plan_documents: normalized_memory.plan_documents.clone(),
    };

    repository::upsert_session(&meta, &filtered_memory)
        .unwrap_or_else(|err| panic!("保存 SQLite 会话 {} 失败: {}", id, err));
    let visible_sources: Vec<MessageSource> = filtered_memory.sources.clone();
    let visible_kinds: Vec<Option<String>> = filtered_memory.interrupt_kinds.clone();
    let (visible_message_ids, visible_messages): (Vec<String>, Vec<Message>) = filtered_memory
        .message_ids
        .iter()
        .cloned()
        .zip(filtered_memory.messages.iter().cloned())
        .unzip();
    // 同步前清理 session_messages 中已被压缩覆盖的孤儿行
    let _ = repository::hide_orphan_session_messages(id, &filtered_memory.message_ids);
    repository::append_or_upsert_session_messages(
        id,
        &visible_messages,
        &visible_message_ids,
        &visible_sources,
        &visible_kinds,
        meta.updated_at,
    )
    .unwrap_or_else(|err| panic!("保存 SQLite 会话历史 {} 失败: {}", id, err));
    let _ = repository::set_last_active_session_id(id);
    meta
}

/// 读取会话的用量账本（按来源分组的明细汇总）。
///
/// 数据源是 `request_usage` 明细表（v28），不是 `sessions.total_*`：
/// 后者只有四个总数，拆不出“谁烧的”。无明细的历史会话返回
/// `has_detail = false`，调用方据此回退到 `SessionMeta::total_*`。
pub fn get_session_usage_breakdown(session_id: &str) -> Result<UsageBreakdown, String> {
    repository::get_session_usage_breakdown(session_id)
}

/// 加载指定会话的完整数据
pub fn load_session(id: &str) -> Result<SessionMemory, String> {
    let memory = repository::load_session(id)?;
    repository::set_last_active_session_id(id)?;
    Ok(memory)
}

pub fn list_visible_session_messages(
    id: &str,
) -> Result<Vec<repository::StoredSessionMessage>, String> {
    repository::list_visible_session_messages(id)
}

/// 按轮凑页读取可见消息（懒加载）：一轮 = 一条可渲染 user + 其后所有消息，
/// 返回 (消息, 是否还有更早的轮)
pub fn load_visible_turns_page(
    id: &str,
    before_seq: Option<i64>,
    max_turns: usize,
) -> Result<(Vec<repository::StoredSessionMessage>, bool), String> {
    repository::load_visible_turns_page(id, before_seq, max_turns)
}

pub fn find_session_message_by_id(
    session_id: &str,
    message_id: &str,
) -> Result<Option<repository::StoredSessionMessage>, String> {
    repository::find_session_message_by_id(session_id, message_id)
}

pub fn hide_session_messages_from_seq(
    session_id: &str,
    seq: usize,
    recalled: bool,
) -> Result<(), String> {
    repository::hide_session_messages_from_seq(session_id, seq, recalled)
}

pub fn delete_session_messages_from_seq(session_id: &str, seq: usize) -> Result<(), String> {
    repository::delete_session_messages_from_seq(session_id, seq)
}

pub fn hide_orphan_session_messages(
    session_id: &str,
    alive_message_ids: &[String],
) -> Result<usize, String> {
    repository::hide_orphan_session_messages(session_id, alive_message_ids)
}

pub fn session_messages_count(session_id: &str) -> Result<usize, String> {
    repository::session_messages_count(session_id)
}

/// 一次查询取回可见消息的总条数与最新 seq：`(total_count, newest_seq)`。
///
/// 可见性口径与分页查询一致（hidden / recalled / compact 全部排除），
/// 供分页接口给前端补齐"后端总条数 + 后端最新 seq"，用于 stale 判定。
pub fn session_message_stats(session_id: &str) -> Result<(i64, Option<i64>), String> {
    repository::session_message_stats(session_id)
}

/// 广播「会话消息集合发生变化」。
///
/// 只在**可见消息条数或最新 seq 真的变了**时才发事件：同一轮里落库可能发生多次
/// （注入用户消息、模式切换、回合收尾），逐次广播会让前端反复标脏、白拉首屏。
/// 比较与去重收口在这里，调用方只需说明语义来源（`reason`）。
///
/// 为什么不挂进 `save_session`：它有 19 个调用点且签名不含 `AppHandle`，为发事件
/// 改签名会把风险扩散到所有落库路径；而且其中不少调用（token 累计、方案文档落库）
/// 并不改变可见消息集合。
///
/// 前端据此把非活跃会话标脏（`stale`），切回时只重载一次；活跃会话则等流式结束后
/// 由既有收尾路径消费。事件名与字段见前端 `useAgentEvents` 的同名监听。
pub fn notify_messages_changed(app: &tauri::AppHandle, session_id: &str, reason: &str) {
    use tauri::Emitter;

    type SessionStatsCache =
        std::sync::Mutex<std::collections::HashMap<String, (i64, Option<i64>)>>;

    /// 上一次广播时的 (可见条数, 最新 seq)，进程级缓存，用于抑制重复广播。
    /// 会话被归档/删除后留一条无用记录，代价可忽略；会话 id 不复用，不会误判。
    static LAST_SNAPSHOT: std::sync::OnceLock<SessionStatsCache> = std::sync::OnceLock::new();

    let stats = match session_message_stats(session_id) {
        Ok(v) => v,
        // 统计失败不广播：拿不到权威计数时宁可沉默，也不发一个错误信号。
        Err(_) => return,
    };

    let snapshots =
        LAST_SNAPSHOT.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = match snapshots.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.get(session_id).copied() == Some(stats) {
        return;
    }
    guard.insert(session_id.to_string(), stats);
    drop(guard);

    let _ = app.emit(
        "session-messages-changed",
        crate::infra::types::events::SessionMessagesChangedEvent {
            session_id: session_id.to_string(),
            count: stats.0,
            last_seq: stats.1,
            reason: reason.to_string(),
        },
    );
}

pub fn save_context_snapshot(
    snapshot: &crate::infra::types::models::SessionContextSnapshot,
) -> Result<(), String> {
    repository::upsert_context_snapshot(snapshot)
}

pub use repository::ContextSnapshotUsageUpdate;

pub fn update_context_snapshot_usage(
    session_id: &str,
    usage: ContextSnapshotUsageUpdate<'_>,
) -> Result<Option<crate::infra::types::models::SessionContextSnapshot>, String> {
    repository::update_context_snapshot_usage(session_id, usage)
}

pub fn get_context_snapshot(
    session_id: &str,
) -> Result<Option<crate::infra::types::models::SessionContextSnapshot>, String> {
    repository::get_context_snapshot(session_id)
}

/// 列出会话关联的计划文档（按更新时间倒序）
pub fn list_plan_documents(session_id: &str) -> Result<Vec<PlanDocument>, String> {
    let mut plans = load_session(session_id)?.plan_documents;
    plans.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
    Ok(plans)
}

pub fn upsert_plan_document(
    session_id: &str,
    mut document: PlanDocument,
) -> Result<PlanDocument, String> {
    let mut memory = load_session(session_id).unwrap_or_default();
    if document.session_id.is_empty() {
        document.session_id = session_id.to_string();
    }
    if document.created_at == 0 {
        document.created_at = now_ts();
    }
    document.updated_at = now_ts();

    if let Some(existing) = memory
        .plan_documents
        .iter_mut()
        .find(|item| item.id == document.id)
    {
        *existing = document.clone();
    } else {
        memory.plan_documents.push(document.clone());
    }

    save_session(session_id, &memory, None);
    Ok(document)
}

pub fn update_plan_document_status(
    session_id: &str,
    plan_id: &str,
    status: &str,
    content: Option<String>,
) -> Result<Option<PlanDocument>, String> {
    let mut memory = load_session(session_id).unwrap_or_default();
    let mut updated = None;
    let now = now_ts();

    if let Some(document) = memory
        .plan_documents
        .iter_mut()
        .find(|item| item.id == plan_id)
    {
        document.status = status.to_string();
        match status {
            // 拒绝：content 作为反馈意见存储，不覆盖原始方案
            "revision_requested" => {
                document.rejection_feedback = content;
            }
            // 批准（含编辑修改）：content 覆盖原始方案
            "approved" => {
                if let Some(content) = content {
                    document.content = content;
                }
            }
            _ => {}
        }
        document.updated_at = now;
        document.decided_at = Some(now);
        updated = Some(document.clone());
    }

    if updated.is_some() {
        save_session(session_id, &memory, None);
    }

    Ok(updated)
}

/// 获取单个会话的元信息（不受 message_count 过滤影响）
pub fn get_session_meta(id: &str) -> Result<SessionMeta, String> {
    repository::get_session_meta(id)
}

/// 归档会话（只打 `deleted_at` 标记，可从界面「归档」区取消）。
///
/// ⚠️ 这里**不能**清快照数据与回收站 —— 归档的意义就是"能取消"，
/// 取消之后回滚还要用它们（见 `repository::delete_session` 的说明）。
pub fn delete_session(id: &str) -> Result<(), String> {
    repository::delete_session(id)
}

/// **硬删除**会话，并清掉它的回滚侧产物。
///
/// 调用点只剩"用户明确要求彻底删除"：`command::session::purge_session`。
/// 曾经还有个"自动清理空会话"的调用点，已随归档改造移除 —— 归档不再按
/// "有没有内容"分流，空会话同样只打标记、留在「归档」区里。
///
/// 为什么必须显式清：`snapshot_trees` / `snapshot_content` 等表**没有 FK 级联**
/// （只有 `session_messages` 有 `ON DELETE CASCADE`），删了会话它们会永久留在库里。
pub fn hard_delete_session(id: &str) -> Result<(), String> {
    repository::hard_delete_session(id)?;
    purge_rollback_artifacts(id);
    Ok(())
}

/// 取消归档（它保留的消息 / 快照 / 回收站立即重新可用）。
pub fn restore_session(id: &str) -> Result<(), String> {
    repository::restore_session(id)
}

/// 已归档的会话列表（界面「归档」区入口用）。
pub fn list_deleted_sessions() -> Result<Vec<SessionMeta>, String> {
    repository::list_deleted_sessions()
}

/// 归档项目：项目本身 + 名下所有未归档会话在**同一事务**里打上归档时间戳。
///
/// 返回被归档的会话数；项目不存在或已归档时报错（此时事务整体回滚，
/// 不会留下「会话已归档、项目还在」的半成品状态）。
///
/// 与旧的 `delete_project` 硬删路径的区别：这里是**软删**，可从「归档」区撤销，
/// 所以快照与回收站产物一律保留（见 [`purge_rollback_artifacts`] 的说明）。
/// 硬删项目那条路径（删项目行 + 删名下会话行）已在 v27 随归档改造整体移除，
/// 项目不再有「真删」入口 —— 彻底删除只发生在会话级（[`hard_delete_session`] / `purge_sessions`）。
pub fn archive_project(project_id: &str, now: u64) -> Result<u64, String> {
    repository::archive_project(project_id, now as i64)
}

/// 取消归档项目（只清 `projects.deleted_at`，名下会话仍需逐条恢复）。
pub fn restore_project(project_id: &str) -> Result<(), String> {
    repository::restore_project(project_id)
}

/// 批量取消归档会话；若其所属项目也处于归档状态，同事务内一并复活。
pub fn restore_sessions(ids: &[String]) -> Result<BatchOpResult, String> {
    repository::restore_sessions(ids)
}

/// 批量**彻底删除**归档会话（行 + 回滚侧产物）。
pub fn purge_sessions(ids: &[String]) -> Result<BatchOpResult, String> {
    repository::purge_sessions(ids)
}

/// 归档会话总数（归档页角标）。
pub fn count_deleted_sessions() -> Result<usize, String> {
    repository::count_deleted_sessions()
}

/// 清掉某会话的回滚侧产物（快照数据 + 回收站目录）—— **只给硬删除路径调用**。
pub fn purge_rollback_artifacts(session_id: &str) {
    if let Err(e) =
        crate::core::rollback::store::SnapshotStore::new(session_id).delete_all_for_session()
    {
        eprintln!(
            "[Session] 清理已删会话 {} 的快照数据失败: {}",
            session_id, e
        );
    }
    crate::core::rollback::trash::purge_session(session_id);
}

/// 重命名会话
pub fn rename_session(
    id: &str,
    new_title: &str,
    is_auto_generated: bool,
) -> Result<SessionMeta, String> {
    let mut meta = repository::get_session_meta(id)?;
    if is_auto_generated {
        set_auto_title_source(&mut meta);
    } else {
        set_manual_title_source(&mut meta);
    }
    repository::rename_session(id, new_title, meta.is_smart_named, &meta.title_source)
}

/// 更新会话的模型预设
pub fn update_session_profile(id: &str, profile_id: &str) -> Result<(), String> {
    repository::update_session_profile(id, profile_id)
}

/// 更新会话的深度思考档位（布尔，落库 `1`/`0`）。
///
/// 调用方是"界面拨动后发消息"这一条路径，与工作模式/权限档位完全同构。
pub fn update_session_thinking_mode(id: &str, enabled: bool) -> Result<(), String> {
    repository::update_session_thinking_mode(id, enabled)
}

/// 读取会话的深度思考档位（布尔）。
pub fn get_session_thinking_mode(id: &str) -> Result<bool, String> {
    Ok(get_session_meta(id)?.thinking_mode)
}

/// 读取会话级运行偏好（工作模式 / 权限档位 / 用户类型）。
/// `None` = 用户在本会话从未表态，恢复方应回落到设置默认。
pub fn get_session_runtime_prefs(id: &str) -> Result<repository::SessionRuntimePrefs, String> {
    repository::get_session_runtime_prefs(id)
}

/// 落库会话的工作模式（用户切换 / 服务端自动切换共用）
pub fn update_session_work_mode(id: &str, mode: &str) -> Result<(), String> {
    repository::update_session_work_mode(id, mode)
}

/// 落库会话的权限档位
pub fn update_session_approval_mode(id: &str, mode: &str) -> Result<(), String> {
    repository::update_session_approval_mode(id, mode)
}

/// 落库会话的用户类型
pub fn update_session_agent_audience(id: &str, audience: &str) -> Result<(), String> {
    repository::update_session_agent_audience(id, audience)
}

/// 获取最后活跃的会话 ID
pub fn get_last_active_session_id() -> Option<String> {
    repository::get_last_active_session_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_extended_path_prefix() {
        assert_eq!(
            strip_extended_path_prefix(r"\\?\C:\Users\test\project"),
            r"C:\Users\test\project"
        );
    }

    #[test]
    fn keeps_normal_paths_unchanged() {
        assert_eq!(
            strip_extended_path_prefix(r"C:\Users\test\project"),
            r"C:\Users\test\project"
        );
        assert_eq!(
            strip_extended_path_prefix("/home/user/project"),
            "/home/user/project"
        );
        assert_eq!(strip_extended_path_prefix(""), "");
    }

    #[test]
    fn only_strips_the_exact_prefix() {
        // 前缀必须精确匹配，避免误伤普通以反斜杠开头的路径
        assert_eq!(
            strip_extended_path_prefix(r"\\?\UNC\server\share"),
            r"UNC\server\share"
        );
        assert_eq!(
            strip_extended_path_prefix(r"\?\C:\not_extended"),
            r"\?\C:\not_extended"
        );
    }
}

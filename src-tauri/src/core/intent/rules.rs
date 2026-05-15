//! # 意图分类规则 (Intent Classification Rules)
//!
//! 基于正则关键词的快速意图匹配引擎。
//! 定义了 12 种意图类型及其匹配规则，按优先级依次匹配：
//!
//! `Dangerous > Plan > Question > Action > Chat > Unclear`
//!
//! 三种核心函数：
//! - `classify_by_rules` — 纯规则匹配（第一层）
//! - `classify_with_context` — 带上下文的匹配（第二层）
//! - `analyze_last_assistant_message` — 上一轮助手消息特征提取

use regex::Regex;
use std::sync::LazyLock;

// ----------------------------------------------------------------------------
// 危险操作模式：匹配可能造成不可逆损害的操作
// 如：删除所有文件、清空数据库、格式化磁盘等
// ----------------------------------------------------------------------------
static DANGEROUS_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(删除|删掉|删除掉|删去|清空|清除|移除|卸载)\s*(所有|全部|整个|一切)",
        r"(?i)(delete|remove|clear|drop|truncate)\s*(all|everything|entire|whole)",
        r"(?i)格式化\s*(磁盘|硬盘|驱动器)",
        r"(?i)format\s*(disk|drive)",
        r"(?i)(清空|删除|删掉)\s*(数据库|项目|文件|目录|文件夹)",
        r"(?i)(drop|delete)\s*(database|table|schema)",
        r"(?i)rm\s+-rf",
        r"(?i)del\s+/\s*s",
        r"(?i)把.*删(了|掉|除)",
        r"(?i)删(了|掉)\s*它",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 复杂项目/方案审批关键词：匹配需要先规划再执行的项目级任务
// ----------------------------------------------------------------------------
static COMPLEX_TASK_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(先|先不要|不要直接).*(方案|计划|审批|审阅)",
        r"(?i)(提交|生成|制定|提出).*(方案|计划).*(审批|审阅|确认)",
        r"(?i)(完整|最小可用|MVP).*(项目|系统|应用|app|project|system)",
        r"(?i)(创建|新建|开发|实现|搭建).*(项目|系统|应用|前端|后端|API)",
        r"(?i)(前端|后端).*(接口|API|REST|数据库|数据存储)",
        r"(?i)(plan|proposal|approve|review).*(before|first|then)",
        r"(?i)(create|build|implement|develop).*(project|system|app|frontend|backend|api)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 代码读取关键词：匹配读文件、看代码、浏览等操作
// ----------------------------------------------------------------------------
static CODE_READ_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(读|读取|查看|打开|显示|展示|浏览)\s+[\w\.\-]+\.[a-z0-9]{1,8}",
        r"(?i)(读|查看|看|打开|显示|展示|浏览|搜|找)\s*(文件|代码|目录|文件夹|日志|配置)",
        r"(?i)(read|view|show|display|open|browse|catalog)\s*(file|code|dir|folder|log|config)",
        r"(?i)(cat|less|head|tail|ls|dir|type)",
        r"(?i)让我看看|给我看看|看看.*代码|看看.*文件",
        r"(?i)怎么写的|怎么实现的|什么内容",
        r"(?i)(内容|结构|骨架|摘要|大纲)\s*(是|是什么|长什么样)",
        r"(?i)(有哪些|哪些|什么|哪个)\s*(文件|目录|代码|模块)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 代码写入关键词：匹配创建、修改文件等操作
// ----------------------------------------------------------------------------
static CODE_WRITE_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(创建|新建|生成|写|编写|修改|编辑|重构|重写|替换|加上|去掉|改成|改为|添加|增加|删除|移除)",
        r"(?i)(create|write|modify|edit|refactor|rewrite|replace|add|remove|delete|update)",
        r"(?i)(改一下|换成|改成|改为|加上|去掉|添上|删掉|写入)",
        r"(?i)把.*(改|换|改|加|删|写)",
        r"(?i)帮我.*(写|改|建|创|修)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 代码审查关键词：匹配审查、找bug、优化建议等
// ----------------------------------------------------------------------------
static CODE_REVIEW_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(检查|审查|review|check|看看.*(有问题|对不对|好不好|行不行|合理|规范))",
        r"(?i)(bug|问题|错误|毛病|不对|不正常|有问题|隐患|风险)",
        r"(?i)(优化|改进|提升|改善|提高)\s*(性能|速度|效率|质量|代码)",
        r"(?i)(optimize|improve|enhance|refactor|review|check|inspect|audit)",
        r"(?i)(好像|似乎|感觉|觉得)\s*(不对|有问题|不合理|怪怪的)",
        r"(?i)(有没有|是否|是不是)\s*(问题|bug|隐患|风险|错误)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 任务执行关键词：匹配运行命令、启动服务等
// ----------------------------------------------------------------------------
static TASK_EXECUTE_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(运行|执行|启动|编译|构建|部署|打包|发布|安装|卸载|更新|升级|下载|上传)",
        r"(?i)(run|execute|start|launch|build|compile|deploy|install|uninstall|update|upgrade)",
        r"(?i)(测试|test|benchmark|profiling)",
        r"(?i)(git|npm|pip|cargo|yarn|pnpm|bun|mvn|gradle|docker|kubectl|helm)",
        r"(?i)(终端|命令行|shell|bash|cmd|powershell|terminal|console)",
        r"(?i)(npm\s+(run|install|build|test|start))",
        r"(?i)(cargo\s+(build|run|test|check|clippy))",
        r"(?i)(git\s+(push|pull|commit|checkout|merge|rebase|status|log))",
        r"(?i)(重启|restart|stop|停止|kill)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 问题咨询关键词：匹配技术问题、概念咨询
// ----------------------------------------------------------------------------
static QUESTION_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(什么是|什么叫|怎么用|怎么理解|为什么|如何|怎样|有没有|能不能)",
        r"(?i)(是什么|啥是|啥叫)",
        r"(?i)(what is|how (to|do|does)|why does|can you explain|difference between)",
        r"(?i)(是什么意思|怎么工作|原理是什么|什么原理|怎么实现)",
        r"(?i)(区别|差异|对比|compare|difference|vs|versus)",
        r"(?i)(想问一下|请教|请问|问个问题)",
        // 假设性问句：如果...会怎么、你会怎么做、应该怎么
        r"(?i)(如果|假如|假设|要是|万一).*(会怎么|怎么做|怎么办|如何)",
        r"(?i)(if|suppose|assuming).*(what would|how would|what should)",
        r"(?i)(你会|你应该|建议|推荐).*(怎么|如何|做)",
        r"(?i)(would you|should I|recommend|suggest).*(how|what)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 设置配置关键词：匹配修改设置、偏好的操作
// ----------------------------------------------------------------------------
static SETTINGS_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(设置|配置|偏好|选项|settings|config|preferences)",
        r"(?i)(想换|切换|换成|改为)\s*(模型|主题|语言|字体|皮肤)",
        r"(?i)(打开|显示|展示)\s*(设置|配置|面板|控制台)",
        r"(?i)(change|switch|set)\s*(model|theme|language|setting|config)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 记忆查询关键词：匹配询问历史对话的请求
// 如：之前讨论了什么、上次说的文件是什么
// ----------------------------------------------------------------------------
static MEMORY_QUERY_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        // 时间指示词
        r"(?i)(之前|以前|上次|上次我们|刚才|早些时候)",
        r"(?i)(previous|last time|earlier|before)",
        // 记忆动词
        r"(?i)(记得|记住|回忆|想起来)",
        r"(?i)(remember|recall|remind)",
        // 对话引用
        r"(?i)(我们(之前|以前)讨论|我们(之前|以前)说)",
        r"(?i)(we discussed|we talked about|we said)",
        // 历史记录
        r"(?i)(历史记录|聊天记录|对话记录)",
        r"(?i)(conversation history|chat history|conversation record|chat log)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 闲聊关键词：匹配日常对话、问候、玩笑等
// 如：你好、讲个笑话、今天天气怎么样
// ----------------------------------------------------------------------------
static GENERAL_CHAT_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        // 问候语（开头匹配）
        r"(?i)^(你好|哈喽|嗨|hi|hello|hey|早上好|晚上好|下午好)",
        r"(?i)^(谢谢|感谢|thanks|thank you)",
        r"(?i)^(再见|拜拜|bye|goodbye)",
        // 娱乐请求
        r"(?i)(讲个|说个|来个)\s*(笑话|故事|段子)",
        r"(?i)(tell|give)\s*(me\s*)?(a\s*)?(joke|story)",
        // 日常话题
        r"(?i)(天气|weather)",
        r"(?i)(怎么样|如何|what do you think)",
        // 情绪表达
        r"(?i)(哈哈|呵呵|嘿嘿|嘻嘻|lol|haha)",
        // 身份询问
        r"(?i)(你是谁|你叫什么|what is your name|who are you)",
        // 时间询问
        r"(?i)(今天|明天|昨天|this|next|yesterday|tomorrow)\s*(星期|周|几)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 肯定延续词：匹配用户确认/继续上一轮操作的短回复
// 如：继续、好的、可以、改一下
// 注意：这些词需要结合上下文判断是确认操作还是闲聊
// ----------------------------------------------------------------------------
static AFFIRMATIVE_CONTINUATION: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        // 确认词（精确匹配整词）
        r"(?i)^(继续|可以|需要|好|好的|行|没问题|是的|对|确认|确定|同意|ok|sure|yes|go ahead|continue|proceed)$",
        // 修改指令
        r"(?i)^(改一下|换成|加上|去掉|修改|调整)",
        // 后续指令
        r"(?i)^(然后呢|接下来|下一步|还需要|也要)",
        // 组合确认
        r"(?i)^(好的?[，,]?\s*(继续|改|换|加|删|做))",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

// ----------------------------------------------------------------------------
// 短输入模糊匹配：匹配过短或无意义的输入
// 如：单个数字、单个字母、纯标点符号
// ----------------------------------------------------------------------------
static SHORT_UNCLEAR: LazyLock<Regex> = LazyLock::new(|| {
    // 匹配：纯数字/纯标点 | 单个字母 | 非中文非字母数字的符号串
    Regex::new(r"^[\d\s\.\,\-\+\*]+$|^[a-zA-Z]$|^[^\w\s\u4e00-\u9fff]+$").unwrap()
});

// ----------------------------------------------------------------------------
// 开发/本地项目上下文：普通用户模式下用于收紧代码读写/审查规则。
// 例如“写一封邮件”“这里有问题”不应仅因出现“写/问题”就进入项目工具链。
// ----------------------------------------------------------------------------
static DEVELOPMENT_CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(代码|源码|文件|文件夹|目录|日志|函数|类|组件|项目|仓库|脚本|程序|应用|网站|网页|前端|后端|接口|数据库|依赖|测试|构建|编译|页面|界面|样式|逻辑|模块|终端|命令|工具|插件|软件|功能|code|source|file|folder|directory|log|function|class|component|project|repo|repository|script|program|app|application|website|webpage|frontend|backend|api|database|dependency|test|build|compile|page|ui|style|logic|module|terminal|command|tool|plugin|software|feature|\.[a-z0-9]{1,8}\b|[a-z]:\\|/[\w\.\-]+)",
    )
    .unwrap()
});

fn has_development_context(input: &str) -> bool {
    DEVELOPMENT_CONTEXT.is_match(input)
}

// ============================================================================
// 意图枚举类型
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Chat,       // 闲聊
    Question,   // 技术问题 + 记忆查询 + 设置
    Action,     // 代码读写/审查/命令执行/任务延续
    Plan,       // 复杂任务规划
    Dangerous,  // 危险操作
    Unclear,    // 不明确 + 需要上下文
}

impl Intent {
    pub fn from_str(s: &str) -> Option<Intent> {
        match s {
            "CHAT" | "GENERAL_CHAT" => Some(Intent::Chat),
            "QUESTION" | "MEMORY_QUERY" | "SETTINGS" => Some(Intent::Question),
            "CODE_READ" | "CODE_WRITE" | "CODE_REVIEW" | "TASK_EXECUTE" | "TASK_CONTINUE" | "PROJECT_ACTION" => Some(Intent::Action),
            "TASK_PLAN" => Some(Intent::Plan),
            "DANGEROUS" | "DANGEROUS_ACTION" => Some(Intent::Dangerous),
            "UNCLEAR" | "NEEDS_CONTEXT" => Some(Intent::Unclear),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Intent::Chat => "CHAT",
            Intent::Question => "QUESTION",
            Intent::Action => "ACTION",
            Intent::Plan => "TASK_PLAN",
            Intent::Dangerous => "DANGEROUS",
            Intent::Unclear => "UNCLEAR",
        }
    }
}

// ============================================================================
// 上下文信息结构体
// ============================================================================

/// 上一轮助手消息的分析结果
/// 用于判断用户的短回复（如"好的"、"继续"）的真实意图
#[derive(Debug, Clone)]
pub struct LastAssistantAction {
    /// 是否为项目操作（创建文件、运行命令等）
    pub was_project_action: bool,
    /// 是否在询问问题（需要用户回答）
    pub was_asking_question: bool,
    /// 是否在提出计划（需要用户确认）
    pub was_proposing_plan: bool,
    /// 操作摘要（用于调试日志）
    pub action_summary: String,
}

// ============================================================================
// 核心分类函数
// ============================================================================

/// 纯规则分类（第一层）
///
/// 仅基于关键词匹配，不考虑上下文。
/// 适用于明确的操作请求，如"创建文件"、"删除所有"。
///
/// # 参数
/// - `input`: 用户输入文本
///
/// # 返回
/// - 意图分类结果
pub fn classify_by_rules(input: &str) -> Intent {
    let trimmed = input.trim();
    let has_dev_context = has_development_context(trimmed);

    // 空输入直接判定为不明确
    if trimmed.is_empty() {
        return Intent::Unclear;
    }

    // 短输入（纯数字/单字母/纯符号）无法独立判断，交给上下文层
    // 匹配纯数字/单字母/纯符号等短输入
    // 这些输入可能是有意义的（如"1"=同意，"666"=厉害），
    // 返回 Unclear 让上下文层或 LLM 层判断
    if SHORT_UNCLEAR.is_match(trimmed) {
        return Intent::Unclear;
    }

    // 外部规则检查（JSON 配置的规则，覆盖/补充编译规则）
    if let Some(external) = check_external_rules(trimmed) {
        if let Some(intent) = Intent::from_str(&external) {
            return intent;
        }
    }

    // 优先级1：危险操作（最高优先级）
    for pattern in DANGEROUS_PATTERNS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Dangerous;
        }
    }

    // 优先级2：复杂项目/方案审批
    for pattern in COMPLEX_TASK_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Plan;
        }
    }

    // 优先级3：记忆查询
    for pattern in MEMORY_QUERY_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Question;
        }
    }

    // 优先级4：代码审查（"好像不对"、"有问题"等自然表达）
    let mut matched_dev_rule_without_context = false;
    for pattern in CODE_REVIEW_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            if has_dev_context {
                return Intent::Action;
            }
            matched_dev_rule_without_context = true;
            break;
        }
    }

    // 优先级5：代码读取
    for pattern in CODE_READ_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            if has_dev_context {
                return Intent::Action;
            }
            matched_dev_rule_without_context = true;
            break;
        }
    }

    // 优先级6：代码写入
    for pattern in CODE_WRITE_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            if has_dev_context {
                return Intent::Action;
            }
            matched_dev_rule_without_context = true;
            break;
        }
    }

    // 优先级7：问题咨询（包括假设性问句，优先于任务执行）
    for pattern in QUESTION_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Question;
        }
    }

    // 优先级8：任务执行（命令/脚本）
    for pattern in TASK_EXECUTE_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Action;
        }
    }

    // 优先级9：设置配置
    for pattern in SETTINGS_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Question;
        }
    }

    // 优先级10：肯定延续词（需要上下文才能确定）
    for pattern in AFFIRMATIVE_CONTINUATION.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Unclear;
        }
    }

    // 优先级11：闲聊关键词
    for pattern in GENERAL_CHAT_KEYWORDS.iter() {
        if pattern.is_match(trimmed) {
            return Intent::Chat;
        }
    }

    if matched_dev_rule_without_context {
        return Intent::Unclear;
    }

    // 默认：需要上下文或LLM判断
    Intent::Unclear
}

/// 带上下文的分类（第二层）
///
/// 在纯规则分类基础上，结合上一轮对话内容判断。
/// 主要解决"好的"、"继续"等短回复的歧义问题。
///
/// # 参数
/// - `input`: 用户输入文本
/// - `last_assistant_action`: 上一轮助手消息的分析结果
///
/// # 返回
/// - 意图分类结果
pub fn classify_with_context(
    input: &str,
    recent_assistant_actions: &[LastAssistantAction],
) -> Intent {
    for action in recent_assistant_actions {
        // 最近 N 轮中有提问且用户有回复 → 视为任务延续
        if action.was_asking_question && !input.trim().is_empty() {
            return Intent::Action;
        }

        // 最近 N 轮中有项目操作或计划提议，且用户回复确认词 → 任务延续
        if action.was_project_action || action.was_proposing_plan {
            for pattern in AFFIRMATIVE_CONTINUATION.iter() {
                if pattern.is_match(input.trim()) {
                    return Intent::Action;
                }
            }
        }
    }

    // 上下文未命中，回退到纯规则分类
    let base_intent = classify_by_rules(input);

    if base_intent != Intent::Unclear {
        return base_intent;
    }

    Intent::Unclear
}

/// 分析上一轮助手消息
///
/// 提取消息中的特征，用于判断用户的短回复意图。
///
/// # 参数
/// - `message`: 助手消息文本
///
/// # 返回
/// - 分析结果结构体
pub fn analyze_last_assistant_message(message: &str) -> LastAssistantAction {
    let lower = message.to_lowercase();

    // 关键词列表用于提取上一轮助手消息的行为特征
    // 项目操作指示词：创建、写入、修改、删除、运行等
    let project_action_indicators = [
        "创建", "写入", "修改", "删除", "运行", "执行", "构建", "安装", "create", "write",
        "modify", "delete", "run", "execute", "build", "install", "文件", "代码", "项目", "file",
        "code", "project", "命令", "终端", "command", "terminal",
    ];

    // 提问指示词：需要、是否、确认、请问等
    let question_indicators = [
        "需要",
        "是否",
        "确认",
        "请问",
        "想要",
        "need",
        "whether",
        "confirm",
        "would you like",
        "？",
        "?",
    ];

    // 计划指示词：计划、步骤、方案、建议等
    let plan_indicators = [
        "计划", "步骤", "方案", "建议", "plan", "step", "proposal", "suggest", "首先", "然后",
        "最后", "first", "then", "finally",
    ];

    // 检测各类特征
    let was_project_action = project_action_indicators
        .iter()
        .any(|ind| lower.contains(ind));

    let was_asking_question = question_indicators.iter().any(|ind| lower.contains(ind));

    let was_proposing_plan = plan_indicators.iter().any(|ind| lower.contains(ind));

    // 提取操作摘要（用于调试）
    let action_summary = if was_project_action {
        message.chars().take(100).collect()
    } else {
        String::new()
    };

    LastAssistantAction {
        was_project_action,
        was_asking_question,
        was_proposing_plan,
        action_summary,
    }
}

// ============================================================================
// 外部意图规则加载（JSON 配置文件扩展编译规则，无需重编译即可新增/调整意图）
// ============================================================================

use std::sync::RwLock;

/// 存储从 JSON 文件加载的外部规则：意图名 → 正则模式列表
static EXTERNAL_INTENT_RULES: RwLock<Vec<(String, Vec<Regex>)>> = RwLock::new(Vec::new());

/// 从 JSON 文件加载额外意图规则，与编译期规则同时生效
pub fn load_external_rules(path: &std::path::Path) -> std::io::Result<usize> {
    let content = std::fs::read_to_string(path)?;
    let rules: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut loaded = 0;
    let mut external = Vec::new();

    if let Some(intents) = rules["intents"].as_object() {
        // 按 priority_order 加载，保持优先级语义
        let order: Vec<&str> = rules["priority_order"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let mut keys: Vec<&str> = intents.keys().map(|k| k.as_str()).collect();
        // 排序：先按 priority_order，其余追加到末尾
        keys.sort_by_key(|k| {
            order
                .iter()
                .position(|o| o.eq_ignore_ascii_case(k))
                .unwrap_or(usize::MAX)
        });

        for key in &keys {
            if let Some(list) = intents[*key].as_array() {
                let mut patterns = Vec::new();
                for p in list {
                    if let Some(s) = p.as_str() {
                        if let Ok(re) = Regex::new(s) {
                            patterns.push(re);
                            loaded += 1;
                        }
                    }
                }
                if !patterns.is_empty() {
                    external.push((key.to_uppercase(), patterns));
                }
            }
        }
    }

    if let Ok(mut guard) = EXTERNAL_INTENT_RULES.write() {
        *guard = external;
    }
    Ok(loaded)
}

/// 检查外部规则，命中时返回意图标签字符串
fn check_external_rules(input: &str) -> Option<String> {
    if let Ok(guard) = EXTERNAL_INTENT_RULES.read() {
        for (name, patterns) in guard.iter() {
            for p in patterns {
                if p.is_match(input) {
                    return Some(name.clone());
                }
            }
        }
    }
    None
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_action() {
        assert_eq!(classify_by_rules("删除所有文件"), Intent::Dangerous);
        assert_eq!(classify_by_rules("清空数据库"), Intent::Dangerous);
        assert_eq!(classify_by_rules("rm -rf /"), Intent::Dangerous);
    }

    #[test]
    fn test_code_read() {
        assert_eq!(classify_by_rules("读取 main.rs"), Intent::Action);
        assert_eq!(classify_by_rules("看看这个文件"), Intent::Action);
        assert_eq!(classify_by_rules("打开日志看看"), Intent::Action);
    }

    #[test]
    fn test_code_write() {
        assert_eq!(classify_by_rules("帮我创建一个文件"), Intent::Action);
        assert_eq!(classify_by_rules("修改这个函数"), Intent::Action);
        assert_eq!(classify_by_rules("改一下这个逻辑"), Intent::Action);
    }

    #[test]
    fn test_complex_task_plan() {
        assert_eq!(
            classify_by_rules("请先提交一份可审批的实施方案，然后创建一个完整最小可用项目"),
            Intent::Plan
        );
        assert_eq!(
            classify_by_rules("在桌面创建一个包含前端和后端的任务管理系统"),
            Intent::Plan
        );
    }

    #[test]
    fn test_code_review() {
        assert_eq!(classify_by_rules("检查一下代码"), Intent::Action);
        assert_eq!(classify_by_rules("这段代码好像有问题"), Intent::Action);
        assert_eq!(classify_by_rules("这里好像有问题"), Intent::Unclear);
    }

    #[test]
    fn test_task_execute() {
        assert_eq!(classify_by_rules("运行测试"), Intent::Action);
        assert_eq!(classify_by_rules("git push"), Intent::Action);
        assert_eq!(classify_by_rules("npm install"), Intent::Action);
    }

    #[test]
    fn test_question() {
        assert_eq!(classify_by_rules("Rust的ownership是什么"), Intent::Question);
        assert_eq!(classify_by_rules("怎么用async/await"), Intent::Question);
        assert_eq!(classify_by_rules("想问一下这个库怎么用"), Intent::Question);
    }

    #[test]
    fn test_settings() {
        assert_eq!(classify_by_rules("帮我改一下配置"), Intent::Question);
        assert_eq!(classify_by_rules("切换模型"), Intent::Question);
        assert_eq!(classify_by_rules("打开设置面板"), Intent::Question);
    }

    #[test]
    fn test_memory_query() {
        assert_eq!(classify_by_rules("之前我们讨论了什么"), Intent::Question);
        assert_eq!(
            classify_by_rules("上次说的那个文件是什么"),
            Intent::Question
        );
        assert_eq!(classify_by_rules("这个项目有哪些文件"), Intent::Action);
    }

    #[test]
    fn test_general_user_content_creation_is_not_code_write() {
        assert_eq!(classify_by_rules("帮我写一封邮件"), Intent::Unclear);
        assert_eq!(classify_by_rules("修改一下这段文案"), Intent::Unclear);
        assert_eq!(classify_by_rules("帮我写一个脚本"), Intent::Action);
    }

    #[test]
    fn test_general_chat() {
        assert_eq!(classify_by_rules("你好"), Intent::Chat);
        assert_eq!(classify_by_rules("讲个笑话"), Intent::Chat);
        assert_eq!(classify_by_rules("谢谢"), Intent::Chat);
    }

    #[test]
    fn test_unclear() {
        assert_eq!(classify_by_rules(""), Intent::Unclear);
        assert_eq!(classify_by_rules("a"), Intent::Unclear);
    }

    #[test]
    fn test_needs_context() {
        assert_eq!(classify_by_rules("继续"), Intent::Unclear);
        assert_eq!(classify_by_rules("好的"), Intent::Unclear);
    }

    #[test]
    fn test_context_continuation() {
        let action = LastAssistantAction {
            was_project_action: true,
            was_asking_question: false,
            was_proposing_plan: false,
            action_summary: "创建文件".to_string(),
        };
        let actions = vec![action];
        assert_eq!(
            classify_with_context("继续", &actions),
            Intent::Action
        );
        assert_eq!(
            classify_with_context("好的", &actions),
            Intent::Action
        );
    }

    #[test]
    fn test_context_casual_chat() {
        let action = LastAssistantAction {
            was_project_action: false,
            was_asking_question: false,
            was_proposing_plan: false,
            action_summary: String::new(),
        };
        let actions = vec![action];
        assert_eq!(
            classify_with_context("好的", &actions),
            Intent::Unclear
        );
    }

    #[test]
    fn test_context_question_answer() {
        let action = LastAssistantAction {
            was_project_action: true,
            was_asking_question: true,
            was_proposing_plan: false,
            action_summary: "询问文件内容".to_string(),
        };
        let actions = vec![action];
        assert_eq!(
            classify_with_context("备忘txt", &actions),
            Intent::Action
        );
        assert_eq!(
            classify_with_context("test.md", &actions),
            Intent::Action
        );
    }
}

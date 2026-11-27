//! # app_config.rs — 应用本地配置（app-config.json）并发安全持久化
//!
//! 负责窗口状态、UI 偏好、技能激活、工具开关与 Shell 方言等配置的读写。
//! 通过进程内互斥锁串行化“读-改-写”闭环，写入采用同目录临时文件 + 原子替换，
//! 解析损坏时自动备份原文件而非静默覆盖。
//!
//! ## Key Exports
//! - `CustomWindowState` / `UiPreferences` / `AppConfigFile`: 应用配置数据模型
//! - `read_file()` / `update_file()`: 进程内并发安全的配置读取与原子更新入口
//! - `get_skill_activation()` / `get_all_skill_activations()` / `set_skill_activation()`: 技能开关读写
//! - `get_shell_settings()` / `save_shell_settings()`: Shell 方言配置读写
//! - `get_tool_state()` / `get_all_tool_states()` / `set_tool_enabled()`: 工具开关读写
//! - `get_window_state()` / `list_window_states()` / `save_window_state()` / `clear_window_states()`: 窗口状态读写
//! - `get_ui_preferences()` / `save_ui_preferences()`: UI 偏好自愈读取与落盘
//!
//! ## Dependencies
//! - Internal: `crate::infra::config::data_paths`, `crate::infra::shell_dialect`
//! - External: `serde`, `serde_json`, `ts_rs`
//!
//! ## Constraints
//! - 所有对 `app-config.json` 的读取与更新必须经过本模块互斥锁，避免并发覆盖和 Windows 句柄冲突

use crate::infra::config::data_paths;
use crate::infra::shell_dialect::ShellSettings;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 进程内全局互斥锁：串行化对 `app-config.json` 的读写，防止并发覆盖与 Windows 替换冲突。
static APP_CONFIG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 同进程临时文件/损坏备份后缀递增序号，避免同一毫秒内多次操作文件名碰撞。
static FILE_OP_SEQ: AtomicU64 = AtomicU64::new(0);

fn config_lock() -> &'static Mutex<()> {
    APP_CONFIG_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CustomWindowState {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub maximized: bool,
    pub fullscreen: bool,
    pub decorated: bool,
    #[ts(type = "number")]
    pub updated_at: u64,
}

/// UI 偏好设置（与前端 usePreferences.ts 对应）
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UiPreferences {
    #[serde(default = "default_font_size")]
    pub font_size: i32,
    #[serde(default = "default_code_font_size")]
    pub code_font_size: i32,
    #[serde(default = "default_true")]
    pub auto_scroll: bool,
    #[serde(default)]
    pub default_expand_thinking: bool,
    #[serde(default = "default_agent_panel_position")]
    pub agent_panel_position: String,
    /// 【已退役·保留字段不删】紧凑模式已从前端整体拆除。
    ///
    /// 1) 前端不再发送该字段：设置项开关、状态、文案已全部删除，`save_ui_preferences`
    ///    全量替换时这里只会回落为默认值 `false`；
    /// 2) 保留字段是为了老 app-config.json 能照常反序列化——结构体缺字段会直接报错，
    ///    而 `#[serde(default)]` 已兜住新配置里的缺省，二者缺一不可；
    /// 3) 与上面的 `code_font_size` 同一处理方式：前端不再发送、后端仍保留字段，
    ///    只为兼容历史配置文件。后端逻辑不读它（只负责透传存储），
    ///    可随下一次配置结构迁移一并删除。
    #[serde(default)]
    pub compact_mode: bool,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// 侧边栏「项目」总区块是否折叠（一次收起所有项目分组）。
    ///
    /// 与 `sidebar_collapsed`（整个侧边栏收起）是两个不同层次：本字段只管
    /// 会话列表里的项目分组。这组状态早期存在前端 localStorage，换机器或
    /// 清缓存就丢；现已并入 UI 偏好统一落库。
    #[serde(default)]
    pub sidebar_projects_collapsed: bool,
    /// 侧边栏「独立会话」区块是否折叠
    #[serde(default)]
    pub sidebar_standalone_collapsed: bool,
    /// 被折叠的项目分组 id 列表。
    ///
    /// 只存「折叠的」：默认展开，新增项目自动可见。用 id 而非名称——
    /// 项目可重命名，id 才是稳定标识。
    #[serde(default)]
    pub sidebar_collapsed_project_ids: Vec<String>,
    #[serde(default)]
    pub agent_panel_visible: bool,
    #[serde(default = "default_agent_audience")]
    pub agent_audience: String,
    #[serde(default = "default_agent_work_mode")]
    pub agent_work_mode: String,
    /// 权限档位："request_approval"（请求审批，默认）/ "auto_approve"（帮我批准）
    #[serde(default = "default_approval_mode")]
    pub agent_approval_mode: String,
    /// 深度思考默认档位（**设置态，三值**）：`"follow_global"`（跟随全局）/ `"on"` / `"off"`。
    ///
    /// 语义（2026-09-19 重构后）：
    /// - `follow_global`：**强制思考模型 → 开，非强制思考模型 → 关**（判据是模型能力
    ///   `thinking_forced`，与 `agent_audience` 已解耦）；
    /// - `on` / `off`：无条件开关。
    ///
    /// 注意：它**只在新建会话时被解析一次**并固化进 `sessions.thinking_mode`（布尔），
    /// 此后修改本字段**不会影响任何已有会话**——这是与"工作模式/权限档位"完全一致的
    /// 三值分治语义（设置态只喂新会话）。
    ///
    /// 该字段原先挂在 `profiles[].config.thinkingDefault`（模型预设级），但裁决层每轮
    /// 现读的是 `active_config()`——即"当前激活预设"，于是改任一副预设都会倒灌所有
    /// 会话。本次重构把它提到全局设置层，与权限/工作模式并列，并改为只在建会话时生效。
    #[serde(default = "default_thinking_default")]
    pub thinking_default: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    /// 「崩溃保护（实时保存）」开关——**纯全局、默认关**。
    ///
    /// 开：主 Agent 流式期间按攒批粒度（200ms / 1KB）把半截正文写进
    /// `agent_run_events`，进程崩溃/断电后能把"跑到一半的那一轮"恢复出来。
    /// 关：只在每轮收尾写一次——此时进程崩溃会丢掉**当前这一轮**已输出的内容
    ///（用户取消、上游断流不受影响，它们进程还活着，走内存态收尾）。
    ///
    /// 为什么默认关：正常路径下"崩溃丢半轮"是可接受的（用户重说一句即可），
    /// 而开启意味着给每个 SSE 帧加一条 SQLite 写的成本；把选择权交给用户，
    /// 默认保持最省的状态。
    ///
    /// 为什么是**全局**而非会话级：它防的是"进程突然没了"这种应用级故障，
    /// 与具体会话无关；做成会话级只会让用户在多会话间反复拨动却无收益。
    #[serde(default)]
    pub crash_protection: bool,
    /// 图片压缩档位："eco"（省流）/ "standard"（标准）/ "hd"（高清）。
    ///
    /// 档位是**用户的选择意图**，宽高数值是它的实现细节，两者一起落库：
    /// - 档位决定界面高亮哪一格，也决定了将来调整某档参数时该档用户跟随新参数；
    /// - 数值是压缩算法的直接输入，`TerminalInput.vue` 不需要再查表换算。
    ///
    /// 两者**必须同时写入**（前端 watcher 负责同步），只写其一会造成"显示档位与
    /// 实际压缩参数不一致"。`normalize_image_tier()` 负责在读取时自愈。
    #[serde(default = "default_image_compress_tier")]
    pub image_compress_tier: String,
    /// 图片压缩最大宽度（像素）。前端「省流/标准/高清」三档的数值形态，默认标准档 1568。
    #[serde(default = "default_image_max_width")]
    pub image_max_width: u32,
    /// 图片压缩最大高度（像素）
    #[serde(default = "default_image_max_height")]
    pub image_max_height: u32,
    /// 图片压缩质量 (0.0 ~ 1.0)。
    ///
    /// 刻意不暴露到界面：JPEG 质量只影响体积和肉眼看感，**不影响 token 数**
    /// （token 只由宽高决定），做成档位只会增加用户负担。
    #[serde(default = "default_image_quality")]
    pub image_quality: f32,
    /// 未发送的输入草稿：`sessionId → 原文`。
    ///
    /// 草稿是输入框的**编辑态**，不是会话的业务数据，因此不放进 `sessions` 表：
    /// 那张表是会话列表查询的主表，加长 TEXT 字段会拖累每次列表查询的行宽；
    /// 另外它也不需要按键级写库——只在切会话和发送完成时各写一次。
    ///
    /// 键用 `__new__` 表示「尚未落库的新会话」（`activeSessionId` 为 `None`）。
    /// 与 `sidebarCollapsedProjectIds` 同理，**整体替换**写入而不是原地改：偏好对象
    /// 是快照式落库的，原地改不利于前端 `watch` 比对。
    #[serde(default)]
    pub input_drafts: HashMap<String, String>,
    /// 向后兼容：读取旧 agent_display_mode 字段
    #[serde(default)]
    agent_display_mode: Option<String>,
    #[serde(default = "default_agent_opacity")]
    pub agent_message_opacity: i32,
    #[serde(default = "default_opacity")]
    pub user_message_opacity: i32,
}

fn default_font_size() -> i32 {
    15
}
fn default_code_font_size() -> i32 {
    13
}
fn default_true() -> bool {
    true
}
fn default_agent_panel_position() -> String {
    "right".to_string()
}
fn default_agent_audience() -> String {
    "developer".to_string()
}
fn default_agent_work_mode() -> String {
    "edit".to_string()
}
fn default_approval_mode() -> String {
    "request_approval".to_string()
}
/// 深度思考默认档位。默认 `follow_global`：跟随模型能力（强制思考模型开、其余关），
/// 这是"升级后行为最接近旧默认"的选择——旧默认在 DeepSeek 上正是开。
fn default_thinking_default() -> String {
    "follow_global".to_string()
}
fn default_locale() -> String {
    "zh-CN".to_string()
}
fn default_image_compress_tier() -> String {
    "standard".to_string()
}
/// 图片压缩默认档位 = **标准档**，与前端 `usePreferences.ts` 的 `IMAGE_COMPRESS_TIERS.standard` 同源。
///
/// 为什么是 1568×896 而不是 1920×1080：1568 是 Anthropic 服务端的长边硬上限，
/// 传更大的图既多花上传流量、又多耗 token，模型却仍然只看到 1568（约 1900 token/张 vs 2800）。
fn default_image_max_width() -> u32 {
    1568
}
fn default_image_max_height() -> u32 {
    896
}
fn default_image_quality() -> f32 {
    0.8
}
fn default_opacity() -> i32 {
    100
}
fn default_agent_opacity() -> i32 {
    0
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            font_size: 15,
            code_font_size: 13,
            auto_scroll: true,
            default_expand_thinking: false,
            agent_panel_position: "right".to_string(),
            compact_mode: false,
            sidebar_collapsed: false,
            sidebar_projects_collapsed: false,
            sidebar_standalone_collapsed: false,
            sidebar_collapsed_project_ids: Vec::new(),
            agent_panel_visible: false,
            agent_audience: "developer".to_string(),
            agent_work_mode: "edit".to_string(),
            thinking_default: default_thinking_default(),
            agent_approval_mode: "request_approval".to_string(),
            locale: "zh-CN".to_string(),
            crash_protection: false,
            image_compress_tier: "standard".to_string(),
            image_max_width: 1568,
            image_max_height: 896,
            image_quality: 0.8,
            input_drafts: HashMap::new(),
            agent_display_mode: None,
            agent_message_opacity: 0,
            user_message_opacity: 100,
        }
    }
}

impl UiPreferences {
    /// 兼容旧版本 agent_display_mode，迁移到双轴。
    ///
    /// 旧「普通用户」= 只读，因此迁移为 audience=user + 只读保护开启 + 编辑模式。
    pub fn migrate_legacy_display_mode(&mut self) {
        if let Some(legacy) = self.agent_display_mode.take() {
            if self.agent_audience == default_agent_audience()
                && self.agent_work_mode == default_agent_work_mode()
            {
                match legacy.as_str() {
                    "user" => {
                        self.agent_audience = "user".to_string();
                        self.agent_work_mode = "edit".to_string();
                    }
                    "developer" => {
                        self.agent_audience = "developer".to_string();
                        self.agent_work_mode = "edit".to_string();
                    }
                    _ => {}
                }
            }
        }
    }

    /// 兼容旧版本：`chat` 曾是用户可选的工作模式（只读保护），第二步起该模式取消，
    /// 迁移为"编辑模式 + 请求审批档"——安全等价：改动都会先问用户。
    pub fn migrate_legacy_work_mode(&mut self) {
        if self.agent_work_mode == "chat" {
            self.agent_approval_mode = default_approval_mode();
            self.agent_work_mode = "edit".to_string();
        }
        // 工作模式收敛为 edit / plan
        if !matches!(self.agent_work_mode.as_str(), "edit" | "plan") {
            self.agent_work_mode = "edit".to_string();
        }
        // 档位收敛
        if !matches!(
            self.agent_approval_mode.as_str(),
            "request_approval" | "auto_approve"
        ) {
            self.agent_approval_mode = default_approval_mode();
        }
        // 深度思考默认档位收敛（三值）：非法值回落 follow_global。
        // 注意兼容旧值 "auto" —— 它曾经挂在模型预设上，语义与 follow_global 最接近。
        if !matches!(
            self.thinking_default.as_str(),
            "follow_global" | "on" | "off"
        ) {
            self.thinking_default = default_thinking_default();
        }
    }

    /// 自愈图片压缩档位，保持"档位 ↔ 宽高数值"一致。
    ///
    /// 为什么要它：这两个字段来自前端两次独立的赋值，任何一种历史遗留都可能让它们对不上 ——
    /// 例如旧版本只存了数值（那时还没有档位字段）、或将来某档参数被调整。
    /// 处理原则是**以数值为准反推档位**，因为数值才是压缩算法实际用的东西，
    /// 反推出来的档位才是界面该显示的事实；数值对不上任何档时收敛到标准档并校正数值。
    pub fn normalize_image_tier(&mut self) {
        // (档位, 宽, 高)。与前端 `usePreferences.ts` 的 `IMAGE_COMPRESS_TIERS` 必须一致。
        const TIERS: [(&str, u32, u32); 3] = [
            ("eco", 1280, 720),
            ("standard", 1568, 896),
            ("hd", 1920, 1080),
        ];
        if let Some((tier, _, _)) = TIERS
            .iter()
            .find(|(_, w, h)| *w == self.image_max_width && *h == self.image_max_height)
        {
            self.image_compress_tier = (*tier).to_string();
            return;
        }
        // 数值不在任何档上（旧自定义值）：收敛到标准档，顺带把数值也校正
        self.image_compress_tier = default_image_compress_tier();
        self.image_max_width = default_image_max_width();
        self.image_max_height = default_image_max_height();
        self.image_quality = default_image_quality();
    }
}

/// 顶层配置文件结构（对应 `data/app-config.json`）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppConfigFile {
    #[serde(default)]
    pub(crate) windows: HashMap<String, CustomWindowState>,
    /// SessionManager 创建新会话 ctx 时读取默认设置（见 infra/state/state.rs）。
    #[serde(default = "UiPreferences::default")]
    pub(crate) ui_preferences: UiPreferences,
    /// 技能激活状态：skill_name → active。未记录的技能默认激活。
    #[serde(default)]
    pub(crate) skills: HashMap<String, bool>,
    /// Shell 执行方言（Windows 专用；非 Windows 恒为 bash）。
    ///
    /// 生效时机与工具开关一致：schema 与提示词都按方言生成，必须重启后统一生效，
    /// 不支持会话内热切换（见 infra/shell_dialect.rs 的模块注释）。
    #[serde(default)]
    pub(crate) shell: ShellSettings,
    /// 工具启用状态：tool_name → enabled。未记录的工具默认启用。
    ///
    /// 与 `skills` 同构：只存"被显式关掉的"，所以文件里通常只有零星几条。
    /// 关掉一个工具 = 模型**完全看不到它**（不进 `tools` 参数、按需工具目录里也搜不到），
    /// 不是"在、但调用被拒"那种软禁用。
    ///
    /// 注意：**生效时机与技能不同**：工具开关走**会话级快照**（见 SessionContext），
    /// 改完只对**新会话**生效 —— 因为核心工具的 schema 必须会话内字节恒定，
    /// 否则每轮 `tools` 参数一变，prompt cache 整体失效。
    /// 技能开关则立即生效（它只改运行时数据、不碰 schema）。
    #[serde(default)]
    pub(crate) tools: HashMap<String, bool>,
}

fn app_config_path() -> PathBuf {
    data_paths::app_config_path()
}

fn old_config_path() -> PathBuf {
    data_paths::data_root().join("window-state.json")
}

/// 备份损坏的非空配置文件，避免下一次写入静默覆盖导致历史配置彻底丢失。
///
/// 优先使用 `rename` 将损坏文件移走：既保留现场供恢复，又避免每次只读都重复复制出多份备份。
fn backup_corrupt_file(path: &Path, parse_err: &serde_json::Error) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = FILE_OP_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app-config.json");
    let backup_name = format!("{}.corrupt.{}.{}", file_name, millis, seq);
    let backup_path = path.with_file_name(backup_name);

    match fs::rename(path, &backup_path) {
        Ok(()) => {
            eprintln!(
                "[AppConfig] 配置文件解析失败（{}），已将损坏文件备份至 {:?} 并回落默认配置",
                parse_err, backup_path
            );
        }
        Err(rename_err) => {
            if let Err(copy_err) = fs::copy(path, &backup_path) {
                eprintln!(
                    "[AppConfig] 配置文件解析失败（{}），且备份损坏文件失败（rename: {}, copy: {}）",
                    parse_err, rename_err, copy_err
                );
            } else {
                eprintln!(
                    "[AppConfig] 配置文件解析失败（{}），已复制损坏文件至 {:?} 并回落默认配置",
                    parse_err, backup_path
                );
            }
        }
    }
}

/// 在调用方已持有互斥锁的前提下读取配置文件（支持旧 `window-state.json` 迁移与损坏备份）。
fn read_file_at_unlocked(path: &Path, old_path: Option<&Path>) -> AppConfigFile {
    // 旧文件迁移：如果新文件不存在但旧文件存在，重命名
    if !path.exists() {
        if let Some(old) = old_path {
            if old.exists() {
                let _ = fs::rename(old, path);
            }
        }
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) => {
            if err.kind() != ErrorKind::NotFound {
                eprintln!("[AppConfig] 读取 {:?} 失败（{}），回落默认配置", path, err);
            }
            return AppConfigFile::default();
        }
    };

    match serde_json::from_str::<AppConfigFile>(&content) {
        Ok(cfg) => cfg,
        Err(err) => {
            if !content.trim().is_empty() {
                backup_corrupt_file(path, &err);
            } else {
                eprintln!(
                    "[AppConfig] 配置文件 {:?} 为空（{}），回落默认配置",
                    path, err
                );
            }
            AppConfigFile::default()
        }
    }
}

/// 原子写入配置文件：先写入同目录唯一 `.tmp` 文件并 `sync_all`，再 `rename` 替换目标文件。
fn write_file_at_unlocked(path: &Path, file: &AppConfigFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建配置目录失败: {}", err))?;
    }
    let content =
        serde_json::to_string_pretty(file).map_err(|err| format!("序列化应用配置失败: {}", err))?;

    let seq = FILE_OP_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app-config.json");
    let tmp_path = path.with_file_name(format!("{}.{}.{}.tmp", file_name, std::process::id(), seq));

    // 先写临时文件并刷盘，确保目标文件绝不会出现半截内容
    let write_res = (|| -> std::io::Result<()> {
        let mut tmp_file = File::create(&tmp_path)?;
        tmp_file.write_all(content.as_bytes())?;
        tmp_file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("写入临时配置文件失败: {}", err));
    }

    // Windows 下目标文件偶发被杀毒/索引软件短暂占用，对 PermissionDenied 做短退避重试
    let mut last_err = None;
    for attempt in 0..4 {
        match fs::rename(&tmp_path, path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == ErrorKind::PermissionDenied && attempt < 3 => {
                last_err = Some(err);
                std::thread::sleep(Duration::from_millis(15 * (attempt + 1)));
            }
            Err(err) => {
                last_err = Some(err);
                break;
            }
        }
    }

    let _ = fs::remove_file(&tmp_path);
    Err(format!(
        "原子替换配置文件失败: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "未知错误".to_string())
    ))
}

/// 路径可注入的并发安全读取入口（便于单测在临时目录中验证）。
fn read_file_at(path: &Path, old_path: Option<&Path>, lock: &Mutex<()>) -> AppConfigFile {
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    read_file_at_unlocked(path, old_path)
}

/// 路径可注入的并发安全“读-改-写”闭环入口（全过程持互斥锁 + 原子落盘）。
fn update_file_at<F, R>(
    path: &Path,
    old_path: Option<&Path>,
    lock: &Mutex<()>,
    updater: F,
) -> Result<R, String>
where
    F: FnOnce(&mut AppConfigFile) -> R,
{
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = read_file_at_unlocked(path, old_path);
    let result = updater(&mut file);
    write_file_at_unlocked(path, &file)?;
    Ok(result)
}

/// 读取 `app-config.json`（进程内加锁同步小文件 IO）。
pub(crate) fn read_file() -> AppConfigFile {
    let path = app_config_path();
    let old = old_config_path();
    read_file_at(&path, Some(&old), config_lock())
}

/// 原子更新 `app-config.json`（在进程内互斥锁保护下完成读-改-写，临时文件原子替换）。
pub(crate) fn update_file<F, R>(updater: F) -> Result<R, String>
where
    F: FnOnce(&mut AppConfigFile) -> R,
{
    let path = app_config_path();
    let old = old_config_path();
    update_file_at(&path, Some(&old), config_lock(), updater)
}

// ── 技能激活状态 ──

/// 读取技能激活状态，未记录的技能默认激活
pub fn get_skill_activation(skill_name: &str) -> bool {
    let file = read_file();
    file.skills.get(skill_name).copied().unwrap_or(true)
}

/// 读取所有技能激活状态
pub fn get_all_skill_activations() -> HashMap<String, bool> {
    read_file().skills
}

/// 设置技能激活状态
pub fn set_skill_activation(skill_name: &str, active: bool) -> Result<(), String> {
    update_file(|file| {
        file.skills.insert(skill_name.to_string(), active);
    })
}

// ── Shell 执行方言 ──

/// 读取 Shell 设置（未配置时为 PowerShell 模式）
pub fn get_shell_settings() -> ShellSettings {
    read_file().shell
}

/// 保存 Shell 设置。只写配置，**不热切换**：需要重启应用后生效。
pub fn save_shell_settings(settings: ShellSettings) -> Result<(), String> {
    update_file(|file| {
        file.shell = settings;
    })
}

// ── 工具开关（与技能同构，但生效时机不同 —— 见 AppConfigFile.tools 的注释）──

/// 读取某个工具的启用状态，未记录的工具默认启用。
pub fn get_tool_state(tool_name: &str) -> bool {
    read_file().tools.get(tool_name).copied().unwrap_or(true)
}

/// 读取所有工具的启用状态（只含被显式设置过的条目）。
pub fn get_all_tool_states() -> HashMap<String, bool> {
    read_file().tools
}

/// 设置工具启用状态。
pub fn set_tool_enabled(tool_name: &str, enabled: bool) -> Result<(), String> {
    update_file(|file| {
        file.tools.insert(tool_name.to_string(), enabled);
    })
}

// ── 窗口状态 ──

pub fn get_window_state(label: &str) -> Option<CustomWindowState> {
    read_file().windows.get(label).cloned()
}

pub fn list_window_states() -> HashMap<String, CustomWindowState> {
    read_file().windows
}

pub fn clear_window_states() -> Result<(), String> {
    update_file(|file| {
        file.windows.clear();
    })
}

pub fn save_window_state(label: String, state: CustomWindowState) -> Result<(), String> {
    update_file(|file| {
        file.windows.insert(label, state);
    })
}

// ── UI 偏好设置 ──

pub fn get_ui_preferences() -> UiPreferences {
    let mut prefs = read_file().ui_preferences;
    prefs.migrate_legacy_display_mode();
    prefs.migrate_legacy_work_mode();
    prefs.normalize_image_tier();
    prefs
}

pub fn save_ui_preferences(mut incoming: UiPreferences) -> Result<(), String> {
    // 前端只会送 edit / plan；出现 chat 说明是旧版界面，按只读保护落库
    incoming.migrate_legacy_work_mode();
    // 落库前先自愈档位，防止前端漏送某一半字段时把不一致的状态写进磁盘
    incoming.normalize_image_tier();
    update_file(|file| {
        file.ui_preferences = incoming;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn prefs_with(tier: &str, w: u32, h: u32) -> UiPreferences {
        UiPreferences {
            image_compress_tier: tier.to_string(),
            image_max_width: w,
            image_max_height: h,
            ..UiPreferences::default()
        }
    }

    /// 数值落在某一档上 → 档位被反推成该档（数值是事实来源）
    #[test]
    fn tier_derived_from_dimensions() {
        let mut p = prefs_with("standard", 1920, 1080);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "hd");

        let mut p = prefs_with("standard", 1280, 720);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "eco");
    }

    /// 数值不在任何档上 → 收敛到标准档，且**数值被一起校正**（关键回归：
    /// 曾经用 Object.assign 写错了字段名，导致数值没改、还多出废字段）
    #[test]
    fn unknown_dimensions_fall_back_to_standard() {
        let mut p = prefs_with("hd", 1000, 600);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "standard");
        assert_eq!(p.image_max_width, 1568);
        assert_eq!(p.image_max_height, 896);
        assert_eq!(p.image_quality, 0.8);
    }

    /// 前端漏送档位字段时（serde 用默认值补齐），不会把已存的数值带偏
    #[test]
    fn missing_tier_field_does_not_disturb_dimensions() {
        let mut p = prefs_with(&default_image_compress_tier(), 1280, 720);
        p.normalize_image_tier();
        assert_eq!(p.image_compress_tier, "eco");
        assert_eq!(p.image_max_width, 1280);
        assert_eq!(p.image_max_height, 720);
    }

    /// 多线程并发更新不同字段时，互斥锁 + 原子写必须保留全部修改且无残留 .tmp 文件
    #[test]
    fn concurrent_updates_preserve_all_fields() {
        let dir = tempdir().expect("创建临时目录失败");
        let path = Arc::new(dir.path().join("app-config.json"));
        let lock = Arc::new(Mutex::new(()));

        let mut handles = Vec::new();
        for i in 0..12u32 {
            let path = Arc::clone(&path);
            let lock = Arc::clone(&lock);
            handles.push(std::thread::spawn(move || {
                update_file_at(&path, None, &lock, |cfg| {
                    cfg.skills.insert(format!("skill_{}", i), i % 2 == 0);
                    cfg.tools.insert(format!("tool_{}", i), i % 3 == 0);
                    cfg.ui_preferences
                        .input_drafts
                        .insert(format!("session_{}", i), format!("draft_{}", i));
                    cfg.windows.insert(
                        format!("win_{}", i),
                        CustomWindowState {
                            width: 800 + i,
                            height: 600 + i,
                            x: 10,
                            y: 20,
                            maximized: false,
                            fullscreen: false,
                            decorated: true,
                            updated_at: 1_700_000_000 + u64::from(i),
                        },
                    );
                })
                .expect("并发写入不应失败");
            }));
        }

        for h in handles {
            h.join().expect("工作线程不应 panic");
        }

        let loaded = read_file_at(&path, None, &lock);
        assert_eq!(loaded.skills.len(), 12);
        assert_eq!(loaded.tools.len(), 12);
        assert_eq!(loaded.ui_preferences.input_drafts.len(), 12);
        assert_eq!(loaded.windows.len(), 12);
        for i in 0..12u32 {
            assert_eq!(
                loaded.skills.get(&format!("skill_{}", i)),
                Some(&(i % 2 == 0))
            );
            assert_eq!(
                loaded.tools.get(&format!("tool_{}", i)),
                Some(&(i % 3 == 0))
            );
            assert_eq!(
                loaded
                    .ui_preferences
                    .input_drafts
                    .get(&format!("session_{}", i)),
                Some(&format!("draft_{}", i))
            );
            assert_eq!(
                loaded.windows.get(&format!("win_{}", i)).map(|w| w.width),
                Some(800 + i)
            );
        }

        // 不应残留任何临时 .tmp 文件
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("读取目录失败")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {:?}", leftovers);
    }

    /// 配置文件 JSON 损坏且非空时，必须先备份损坏文件再回落默认配置，避免后续写入永久覆盖
    #[test]
    fn corrupt_json_is_backed_up_instead_of_silent_overwrite() {
        let dir = tempdir().expect("创建临时目录失败");
        let path = dir.path().join("app-config.json");
        let lock = Mutex::new(());
        let broken_content = "{ \"uiPreferences\": { \"fontSize\": 18, ";
        fs::write(&path, broken_content).expect("写入损坏配置失败");

        // 首次读取触发备份并回落默认值
        let cfg = read_file_at(&path, None, &lock);
        assert_eq!(cfg.ui_preferences.font_size, 15);

        // 再次读取不应生成第二份重复备份
        let _ = read_file_at(&path, None, &lock);

        // 后续正常更新能恢复写入合法配置，同时损坏备份依然完好
        update_file_at(&path, None, &lock, |c| {
            c.skills.insert("demo".to_string(), false);
        })
        .expect("恢复写入不应失败");

        let restored = read_file_at(&path, None, &lock);
        assert_eq!(restored.skills.get("demo"), Some(&false));

        let backups: Vec<_> = fs::read_dir(dir.path())
            .expect("读取目录失败")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("app-config.json.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1, "应只生成一份损坏备份");
        let saved_broken = fs::read_to_string(backups[0].path()).expect("读取备份失败");
        assert_eq!(saved_broken, broken_content);
    }

    /// 空白配置文件不应生成多余的损坏备份文件
    #[test]
    fn empty_file_does_not_create_corrupt_backup() {
        let dir = tempdir().expect("创建临时目录失败");
        let path = dir.path().join("app-config.json");
        let lock = Mutex::new(());
        fs::write(&path, "   \n  ").expect("写入空白文件失败");

        let cfg = read_file_at(&path, None, &lock);
        assert_eq!(cfg.ui_preferences.font_size, 15);

        let backups: Vec<_> = fs::read_dir(dir.path())
            .expect("读取目录失败")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt."))
            .collect();
        assert!(backups.is_empty(), "空白文件不应产生损坏备份");
    }

    /// 旧版 window-state.json 存在时，首次读取应自动迁移至 app-config.json
    #[test]
    fn migrates_legacy_window_state_file() {
        let dir = tempdir().expect("创建临时目录失败");
        let path = dir.path().join("app-config.json");
        let old_path = dir.path().join("window-state.json");
        let lock = Mutex::new(());

        let legacy_json = r#"{
            "windows": {
                "main": {
                    "width": 1200,
                    "height": 800,
                    "x": 100,
                    "y": 50,
                    "maximized": false,
                    "fullscreen": false,
                    "decorated": true,
                    "updatedAt": 123456
                }
            }
        }"#;
        fs::write(&old_path, legacy_json).expect("写入旧配置失败");

        let cfg = read_file_at(&path, Some(&old_path), &lock);
        assert!(path.exists());
        assert!(!old_path.exists());
        assert_eq!(cfg.windows.get("main").map(|w| w.width), Some(1200));
    }
}

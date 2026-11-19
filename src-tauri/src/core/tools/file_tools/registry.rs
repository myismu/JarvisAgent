//! # registry.rs — 注册 file_tools 提供给 Agent 的工具定义
//!
//! 通过 `define_tools!` 宏声明文件读取、写入、编辑、搜索和目录列表工具的 schema、搜索提示、只读属性与并发安全属性。
//!
//! ## Key Exports
//! - `register_tools()`: 向工具注册表加入 file_tools 的可调用工具定义
//!
//! ## Dependencies
//! - Internal: `crate::core::tools::framework::registry::ToolDef`, `crate::define_tools!`
//! - External: `serde_json`
//!
//! ## 工具分类
//! - **核心工具**（should_defer: false）：ReadFile, SearchRepo, SymbolSearch, ListDirectory
//! - **按需工具**（should_defer: true）：WriteFile, EditFile, ApplyPatch, ReadFileSkeleton, DeleteFile, RenameFile
//!
//! ## 设计决策
//! - 写操作工具（WriteFile, EditFile）设为按需工具，防止聊天模式下误操作
//! - 只读工具设为核心工具，保证 prompt cache 命中率

use serde_json::json;

use crate::core::tools::framework::registry::ToolDef;

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "ReadFile",
            description: "读取文件内容，支持按行号精确读取",
            search_hint: "read file content view",
            category: "文件操作",
            schema: json!({
                "name": "ReadFile",
                // 二进制黑名单与各类文件的处理建议从提示词搬来（2026-09-21）：
                // 这段原在 base_p0.md 的「禁止读取二进制/压缩文件」整节，属 ReadFile 单工具的
                // 用法边界，放这里才对（ReadFile 是核心工具，schema 常驻，模型随时看得到）。
                // 搬来时修掉了三条描述不存在能力的说法：图片渲染、pages 参数、数据库工具
                // —— 详见 common.rs 里 binary_file_read_error 的报错文案注释。
                "description": "读取文件内容。支持语义化点读技术，可通过 start_line 和 end_line 获取特定代码块，避免 Context 过长。\n\n绝对禁止读取二进制/压缩文件（.exe/.dll/.pdb/.zip/.gz/.tar/.png/.pdf/.db 等）：这些扩展名已被系统列入黑名单，调用会直接失败并给出替代建议；违规读取会污染上下文、损坏会话。按类型处理：压缩包用 RunCommand 解压、编译产物改读对应源码、图片把文件直接发送到对话里；PDF 与数据库文件不支持按文本读取。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要读取的文件路径（必填）"},
                        "start_line": {"type": "integer", "description": "可选。起始行号（从 1 开始）"},
                        "end_line": {"type": "integer", "description": "可选。结束行号（包含）"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: false,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "ReadFileSkeleton",
            description: "提取文件结构骨架（类、函数签名及行号）",
            search_hint: "skeleton structure class function signature",
            category: "文件操作",
            schema: json!({
                "name": "ReadFileSkeleton",
                "description": "提取文件结构骨架（Skeleton）。快速扫描并返回文件的类、函数签名及其行号，结合 read_file 进行精确片段阅读。",
                "input_schema": {
                    "type": "object",
                    "properties": { "path": {"type": "string", "description": "要提取结构骨架的文件路径（必填）"} },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "WriteFile",
            description: "写入普通文本文件内容；不得用于 .ipynb/Jupyter Notebook JSON",
            search_hint: "write file create new",
            category: "文件操作",
            schema: json!({
                "name": "WriteFile",
                // 判据取并集（2026-09-21）：schema 原先只按"用户明确要求 / 文件很小且读过"，
                // 提示词按"改动约占七成以上"——两者是不同的判据，模型看两处会犯迷糊。现在都列。
                "description": "写入普通文本文件内容。优先用于创建新文件；修改已有文件时应优先使用 EditFile，除非用户明确要求重写整个文件、或文件很小且已经完整读取过、或文件绝大部分内容（约七成以上）确实要重写。只改几处就把整个文件重写一遍是违规（费 token、慢、容易顺手改动无关内容）。大量修改应使用 ApplyPatch。不要用于 .ipynb/Jupyter Notebook 或 notebook-shaped JSON；Notebook 必须使用 notebook_edit 进行 cell 级 replace/insert/delete。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要写入的文件路径（必填）"},
                        "content": {"type": "string", "description": "要写入的完整文件内容（必填）"}
                    },
                    "required": ["path", "content"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "EditFile",
            description: "基于搜索与替换修改普通文本；不得用于 .ipynb/Jupyter Notebook JSON",
            search_hint: "edit file replace search modify",
            category: "文件操作",
            schema: json!({
                "name": "EditFile",
                // 补上"匹配失败之后怎么办"（2026-09-21 从提示词搬来）：原先只写了失败的
                // 表现（返回所有匹配位置），没说补救路径，模型容易误以为该改用 WriteFile 整写。
                "description": "基于搜索与替换修改文件中的文本片段。支持两种模式：单条编辑（old_text+new_text）和批量编辑（edits 数组）。每段 old_text 必须在文件中唯一匹配（3~5 行上下文），否则返回所有匹配位置。匹配失败 ≠ 该改用 WriteFile：失败只说明 old_text 与当前文件内容对不上，重新 ReadFile 拿准确内容再改即可，不要因匹配失败改用整文件覆盖。同一文件多处修改优先用 edits 数组批量提交，不要逐个调用。小范围单点可继续用 old_text+new_text。跨文件修改用 ApplyPatch。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要编辑的文件路径（必填）"},
                        "old_text": {"type": "string", "description": "单条编辑：要替换的原文片段，必须包含 3~5 行上下文确保唯一匹配"},
                        "new_text": {"type": "string", "description": "单条编辑：替换后的新内容"},
                        "replace_all": {"type": "boolean", "description": "单条编辑：是否替换所有匹配项。默认 false"},
                        "edits": {
                            "type": "array",
                            "description": "批量编辑：多条替换的列表。优先于 old_text/new_text。每个元素包含 {old_text, new_text, replace_all?}",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_text": {"type": "string", "description": "要替换的原文片段"},
                                    "new_text": {"type": "string", "description": "替换后的新内容"},
                                    "replace_all": {"type": "boolean", "description": "是否替换所有匹配项。默认 false"}
                                },
                                "required": ["old_text", "new_text"]
                            }
                        }
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "ApplyPatch",
            description: "事务式应用 unified diff 多文件补丁",
            search_hint: "apply patch diff multi file hunk dry run preview edit",
            category: "文件操作",
            schema: json!({
                "name": "ApplyPatch",
                "description": "事务式应用 unified diff / *** Begin Patch 格式补丁。适合多 hunk、多文件复杂修改；小范围单点替换优先使用 EditFile。支持 dry_run 预览；仍会执行 workspace 权限校验、Notebook 拦截、编码保留、TOCTOU 检查和快照记录。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": "unified diff 或 *** Begin Patch 格式补丁文本"},
                        "dry_run": {"type": "boolean", "description": "仅预检和预览，不写入文件。默认 false"}
                    },
                    "required": ["patch"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "SearchRepo",
            description: "在指定目录下全局搜索包含关键词的文本",
            search_hint: "search find grep text pattern",
            category: "搜索检索",
            schema: json!({
                "name": "SearchRepo",
                "description": "在指定目录下全局搜索包含特定关键词或正则表达式的文本内容。自动忽略编译产物和静态资源。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "要搜索的关键词或正则表达式"},
                        "dir": {"type": "string", "description": "要搜索的目录路径，默认搜索整个项目根目录"},
                        "regex": {"type": "boolean", "description": "是否将 pattern 作为正则表达式处理，默认 false"},
                        "case_insensitive": {"type": "boolean", "description": "是否忽略大小写，默认 false"},
                        "limit": {"type": "integer", "description": "最大返回匹配数，默认 50，最大 500"},
                        "context_lines": {"type": "integer", "description": "每个匹配前后返回的上下文行数，默认 0，最大 10"},
                        "include": {"type": "string", "description": "可选。include glob 过滤，例如 src/**/*.rs 或 *.{ts,tsx}"},
                        "exclude": {"type": "string", "description": "可选。exclude glob 过滤，例如 **/*.test.ts 或 dist/**"},
                        "type": {"type": "string", "description": "可选。文件类型过滤，例如 rust、typescript、vue、json"},
                        "ignore_dirs": {"type": ["string", "array"], "items": {"type": "string"}, "description": "可选。额外忽略目录，支持逗号/空格分隔字符串或字符串数组"}
                    },
                    "required": ["pattern"]
                }
            }),
            should_defer: false,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "SymbolSearch",
            description: "符号搜索：查定义、查引用、读代码块（operation 分模式）",
            search_hint: "symbol search definition references block find read 符号 定义 引用 代码块",
            category: "搜索检索",
            schema: json!({
                "name": "SymbolSearch",
                "description": "符号搜索统一入口，按 operation 分三种模式：definition 在目录中查找符号定义候选（返回文件、行号、签名、置信度）；references 查找符号的引用/导入导出；block 读取指定文件中符号所在的完整代码块。基于扩展名与轻量文本规则识别，非语义级分析。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "operation": {"type": "string", "enum": ["definition", "references", "block"], "description": "查询类型：definition 查定义、references 查引用、block 读代码块"},
                        "symbol": {"type": "string", "description": "要查询的符号名"},
                        "path": {"type": "string", "description": "operation=block 时必填：符号所在文件路径"},
                        "dir": {"type": "string", "description": "可选。搜索目录，默认项目根目录（definition/references 用）"},
                        "kind": {"type": "string", "enum": ["function", "class", "type", "component", "variable", "any"], "description": "可选。符号类型过滤，默认 any（仅 definition 用）"},
                        "limit": {"type": "integer", "description": "最大返回结果数：definition 默认 50/上限 200，references 默认 100/上限 500"},
                        "include": {"type": "string", "description": "可选。include glob 过滤，例如 src/**/*.rs"},
                        "exclude": {"type": "string", "description": "可选。exclude glob 过滤"},
                        "type": {"type": "string", "description": "可选。文件类型过滤，例如 rust、typescript、vue"},
                        "ignore_dirs": {"type": ["string", "array"], "items": {"type": "string"}, "description": "可选。额外忽略目录"}
                    },
                    "required": ["operation", "symbol"]
                }
            }),
            should_defer: false,
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "ListDirectory",
            description: "列出目录当前一层的内容（非递归；跳过依赖/构建目录，最多 200 项）",
            search_hint: "list directory folder files ls",
            category: "文件操作",
            should_defer: false,
            schema: json!({
                "name": "ListDirectory",
                "description": "列出指定目录当前一层的内容（非递归）。依赖/构建目录（node_modules、target、dist）会被跳过；超过 200 项时截断并提示；需要按名称/扩展名筛选时用 FindFiles。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "目录路径"}
                    },
                    "required": ["path"]
                }
            }),
            is_read_only: true,
            is_concurrency_safe: true,
            is_enabled: true,
        },
        ToolDef {
            name: "DeleteFile",
            description: "删除指定文件或目录（目录整棵一起）",
            search_hint: "delete file remove rm directory folder 目录 文件夹",
            category: "文件操作",
            schema: json!({
                "name": "DeleteFile",
                // 「可删目录」必须写在描述里：模型会望文生义把 DeleteFile 当"只能删文件"，
                // 然后一个文件一个文件地掏空目录、再去用 Remove-Item 删目录壳，被安全策略拦下，
                // 白白空转（2026-09-21 实测，长任务会话 59293891 #632/#643-652）。
                // 实现是"整个对象 rename 进回收站"，目录、非空目录都是一次搬走。
                "description": "删除指定的**文件或目录**。删目录时是**整棵一起删**（含非空目录与其中全部内容），不需要先逐个清空再删壳。删除前会自动记录快照用于回滚，实体则移入回收站，因此可恢复。注意：不要用 RunCommand 的 Remove-Item / del / rmdir 删除——shell 删除会被安全策略直接拒绝。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要删除的文件或目录路径"}
                    },
                    "required": ["path"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
        ToolDef {
            name: "RenameFile",
            description: "重命名或移动指定文件",
            search_hint: "rename move file mv ren",
            category: "文件操作",
            schema: json!({
                "name": "RenameFile",
                "description": "重命名或移动指定文件到新路径。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "要重命名的文件路径"},
                        "new_path": {"type": "string", "description": "新的文件路径"}
                    },
                    "required": ["path", "new_path"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        }
    }
}

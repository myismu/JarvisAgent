---
name: tool-schema-declarative-macro
overview: 创建声明宏 tool_def! 替代手写 json! Schema，简化所有工具注册代码
todos:
  - id: define-tool-def-macro
    content: 在 registry.rs 中定义 tool_def! 声明宏，支持简单属性、enum、array、嵌套 items、动态表达式
    status: in_progress
  - id: migrate-simple-tools
    content: 改造简单工具注册：file_tools/registry.rs、system_tools.rs、notebook_tools.rs、tool_search.rs（共10个工具）
    status: pending
    dependencies:
      - define-tool-def-macro
  - id: migrate-complex-tools
    content: 改造复杂工具注册：shell_tools.rs（动态desc）、claude_code_tools.rs（非标准属性名）、agent_tools.rs（动态enum）（共12个工具）
    status: pending
    dependencies:
      - define-tool-def-macro
  - id: migrate-task-tools
    content: 改造 task_tools 下的7个 tool_def() 函数：todo_write.rs 及 persistent/ 下6个文件
    status: pending
    dependencies:
      - define-tool-def-macro
  - id: verify-and-test
    content: 编译验证并运行 cargo test，确保所有工具 schema 与改造前完全一致
    status: pending
    dependencies:
      - migrate-simple-tools
      - migrate-complex-tools
      - migrate-task-tools
---

## Product Overview

为 JarvisAgent 后端工具系统创建声明宏 `tool_def!`，替代当前冗长的 `json!({...})` + `ToolDef { ... }` 手写模式，简化所有工具注册代码。

## Core Features

- 定义 `tool_def!` 声明宏，自动从 ToolDef 的 name/description 生成 schema 中的重复字段
- 支持简单属性（string/integer/boolean）和复杂属性（enum/items/嵌套对象）
- 支持运行时动态值（如 `shell_tool_description()`）
- 兼容非标准属性名（如 `-B`/`-A`/`-C`）
- 将 `is_enabled` 默认为 true，可省略
- 改造全部 22+ 个工具定义，统一使用新宏

## Tech Stack

- 语言：Rust
- 宏类型：`macro_rules!` 声明宏
- 修改范围：`src-tauri/src/core/tools/` 下所有工具注册文件

## Implementation Approach

### 宏设计策略

采用"声明宏生成 `ToolDef` + `json!`"的策略：宏展开后生成的代码与当前手写代码结构完全一致，确保零行为变化。宏只是消除重复、减少括号嵌套。

### 宏语法设计

**改造前（当前写法，约30行/工具）**：

```rust
ToolDef {
    name: "read_file",
    description: "读取文件内容",
    search_hint: "read file content view",
    schema: json!({
        "name": "read_file",              // 重复
        "description": "读取文件内容...",   // 重复
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "文件路径"},
                "start_line": {"type": "integer", "description": "起始行号"},
            },
            "required": ["path"]
        }
    }),
    should_defer: true,
    is_read_only: true,
    is_concurrency_safe: true,
    is_enabled: true,
}
```

**改造后（新宏写法，约10行/工具）**：

```rust
tool_def!("read_file",
    desc: "读取文件内容",
    hint: "read file content view",
    props: {
        path: string => "文件路径",
        start_line: integer => "起始行号",
    },
    required: ["path"],
    defer: true,
    read_only: true,
    concurrency_safe: true,
)
```

### 复杂属性语法

对于特殊属性，提供扩展语法：

```rust
// enum 属性
status: string enum ["pending", "in_progress", "completed"] => "当前状态"

// 数组属性（简单元素类型）
args: array of string => "参数列表"

// 嵌套 items（todo_write 的场景）
todos: array items {
    id: string => "项目ID",
    content: string => "待办内容",
    activeForm: string => "进行中显示文本",
    status: string enum ["pending", "in_progress", "completed"] => "状态",
} required ["content", "activeForm", "status"] => "待办列表"

// 非标准属性名（grep 的 -B/-A/-C）
"-B": integer => "匹配前显示行数",
"-A": integer => "匹配后显示行数",
"-C": integer => "上下文行数",

// 运行时动态 description（run_shell）
// 使用 raw_json 语法，直接嵌入表达式
desc: expr shell_tool_description(),

// 运行时动态 enum 值（task 工具）
subagent_type: string enum expr AgentRegistry::global().available_types() => "代理类型"
```

### 宏展开原理

`tool_def!` 宏的核心工作：

1. 将 `name` 参数同时填入 `ToolDef.name` 和 `schema.name`
2. 将 `desc` 同时填入 `ToolDef.description` 和 `schema.description`
3. 将 `props` 块展开为 `input_schema.properties` 的 JSON 对象
4. 将 `required` 数组填入 `input_schema.required`
5. 将各布尔标志填入 ToolDef 对应字段，`is_enabled` 默认 true

### 不改造 `define_tools!` 宏

现有的 `define_tools!` 宏保持不变，它负责批量注册。改造后 `define_tools!` 内部的 `ToolDef { ... }` 替换为 `tool_def!(...)` 调用即可。

## Implementation Notes

- **向后兼容**：宏展开结果必须与当前手写代码生成的 JSON 结构完全一致，通过对比测试验证
- **分步迁移**：先实现宏并改造简单工具，再处理复杂工具（todo_write、run_shell、task）
- **保留 task_tools 的独立 tool_def() 函数模式**：这些文件中 tool_def() 与执行逻辑在同一个文件，保持其结构不变，只替换内部 json! 为宏调用
- **动态值处理**：运行时生成的 description 和 enum 值通过 `expr` 类型的宏参数传入，宏本身不执行计算

## Directory Structure

```
src-tauri/src/core/tools/
├── registry.rs                          # [MODIFY] 新增 tool_def! 宏定义，ToolDef 结构体不变
├── file_tools/
│   └── registry.rs                      # [MODIFY] 6个 ToolDef 替换为 tool_def! 调用
├── shell_tools.rs                       # [MODIFY] 4个 ToolDef 替换（含动态 desc 特殊处理）
├── agent_tools.rs                       # [MODIFY] 6个 ToolDef 替换（含 task 的动态 enum）
├── system_tools.rs                      # [MODIFY] 2个 ToolDef 替换
├── claude_code_tools.rs                 # [MODIFY] 2个 ToolDef 替换（含 -B/-A/-C 属性名）
├── notebook_tools.rs                    # [MODIFY] 1个 ToolDef 替换
├── tool_search.rs                       # [MODIFY] 1个 ToolDef 替换
└── task_tools/
    ├── todo_write.rs                    # [MODIFY] tool_def() 内部改用 tool_def! 宏
    ├── persistent/
    │   ├── create.rs                    # [MODIFY] tool_def() 内部改用 tool_def! 宏
    │   ├── update.rs                    # [MODIFY] tool_def() 内部改用 tool_def! 宏
    │   ├── delete.rs                    # [MODIFY] tool_def() 内部改用 tool_def! 宏
    │   ├── list.rs                      # [MODIFY] tool_def() 内部改用 tool_def! 宏
    │   ├── get.rs                       # [MODIFY] tool_def() 内部改用 tool_def! 宏
    │   └── summary.rs                   # [MODIFY] tool_def() 内部改用 tool_def! 宏
    ├── registry.rs                      # [MODIFY] 无需修改（只是调用 register）
    └── mod.rs                           # 无需修改
```
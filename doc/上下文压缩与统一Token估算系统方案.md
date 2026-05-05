# 上下文压缩与统一 Token 估算系统方案

## 1. 背景与目标

当前系统中存在两类相近但职责不同的数据：

```text
session_memory
  用于构建发送给 LLM 的会话上下文。

session_messages
  用于用户界面回显、会话历史查看、撤回与回滚定位。
```

当前上下文压缩主要发生在 `session_memory.messages` 上：当消息 token 估算超过阈值时，系统会调用 LLM 生成摘要，并用摘要消息替换原始消息数组。保存会话时，`session_messages` 又会按 `session_memory.messages` 重新展开生成 `seq`，这会导致 UI 历史也被压缩后的摘要污染。

用户期望的新目标是：

```text
1. LLM 上下文可以压缩。
2. 用户界面历史保持完整。
3. 撤回 / 回滚仍然准确。
4. 统一监控窗口 token 估算与压缩触发依据。
5. 后续可扩展为对工具、系统提示词、动态上下文、图片等 section 进行压缩。
```

因此，本方案参考 Claude Code 的上下文压缩思路：

```text
原始 transcript / UI 历史长期保留
        ≠
每次发送给 LLM 的上下文
```

压缩应该作用于“LLM 上下文视图”，而不是破坏原始消息日志。

## 2. 当前实现现状

### 2.1 当前 token 估算分成两套

当前压缩触发使用 `estimate_tokens(messages)`，位置：

```text
src-tauri/src/core/session/memory.rs
```

它按字符数粗略估算：

```text
token ≈ chars / 4
```

当前监控窗口使用上下文快照估算，位置：

```text
src-tauri/src/core/agent/pipeline.rs
update_context_snapshot()
```

监控窗口会分 section 估算：

```text
system     System Prompt
dynamic    Dynamic Context
messages   Session Messages
tools      Tools Schema
attachments / images
runtime    Tool Results / Thinking
```

问题是：

```text
压缩触发只看 session.messages
监控窗口看完整请求上下文
```

这会造成 UI 显示上下文已经很高，但后端压缩未触发的情况。

### 2.2 当前 session_messages 与 session_memory 强耦合

当前保存会话时会：

```text
1. 写 session_memory.memory_json
2. DELETE 当前 session 的所有 session_messages
3. 按 session_memory.messages.enumerate() 重建 session_messages.seq
```

因此当前关系是：

```text
session_memory.messages[index]
        ↓
session_messages.seq
```

这意味着 `session_memory.messages` 一旦被压缩重写，`session_messages` 也会被重建为压缩后的短历史。

### 2.3 当前撤回 / 回滚依赖数组下标

当前撤回逻辑按 `session_memory.messages` 的数组下标定位用户消息：

```text
recall_last_message()
  找到最后一条 User 消息 index
  session.messages.truncate(index)

recall_message_from_index(user_message_index)
  session.messages.truncate(user_message_index)
```

当前 checkpoint 关联表也使用：

```text
checkpoint_user_message_links.user_message_index
```

这个 index 本质上仍然是 `session_memory.messages` 的数组下标。

问题是：

```text
压缩前 index 指向原始用户消息
压缩后 session_memory.messages 被摘要重写
原 index 不再稳定
```

所以如果未来让 `session_messages` 保持完整，而 `session_memory` 独立压缩，现有 index 关联会失效。

## 3. 目标架构

目标架构应拆成四层：

```text
┌────────────────────────────────────┐
│ session_messages                    │
│ 原始消息日志，UI 历史事实源          │
│ append-only / soft-delete / recall  │
└────────────────────────────────────┘
                 ↓ 稳定引用
┌────────────────────────────────────┐
│ message_id / turn_id / checkpoint_id │
│ 撤回、回滚、引用、摘要覆盖范围        │
└────────────────────────────────────┘
                 ↓ 构建
┌────────────────────────────────────┐
│ session_memory / context_segments   │
│ LLM 上下文视图，可压缩、裁剪、重建    │
└────────────────────────────────────┘
                 ↓ 估算
┌────────────────────────────────────┐
│ ContextBudgetSnapshot               │
│ 统一 token 估算、监控展示、压缩触发   │
└────────────────────────────────────┘
```

核心原则：

```text
session_messages 是原始历史事实源。
session_memory 是 LLM 上下文缓存 / 视图。
撤回与回滚使用稳定 message_id，不使用数组下标。
监控窗口的总上下文估算同时作为压缩触发依据。
```

## 4. 数据模型设计

### 4.1 session_messages 增加稳定消息标识

建议将 `session_messages` 从“由 memory 展开的索引表”升级为“原始消息日志表”。

推荐字段：

```sql
session_messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  message_id TEXT NOT NULL UNIQUE,
  session_id TEXT NOT NULL,
  turn_id TEXT,
  seq INTEGER NOT NULL,
  role TEXT NOT NULL,
  content_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER,
  recalled_at INTEGER,
  hidden_at INTEGER,
  source TEXT NOT NULL DEFAULT 'user_visible'
)
```

字段说明：

```text
message_id
  稳定消息 ID。撤回、回滚、摘要范围引用都使用它。

turn_id
  一轮用户请求对应的 turn。用户消息、assistant 消息、tool use、tool result 可归属到同一 turn。

seq
  仅作为 UI 排序字段，不作为长期业务主键。

recalled_at
  用户撤回后做软删除或标记，不直接物理删除，便于审计和回滚。

hidden_at
  UI 可隐藏但保留记录，例如内部摘要消息。

source
  标记消息来源，例如 user_visible、assistant_visible、tool_result、compact_summary。
```

### 4.2 session_memory 不再作为 UI 历史事实源

`session_memory` 继续保存 LLM 需要的上下文，但不再负责驱动 UI 完整历史。

推荐结构从：

```rust
pub struct SessionMemory {
    pub messages: Vec<Message>,
    pub context: Vec<String>,
    ...
}
```

逐步演进为：

```rust
pub struct SessionMemory {
    pub context_segments: Vec<ContextSegment>,
    pub activated_tools: Vec<String>,
    pub agent_steps: Vec<AgentStep>,
    pub plan_documents: Vec<PlanDocument>,
}
```

兼容阶段可以先保留 `messages`，但语义改成：

```text
session_memory.messages = 当前 LLM 上下文消息视图
session_messages        = UI 原始历史事实源
```

### 4.3 ContextSegment

引入上下文片段模型，支持多 section 压缩。

```rust
pub struct ContextSegment {
    pub segment_id: String,
    pub session_id: String,
    pub kind: ContextSegmentKind,
    pub source_message_ids: Vec<String>,
    pub source_range: Option<MessageRange>,
    pub content: ContextSegmentContent,
    pub estimated_tokens: usize,
    pub token_count_method: String,
    pub compacted_from: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

`kind` 可选值：

```text
system_prompt
rules_memory
dynamic_context
conversation_recent
conversation_summary
tool_schema
tool_result_summary
image_summary
runtime_thinking
plan_state
```

这样未来可以做到：

```text
消息过长        → 压缩 conversation_recent / conversation_summary
工具结果过长    → 压缩 tool_result_summary
工具 schema 过长 → 精简 tools schema 或按 intent 选择工具
系统提示词过长  → 拆成稳定 system prompt + 可裁剪 rules/memory
图片过长        → 远期图片替换为 image_summary
```

### 4.4 压缩摘要覆盖范围

新增摘要覆盖范围记录，保证压缩后仍能追溯原始消息。

```sql
context_compactions (
  compaction_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  before_snapshot_id TEXT,
  after_segment_id TEXT NOT NULL,
  source_start_message_id TEXT,
  source_end_message_id TEXT,
  source_message_ids_json TEXT NOT NULL,
  transcript_id TEXT,
  summary TEXT NOT NULL,
  estimated_tokens_before INTEGER NOT NULL,
  estimated_tokens_after INTEGER NOT NULL,
  trigger_reason TEXT NOT NULL,
  created_at INTEGER NOT NULL
)
```

作用：

```text
1. 知道某个摘要覆盖了哪些原始消息。
2. 用户问“之前第几轮”时，可以回查原始 message。
3. 撤回/回滚时可以判断是否需要重新构建 LLM 上下文。
4. 调试压缩质量时可以对比压缩前后 token。
```

## 5. 统一 Token 估算系统

### 5.1 统一入口

新增独立模块：

```text
src-tauri/src/core/context_budget/
  mod.rs
  estimator.rs
  snapshot.rs
  policy.rs
  section.rs
```

核心接口：

```rust
pub fn build_context_budget_snapshot(input: ContextBudgetInput) -> ContextBudgetSnapshot;
```

输入：

```rust
pub struct ContextBudgetInput {
    pub session_id: String,
    pub run_id: Option<String>,
    pub loop_count: usize,
    pub model_id: String,
    pub api_format: ApiFormat,
    pub intent: String,
    pub system_prompt: String,
    pub dynamic_context: String,
    pub llm_messages: Vec<Message>,
    pub tools: Vec<serde_json::Value>,
    pub max_output_tokens: i32,
}
```

输出复用并增强当前 `SessionContextSnapshot`：

```rust
pub struct ContextBudgetSnapshot {
    pub session_id: String,
    pub estimated_tokens: usize,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: i32,
    pub available_input_tokens: Option<i64>,
    pub usage_percent: Option<f32>,
    pub sections: Vec<ContextSectionSnapshot>,
    pub pressure: ContextPressure,
    pub recommended_actions: Vec<CompactionAction>,
}
```

### 5.2 分 section 估算

统一估算应覆盖完整 LLM 请求：

```text
estimated_total_input_tokens =
  system_prompt_tokens
+ dynamic_context_tokens
+ messages_tokens
+ tools_schema_tokens
+ attachments_tokens
+ runtime_tokens
+ provider_adapter_overhead
```

每个 section 都应记录：

```text
key
label
chars
estimated_tokens
token_count_method
item_count
content_preview
truncated
compactable
priority
```

其中 `compactable` 表示该 section 是否可以压缩：

```text
system_prompt       默认不可压缩，只能拆分或裁剪可选 rules
rules_memory        可压缩 / 可按需加载
conversation        可压缩
tool_results        可压缩
tools_schema        可按 intent 裁剪，不建议摘要压缩 schema
attachments/images  可替换为摘要
runtime_thinking    可折叠或丢弃远期内容
```

### 5.3 估算方法

优先级：

```text
1. provider / model tokenizer 可用时，使用真实 tokenizer。
2. 项目已有 count_text(model_id, text) 可作为统一入口。
3. tokenizer 不可用时，回退 chars / 4。
4. 图片、工具 schema、adapter overhead 使用经验值或独立规则。
```

必须保证：

```text
监控窗口展示的 estimated_tokens
        ==
压缩触发使用的 estimated_tokens
```

即：

```text
监控窗口的总上下文估算
          ↓
  也作为压缩触发依据
```

## 6. 压缩触发策略

### 6.1 从单一阈值升级为压力等级

当前：

```text
session_memory.messages tokens > MAX_TOKENS_COMPACT_TRIGGER
```

推荐：

```text
根据完整 ContextBudgetSnapshot 判断压力等级
```

```rust
pub enum ContextPressure {
    Normal,
    Watch,
    CompactRecommended,
    CompactRequired,
    Overflow,
}
```

建议规则：

```text
Normal
  estimated_input_tokens < max_context * 0.60

Watch
  estimated_input_tokens >= max_context * 0.60

CompactRecommended
  estimated_input_tokens >= max_context * 0.75

CompactRequired
  estimated_input_tokens >= max_context * 0.85

Overflow
  estimated_input_tokens + max_output_tokens >= max_context
```

如果模型没有 `max_context_tokens`，则使用配置兜底：

```text
MAX_TOKENS_COMPACT_TRIGGER
```

但长期应尽量按模型能力动态判断。

### 6.2 压缩触发点

推荐触发链路：

```text
prepare_history_snapshot()
  ↓
构建 tools schema
  ↓
build_context_budget_snapshot()
  ↓
保存并通知监控窗口
  ↓
根据 snapshot.pressure 判断是否压缩
  ↓
如果压缩，重建 LLM context
  ↓
再次 build_context_budget_snapshot()
  ↓
build_llm_request()
```

也就是说，压缩判断应该发生在真正构建请求前，并使用与监控窗口完全相同的输入。

当前代码中 `compact_if_needed()` 位于主循环请求前，但它只能看到 `session.messages`。建议改为：

```text
compact_if_needed(history_snapshot, tools, budget_snapshot)
```

或进一步拆成：

```text
ContextBudgetService::estimate()
CompactionPolicy::plan()
CompactionEngine::apply()
```

### 6.3 压缩动作优先级

参考 Claude Code 的行为，推荐从低损耗到高损耗：

```text
1. micro compact
   折叠旧 tool_result、远期 thinking、远期图片 base64。

2. section prune
   对工具 schema 按 intent / activated_tools 裁剪。

3. conversation summarize
   把早期对话摘要为 conversation_summary。

4. tool result summarize
   对大型工具结果生成摘要，保留工具名、输入摘要、关键输出、错误。

5. memory/rules compaction
   对动态规则或项目记忆做按需加载或摘要。

6. hard fail / ask user
   如果仍然超过上下文窗口，提示用户新建会话或手动选择保留内容。
```

重要原则：

```text
优先压缩可恢复、可追溯、低价值密度的内容。
不要优先压缩当前用户请求、最近 assistant 回复、当前任务状态。
```

## 7. 会话压缩流程设计

### 7.1 对话历史压缩

推荐保留最近窗口，压缩较早窗口：

```text
完整 UI 历史：
  M1 M2 M3 M4 M5 M6 M7 M8 M9 M10

LLM 上下文：
  Summary(M1-M6) + M7 M8 M9 M10
```

压缩流程：

```text
1. 从 session_messages 中选择待压缩 message_id 范围。
2. 读取原始 content_json。
3. 保存 transcript 或 compaction source。
4. 调用 summary agent 生成结构化摘要。
5. 写入 context_compactions。
6. 更新 session_memory / context_segments。
7. 不修改 session_messages 原始内容。
8. 重新估算 ContextBudgetSnapshot。
```

摘要格式建议：

```text
[Conversation compressed]
覆盖范围：message_id A ~ message_id F
摘要时间：...

## 用户目标
...

## 已完成事项
...

## 当前状态
...

## 关键决策
...

## 重要文件 / 代码片段
...

## 未完成任务
...

## 风险与注意事项
...
```

### 7.2 当前用户消息保护

和当前实现类似，压缩时必须保护最后一条用户消息：

```text
如果最后一条消息是当前用户输入：
  1. 临时从 LLM context 中移除。
  2. 压缩旧上下文。
  3. 插入 assistant padding，避免连续 user 消息。
  4. 再把当前用户输入放回末尾。
```

但这个操作只影响 LLM context，不影响 `session_messages`。

### 7.3 压缩摘要与原始消息的引用

压缩摘要必须记录：

```text
source_message_ids
source_start_message_id
source_end_message_id
transcript_id
```

这样用户后续问：

```text
“刚才前面那个方案是什么？”
“撤回到我问 X 的那一步”
```

系统可以：

```text
1. 在 session_messages 中定位原始消息。
2. 判断该消息是否处于某个 compressed range。
3. 必要时把对应原文或局部摘要重新注入 LLM 上下文。
```

## 8. UI 历史、撤回、回滚设计

### 8.1 UI 历史始终读取 session_messages

前端聊天区应以 `session_messages` 为事实源：

```text
展示完整原始消息
不展示内部 compact_summary，除非用户打开调试视图
支持按 recalled_at / hidden_at 过滤
```

`session_memory` 不再驱动 UI 历史展示。

### 8.2 撤回使用 message_id

当前：

```text
recall_message_from_index(user_message_index)
```

推荐：

```text
recall_message_from_id(message_id)
```

撤回流程：

```text
1. 在 session_messages 中找到目标 user message。
2. 标记该 message 及其后续同 session 消息为 recalled_at，或按策略截断可见历史。
3. 清理 / 重建 checkpoint link。
4. 从剩余可见 session_messages 重建 LLM context。
5. 如果重建后 token 仍过长，重新执行压缩。
6. 保存新的 session_memory / context_segments。
```

这样即使之前做过压缩，撤回也不会依赖压缩后的数组下标。

### 8.3 Checkpoint 绑定 message_id

当前：

```text
checkpoint_user_message_links.user_message_index
```

推荐迁移为：

```sql
checkpoint_user_message_links (
  session_id TEXT NOT NULL,
  user_message_id TEXT NOT NULL,
  checkpoint_id TEXT NOT NULL,
  has_file_edits INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(session_id, user_message_id)
)
```

如果需要兼容旧数据，可保留旧字段：

```text
user_message_index deprecated
user_message_id new
```

迁移时用当前 `session_messages.seq` 回填 `user_message_id`。

### 8.4 回滚后重建 LLM 上下文

回滚不应该直接操作压缩后的 `session_memory.messages` 下标。

推荐流程：

```text
rollback_to_checkpoint(checkpoint_id)
  ↓
查 checkpoint 绑定的 user_message_id
  ↓
在 session_messages 中定位该用户消息
  ↓
标记该消息之后的 UI 消息为 recalled / hidden
  ↓
恢复文件 snapshot
  ↓
基于剩余 UI 历史重建 LLM context
  ↓
必要时重新压缩
```

## 9. 多 Section 压缩扩展

本方案的重点不是只解决 conversation 压缩，而是为后续统一扩展做准备。

### 9.1 System Prompt

系统提示词通常不应该直接摘要压缩，因为它包含行为约束。

推荐策略：

```text
1. 固定核心 system prompt，不压缩。
2. 把可选规则、动态说明、项目文档拆成独立 sections。
3. 对可选 sections 做按需加载、优先级裁剪或摘要。
```

### 9.2 Tools Schema

工具 schema 不建议用自然语言摘要替换，否则模型无法正确调用工具。

推荐策略：

```text
1. 按 intent 选择工具。
2. 按 activated_tools 裁剪工具。
3. 对低优先级工具延迟加载。
4. 优化 schema 描述长度。
```

### 9.3 Tool Results

工具结果是最适合压缩的内容之一。

推荐策略：

```text
1. 最近 N 个 tool_result 保留原文。
2. 旧 tool_result 折叠为：工具名 + 输入摘要 + 输出摘要 + 错误状态。
3. 大型文件读取结果可替换为文件路径、行号范围、摘要。
4. 搜索结果可保留命中摘要，不保留全部输出。
```

### 9.4 Dynamic Context

动态上下文可能包含当前目录、环境、任务状态、文件索引等。

推荐策略：

```text
1. 当前用户请求强相关内容保留。
2. 可重新计算的环境信息不进入长期摘要。
3. 大型动态上下文按 section 独立估算和裁剪。
```

### 9.5 Attachments / Images

图片和附件成本高。

推荐策略：

```text
1. 最近图片保留 base64 或 provider 需要的原始输入。
2. 远期图片替换为 image_summary。
3. 摘要保留用户意图、图片关键信息、相关结论。
```

## 10. 参考 Claude Code 的行为映射

Claude Code 公开机制可抽象为：

```text
1. 本地 transcript 保留原始会话。
2. LLM context 接近上限时自动 compact。
3. compact 前先清理旧工具输出。
4. 必要时把对话历史替换为结构化摘要。
5. system prompt、memory、skills 等启动上下文可重新注入。
6. /context 用于展示上下文占用。
7. /compact 支持手动压缩。
```

映射到 JarvisAgent：

```text
Claude Code transcript
  → session_messages + session_transcripts

Claude Code compacted conversation
  → context_segments.conversation_summary

Claude Code /context
  → ContextInspector + ContextBudgetSnapshot

Claude Code auto-compact trigger
  → 基于 ContextBudgetSnapshot.pressure

Claude Code cleanup old tool outputs
  → micro_compact / tool_result_summary

Claude Code memory reinjection
  → rules_memory / dynamic_context sections
```

## 11. 分阶段实施计划

### Phase 1：统一 token 估算入口

目标：先解决“监控窗口估算”和“压缩触发估算”不一致的问题。

改动：

```text
1. 提取 update_context_snapshot() 内部 section 估算逻辑。
2. 新增 ContextBudgetService。
3. compact_if_needed() 改为使用完整 ContextBudgetSnapshot.estimated_tokens。
4. ContextInspector 继续展示同一份 snapshot。
5. 保留当前 session_memory 压缩方式，降低改动风险。
```

验收：

```text
监控窗口显示的总 estimated_tokens 与压缩触发日志使用同一个值。
工具 schema / system prompt / dynamic context 增大时，也能推动压缩判断。
```

### Phase 2：session_messages 与 session_memory 解耦

目标：压缩不再污染 UI 历史。

改动：

```text
1. session_messages 增加 message_id / turn_id / recalled_at / hidden_at。
2. 保存 session_memory 时不再 DELETE + 重建 session_messages。
3. 新增 append_session_message / mark_messages_recalled。
4. UI 历史读取改为以 session_messages 为事实源。
5. session_memory.messages 只表示 LLM context view。
```

验收：

```text
触发 auto_compact 后，刷新会话 UI 仍能看到完整原始历史。
LLM 请求中使用的是压缩后的上下文。
```

### Phase 3：撤回 / 回滚迁移到 message_id

目标：压缩后撤回和回滚仍准确。

改动：

```text
1. recall_message_from_index 改为 recall_message_from_id。
2. checkpoint_user_message_links 增加 user_message_id。
3. snapshot / checkpoint 关联改为绑定 user_message_id。
4. 回滚后基于 session_messages 重建 LLM context。
5. 兼容旧 index 字段，迁移完成后废弃。
```

验收：

```text
压缩前创建的 checkpoint，在压缩后仍能准确回滚。
用户撤回某条历史消息，不会因为 session_memory 被摘要重写而错位。
```

### Phase 4：引入 context_segments 与 compaction 记录

目标：支持可追溯的摘要覆盖范围。

改动：

```text
1. 新增 context_segments / context_compactions 表或内嵌 JSON 结构。
2. conversation_summary 记录 source_message_ids。
3. auto_compact 不再简单 clear messages，而是生成 summary segment。
4. LLM context builder 从 segments + 最近消息构建 messages。
```

验收：

```text
能查看每次压缩覆盖了哪些原始消息。
用户引用被压缩范围内的信息时，可以回查原始消息或摘要。
```

### Phase 5：多 section 压缩策略

目标：从“只压缩对话历史”升级为“完整上下文预算管理”。

改动：

```text
1. tools schema 按 intent / activated_tools 裁剪。
2. tool_result_summary 独立压缩。
3. image_summary 替换远期图片。
4. rules_memory 按需加载或摘要。
5. ContextInspector 展示每个 section 的 compactable / priority / action。
```

验收：

```text
当 tools / system-adjacent rules / images 占用过大时，系统能选择对应 section 做压缩或裁剪。
```

## 12. 关键风险与规避

### 12.1 不要继续依赖数组下标作为业务主键

风险：

```text
压缩、撤回、回滚、重新构建上下文都会改变数组顺序。
```

规避：

```text
使用 message_id / turn_id / checkpoint_id。
seq 只用于排序。
```

### 12.2 不要压缩掉当前用户请求

风险：

```text
LLM 不知道当前要做什么。
```

规避：

```text
最近用户消息、最近 assistant 回复、当前 tool call 链必须高优先级保留。
```

### 12.3 不要用摘要替代工具 schema

风险：

```text
模型无法按 schema 调用工具。
```

规避：

```text
工具 schema 只能裁剪、按需加载、优化描述，不能自然语言摘要替代。
```

### 12.4 压缩后必须二次估算

风险：

```text
压缩失败或摘要过长，仍然超过上下文窗口。
```

规避：

```text
每次 compaction 后重新生成 ContextBudgetSnapshot。
如果仍 Overflow，继续执行下一优先级动作或提示用户。
```

### 12.5 摘要质量影响后续任务连续性

风险：

```text
摘要漏掉关键约束，后续 Agent 行为跑偏。
```

规避：

```text
摘要 prompt 固定结构，必须包含目标、当前状态、关键决策、文件路径、未完成任务、风险。
保留 transcript_id 以便回查。
```

## 13. 推荐最终数据流

### 13.1 新消息进入

```text
用户发送消息
  ↓
写入 session_messages(message_id, turn_id, seq, role=user)
  ↓
基于可见 session_messages 构建 / 更新 session_memory context view
  ↓
构建 ContextBudgetSnapshot
  ↓
必要时压缩 context view
  ↓
发送给 LLM
```

### 13.2 LLM 回复进入

```text
LLM streaming response
  ↓
工具调用 / 工具结果 / assistant 内容
  ↓
写入 session_messages
  ↓
更新 session_memory context view
  ↓
保存 context snapshot
```

### 13.3 自动压缩

```text
构建完整上下文预算
  ↓
pressure >= CompactRequired
  ↓
选择可压缩 section
  ↓
生成摘要 / 裁剪 / 折叠
  ↓
写 context_compactions
  ↓
更新 session_memory / context_segments
  ↓
不修改 session_messages 原始历史
  ↓
重新估算并发送请求
```

### 13.4 撤回

```text
用户撤回 message_id
  ↓
定位 session_messages 原始消息
  ↓
标记目标消息及后续消息 recalled
  ↓
基于剩余可见历史重建 LLM context view
  ↓
必要时重新压缩
  ↓
更新 UI 与上下文快照
```

### 13.5 回滚

```text
用户选择 checkpoint
  ↓
checkpoint → user_message_id
  ↓
恢复文件 snapshot
  ↓
标记该用户消息之后的 session_messages recalled
  ↓
重建 LLM context view
  ↓
必要时重新压缩
```

## 14. 最小可行落地建议

如果要控制改造风险，建议最先做这三件事：

```text
1. 把 update_context_snapshot 的估算逻辑提成统一 ContextBudgetService。
2. compact_if_needed 改成使用 ContextBudgetSnapshot.estimated_tokens。
3. 给 session_messages 引入 message_id，并规划 recall/checkpoint 从 index 迁移到 message_id。
```

这三步完成后，即使暂时还没完全引入 context_segments，也能先解决最关键的两个问题：

```text
监控窗口估算与压缩触发一致。
后续解耦 UI 历史和 LLM memory 时有稳定主键可用。
```

## 15. 总结

推荐方案不是简单地“只压缩 session_memory”，而是建立一套统一的上下文预算系统：

```text
session_messages 负责完整历史
session_memory / context_segments 负责 LLM 上下文视图
ContextBudgetSnapshot 负责统一估算、展示和触发
CompactionEngine 负责按 section 压缩
message_id 负责撤回、回滚和摘要追溯
```

这样可以同时满足：

```text
LLM 上下文可压缩
UI 历史不丢失
撤回 / 回滚不受压缩影响
监控窗口与压缩触发一致
未来可扩展到工具、系统提示词、图片、动态上下文等多 section 压缩
```

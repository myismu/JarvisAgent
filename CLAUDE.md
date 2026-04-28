# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

JarvisAgent is a Tauri 2.0 + Vue 3 desktop AI coding assistant. Frontend is TypeScript/Pinia, backend is Rust/Tokio. Supports 20+ LLM models across DeepSeek, Claude, GPT, Gemini, Qwen, and others.

## Commands

```bash
pnpm tauri dev         # development with hot reload
pnpm build             # frontend type-check only (vue-tsc --noEmit && vite build)
pnpm tauri build        # production build
cargo test              # Rust tests (run from src-tauri/)
pnpm install            # frontend dependencies
```

## Architecture

### Frontend ↔ Backend communication

Frontend calls Rust commands via Tauri `invoke`. Backend pushes events to frontend via Tauri `emit`. All commands are registered in `src-tauri/src/lib.rs`'s `invoke_handler`. The public API surface is re-exported through `src-tauri/src/core/mod.rs`.

### Agent pipeline

Entry point: `src-tauri/src/core/agent/pipeline.rs` (function `run_pipeline`).

```
Intent classification → tool set loading → context assembly → agent loop → stream output
```

Pipeline stages:
- **Intent**: `src-tauri/src/core/intent.rs` — classifies user message into `GENERAL_CHAT | PROJECT_ACTION | MEMORY_QUERY | DANGEROUS_ACTION` using the utility model
- **Tool registration/routing**: `src-tauri/src/core/tools/mod.rs` — loads tools on demand based on intent, routes tool calls
- **Streaming**: `src-tauri/src/core/agent/stream.rs` — parses SSE response, handles thinking blocks, text blocks, tool calls
- **Context**: `src-tauri/src/core/agent/context.rs` — injects dynamic context (memory, skills, directory structure)
- **Tool execution**: `src-tauri/src/core/agent/tools_runner.rs` — executes tool calls returned by the model

### LLM provider abstraction

`LlmProvider` trait (`src-tauri/src/core/traits.rs`) abstracts Anthropic vs OpenAI API format differences. Concrete implementations in `src-tauri/src/core/providers/` (`anthropic.rs`, `openai.rs`). `ApiFormat` enum (`src-tauri/src/core/api_format.rs`) replaces old string comparison, provides auth header, version header, etc.

### Error types

`src-tauri/src/core/error.rs` — top-level `AgentError` (used as Tauri command return type) wraps `ApiError` (HTTP, retries), `ToolError` (execution, permissions), `MemoryError` (compaction, file I/O). All use `thiserror::Error` + `Serialize`.

### State management

- **Rust**: `SessionManager` (`src-tauri/src/core/state.rs`) — global `HashMap<String, Arc<SessionContext>>` holding cancellation tokens, workspace, pending permissions per session
- **Frontend Pinia stores** (`src/stores/`): `session.ts` (session lifecycle), `chat.ts` (message rendering), `agent.ts` (agent runs, sub-agents, todos), `permission.ts` (permission modals)
- **Event bridge**: `src/composables/useAgentEvents.ts` — listens to all backend Tauri events, dispatches to stores

### Snapshot & checkpoint system

- **Snapshot engine** (`src-tauri/src/core/snapshot_engine/`): file-level snapshots with tree versioning, atomic rollback, journal, GC, multi-agent sandbox and merge
- **Checkpoints** (`src-tauri/src/core/checkpoint.rs`): tree-based checkpoints recording `FileOperation` entries for rollback, with branch management
- **Snapshot manager** (`src-tauri/src/core/snapshot_manager/`): `SessionManagerRegistry` for session-scoped snapshot state, persisted under `.sessions/<id>/snapshots/`

### Model registry

`src-tauri/model_registry.json` defines model capabilities (thinking parameter names, vision support, context length, max tokens per turn). Queried via `src-tauri/src/core/registry.rs`.

### Data directories

All data stored under the app runtime directory: `config.json`, `.sessions/`, `.tasks/`, `.checkpoints/`, `.snapshots/`, `skills/`, `global_memory.md`.

## Conventions

- Do not delete existing Chinese comments in the Rust/Vue source code (project-wide rule)
- Use `thiserror` for new error types; avoid bare string errors
- New API format logic should extend `LlmProvider` trait, not add format checks in business code
- New frontend event types must be defined in `src/types/index.ts` and handled in `useAgentEvents.ts`

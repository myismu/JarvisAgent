//! orchestration 模块 - Agent 编排与调度系统
//!
//! 负责多任务并行调度、子Agent管理、任务生命周期追踪。
//! 核心组件：
//! - `TaskScheduler`: 基于依赖图的并行任务调度器
//! - `TaskManager`: 任务 CRUD 与依赖管理
//! - `SubAgentMonitor`: 子Agent运行状态监控
//! - `AgentRun`: 主Agent执行记录与检查点

pub mod agent_run_repository;
pub mod agent_runs;
pub mod multi_agent;
pub mod scheduler;
pub mod subagents;
pub mod tasks;

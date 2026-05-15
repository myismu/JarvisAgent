//! Lightweight subagent registry.
//!
//! This module keeps the agent-type contract separate from the tool registry:
//! agent definitions decide which tools a subagent may see, while ToolRegistry
//! remains the source of truth for tool schemas and read-only metadata.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::registry::ToolRegistry;

pub const DEFAULT_AGENT_ROLE: &str = "general";
pub const IMPLEMENTATION_AGENT_ROLE: &str = "implementation";

const GENERAL_TOOLS: &[&str] = &[
    "GetSystemInfo",
    "LoadSkill",
    "ListDirectory",
    "FindFiles",
    "SearchText",
    "SearchRepo",
    "ReadFile",
    "ReadFileSkeleton",
    "FindSymbol",
    "ReadSymbol",
    "FindReferences",
    "CodeSearch",
    "WriteFile",
    "EditFile",
    "EditNotebook",
    "RunCommand",
    "RunGitCommand",
    "StartBackgroundCommand",
    "CheckBackgroundCommand",
];

const READ_ONLY_RESEARCH_TOOLS: &[&str] = &[
    "GetSystemInfo",
    "LoadSkill",
    "ListDirectory",
    "FindFiles",
    "SearchText",
    "SearchRepo",
    "ReadFile",
    "ReadFileSkeleton",
    "FindSymbol",
    "ReadSymbol",
    "FindReferences",
    "CodeSearch",
    "RunGitCommand",
    "CheckBackgroundCommand",
];

const VERIFICATION_TOOLS: &[&str] = &[
    "GetSystemInfo",
    "LoadSkill",
    "ListDirectory",
    "FindFiles",
    "SearchText",
    "SearchRepo",
    "ReadFile",
    "ReadFileSkeleton",
    "FindSymbol",
    "ReadSymbol",
    "FindReferences",
    "CodeSearch",
    "RunCommand",
    "RunGitCommand",
    "CheckBackgroundCommand",
];

#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub agent_role: &'static str,
    pub when_to_use: &'static str,
    pub system_prompt: &'static str,
    pub tools: &'static [&'static str],
    pub disallowed_tools: &'static [&'static str],
    pub model: Option<&'static str>,
    pub read_only_default: bool,
    pub max_turns: Option<usize>,
}

pub struct AgentRegistry {
    agents: HashMap<&'static str, AgentDefinition>,
    insertion_order: Vec<&'static str>,
}

static AGENT_REGISTRY: OnceLock<AgentRegistry> = OnceLock::new();

impl AgentRegistry {
    pub fn global() -> &'static AgentRegistry {
        AGENT_REGISTRY.get_or_init(|| {
            let mut registry = AgentRegistry {
                agents: HashMap::new(),
                insertion_order: Vec::new(),
            };

            registry.register(AgentDefinition {
                agent_role: DEFAULT_AGENT_ROLE,
                when_to_use: "General delegated work. Defaults to read-only unless the caller explicitly allows writes.",
                system_prompt: "You are a general-purpose subagent. Complete the delegated task directly and report only the useful result.",
                tools: GENERAL_TOOLS,
                disallowed_tools: &[],
                model: None,
                read_only_default: true,
                max_turns: Some(30),
            });

            registry.register(AgentDefinition {
                agent_role: "explore",
                when_to_use: "Read-only codebase exploration, file discovery, and focused research.",
                system_prompt: "You are an exploration subagent. Inspect the codebase, gather evidence, and return concise findings with file paths. Do not modify files.",
                tools: READ_ONLY_RESEARCH_TOOLS,
                disallowed_tools: &[],
                model: None,
                read_only_default: true,
                max_turns: Some(20),
            });

            registry.register(AgentDefinition {
                agent_role: "plan",
                when_to_use: "Read-only planning before implementation. Produce an actionable plan, not code changes.",
                system_prompt: "You are a planning subagent. Analyze the requested change and return a concrete implementation plan. Do not modify files.",
                tools: READ_ONLY_RESEARCH_TOOLS,
                disallowed_tools: &[],
                model: None,
                read_only_default: true,
                max_turns: Some(15),
            });

            registry.register(AgentDefinition {
                agent_role: "review",
                when_to_use: "Independent read-only code review focused on bugs, risks, regressions, and missing tests.",
                system_prompt: "You are a code review subagent. Prioritize concrete defects with file references. Do not modify files.",
                tools: READ_ONLY_RESEARCH_TOOLS,
                disallowed_tools: &[],
                model: None,
                read_only_default: true,
                max_turns: Some(15),
            });

            registry.register(AgentDefinition {
                agent_role: "verification",
                when_to_use: "Verify behavior after changes by inspecting code and running targeted checks or tests.",
                system_prompt: "You are a verification subagent. Run targeted checks when useful, inspect failures, and report pass/fail evidence. Do not edit files.",
                tools: VERIFICATION_TOOLS,
                disallowed_tools: &["WriteFile", "EditFile", "EditNotebook", "StartBackgroundCommand"],
                model: None,
                read_only_default: false,
                max_turns: Some(20),
            });

            registry.register(AgentDefinition {
                agent_role: IMPLEMENTATION_AGENT_ROLE,
                when_to_use: "Concrete implementation work that may edit files or run commands.",
                system_prompt: "You are an implementation subagent. Make the requested changes, keep scope tight, and verify the result when practical.",
                tools: GENERAL_TOOLS,
                disallowed_tools: &[],
                model: None,
                read_only_default: false,
                max_turns: Some(50),
            });

            registry
        })
    }

    fn register(&mut self, agent: AgentDefinition) {
        if !self.agents.contains_key(agent.agent_role) {
            self.insertion_order.push(agent.agent_role);
        }
        self.agents.insert(agent.agent_role, agent);
    }

    pub fn get(&self, agent_role: &str) -> Option<&AgentDefinition> {
        self.agents.get(agent_role)
    }

    pub fn default_agent(&self) -> &AgentDefinition {
        self.get(DEFAULT_AGENT_ROLE)
            .expect("default subagent definition must exist")
    }

    pub fn available_types(&self) -> Vec<&'static str> {
        self.insertion_order.clone()
    }

    pub fn prompt_listing(&self) -> String {
        self.insertion_order
            .iter()
            .filter_map(|agent_role| self.agents.get(agent_role))
            .map(|agent| format!("- {}: {}", agent.agent_role, agent.when_to_use))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn resolve_tools(
        &self,
        agent: &AgentDefinition,
        read_only: bool,
    ) -> Vec<serde_json::Value> {
        let tool_registry = ToolRegistry::global();
        let deny: HashSet<&str> = agent.disallowed_tools.iter().copied().collect();
        let mut seen = HashSet::new();
        let mut schemas = Vec::new();

        for tool_name in agent.tools {
            let tool_name = *tool_name;
            if !seen.insert(tool_name) || deny.contains(tool_name) {
                continue;
            }
            let Some(tool) = tool_registry.get(tool_name) else {
                continue;
            };
            if !tool.is_enabled {
                continue;
            }
            if read_only && !tool.is_read_only {
                continue;
            }
            schemas.push(tool.schema.clone());
        }

        schemas
    }
}

pub fn normalize_agent_role(value: Option<&str>) -> &str {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_AGENT_ROLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_exists() {
        let registry = AgentRegistry::global();
        assert_eq!(registry.default_agent().agent_role, DEFAULT_AGENT_ROLE);
        assert!(registry.available_types().contains(&"implementation"));
    }

    #[test]
    fn explore_agent_resolves_read_only_tools() {
        let registry = AgentRegistry::global();
        let agent = registry.get("explore").unwrap();
        let tools = registry.resolve_tools(agent, agent.read_only_default);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        assert!(names.contains(&"ReadFile"));
        assert!(names.contains(&"SearchText"));
        assert!(!names.contains(&"EditFile"));
        assert!(!names.contains(&"RunCommand"));
    }

    #[test]
    fn implementation_agent_can_include_mutating_tools() {
        let registry = AgentRegistry::global();
        let agent = registry.get("implementation").unwrap();
        let tools = registry.resolve_tools(agent, false);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        assert!(names.contains(&"EditFile"));
        assert!(names.contains(&"EditNotebook"));
        assert!(names.contains(&"RunCommand"));
    }

    #[test]
    fn read_only_filter_uses_tool_metadata() {
        let registry = AgentRegistry::global();
        let agent = registry.get("implementation").unwrap();
        let tools = registry.resolve_tools(agent, true);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        assert!(names.contains(&"ReadFile"));
        assert!(names.contains(&"SearchText"));
        assert!(!names.contains(&"EditFile"));
        assert!(!names.contains(&"EditNotebook"));
        assert!(!names.contains(&"StartBackgroundCommand"));
    }

    #[test]
    fn task_schema_exposes_typed_agent_fields() {
        let tool_registry = ToolRegistry::global();
        let task = tool_registry.get("RunSubagent").unwrap();
        let properties = &task.schema["input_schema"]["properties"];

        assert!(properties["description"].is_object());
        assert!(properties["subagent_role"].is_object());
        assert!(properties["model"].is_object());
        assert!(properties["read_only"].is_object());
    }
}

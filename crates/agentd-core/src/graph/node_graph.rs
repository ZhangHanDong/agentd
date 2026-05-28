//! Typed, validated workflow graph built from a parsed `dot::ast::Graph`.
//! See design §2.7 and `specs/core/p2-node-graph-validate.spec.md`.

use std::collections::BTreeMap;

use crate::CoreError;
use crate::dot::ast::Graph as DotGraph;
use crate::graph::validate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Start,
    Terminal,
    Regular,
}

impl NodeShape {
    fn from_attr(shape: Option<&str>) -> Self {
        match shape {
            Some("Mdiamond") => Self::Start,
            Some("Msquare") => Self::Terminal,
            _ => Self::Regular,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    Codergen,
    Conditional,
    Tool,
    WaitHuman,
    ParallelFanOut,
    ParallelFanIn,
}

impl HandlerKind {
    /// Map a `handler=` attribute value to a known P0 handler.
    /// `stack.manager_loop` is intentionally absent (P1+).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "codergen" => Some(Self::Codergen),
            "conditional" => Some(Self::Conditional),
            "tool" => Some(Self::Tool),
            "wait.human" => Some(Self::WaitHuman),
            "parallel.fan_out" => Some(Self::ParallelFanOut),
            "parallel.fan_in" => Some(Self::ParallelFanIn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeDef {
    pub id: String,
    pub shape: NodeShape,
    pub handler: Option<HandlerKind>,
    pub goal_gate: bool,
    /// All raw attributes, preserved for handlers in later phases.
    pub attrs: BTreeMap<String, String>,
}

impl NodeDef {
    #[must_use]
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct EdgeDef {
    pub from: String,
    pub to: String,
    pub attrs: BTreeMap<String, String>,
}

impl EdgeDef {
    #[must_use]
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }

    /// `retry_target=true` edge attribute (D8b).
    #[must_use]
    pub fn is_retry_target(&self) -> bool {
        self.attr("retry_target") == Some("true")
    }
}

#[derive(Debug, Clone)]
pub struct NodeGraph {
    pub name: String,
    pub nodes: Vec<NodeDef>,
    pub edges: Vec<EdgeDef>,
}

impl NodeGraph {
    /// Build and validate a `NodeGraph` from a parsed DOT graph.
    ///
    /// # Errors
    /// Returns [`CoreError::GraphValidate`] listing EVERY validation violation
    /// (not just the first) when the graph is structurally or semantically
    /// invalid per design §2.7.
    pub fn from_ast(ast: &DotGraph) -> Result<Self, CoreError> {
        let mut violations: Vec<String> = Vec::new();

        let nodes: Vec<NodeDef> = ast
            .nodes
            .iter()
            .map(|n| {
                let shape = NodeShape::from_attr(n.attrs.get("shape").map(String::as_str));
                let handler = n.attrs.get("handler").and_then(|h| {
                    let parsed = HandlerKind::parse(h);
                    if parsed.is_none() {
                        violations.push(format!("node '{}': unknown handler '{h}'", n.id));
                    }
                    parsed
                });
                let goal_gate = n.attrs.get("goal_gate").map(String::as_str) == Some("true");
                NodeDef {
                    id: n.id.clone(),
                    shape,
                    handler,
                    goal_gate,
                    attrs: n.attrs.clone(),
                }
            })
            .collect();

        let edges: Vec<EdgeDef> = ast
            .edges
            .iter()
            .map(|e| EdgeDef {
                from: e.from.clone(),
                to: e.to.clone(),
                attrs: e.attrs.clone(),
            })
            .collect();

        let graph = Self {
            name: ast.name.clone(),
            nodes,
            edges,
        };

        validate::run(&graph, &mut violations);

        if violations.is_empty() {
            Ok(graph)
        } else {
            Err(CoreError::GraphValidate(violations.join("; ")))
        }
    }

    #[must_use]
    pub fn node(&self, id: &str) -> Option<&NodeDef> {
        self.nodes.iter().find(|n| n.id == id)
    }

    #[must_use]
    pub fn starts(&self) -> Vec<&NodeDef> {
        self.nodes
            .iter()
            .filter(|n| n.shape == NodeShape::Start)
            .collect()
    }

    #[must_use]
    pub fn terminals(&self) -> Vec<&NodeDef> {
        self.nodes
            .iter()
            .filter(|n| n.shape == NodeShape::Terminal)
            .collect()
    }
}

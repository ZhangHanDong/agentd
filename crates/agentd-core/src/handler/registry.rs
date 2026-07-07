//! Resolves a [`HandlerKind`] to the `Arc<dyn Handler>` that implements it.

use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::HandlerKind;
use crate::handler::{
    CodergenHandler, ConditionalHandler, FanInHandler, FanOutHandler, Handler, ToolHandler,
    WaitHumanHandler,
};

/// Maps each node kind to its handler implementation.
#[derive(Clone, Default)]
pub struct HandlerRegistry {
    handlers: HashMap<HandlerKind, Arc<dyn Handler>>,
}

impl std::fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerRegistry")
            .field("kinds", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl HandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with all six P0.1 handlers registered: the synchronous
    /// `conditional`/`tool` and the park-style `wait.human`/`fan_out`/`fan_in`/
    /// `codergen`.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        reg.register(HandlerKind::Conditional, Arc::new(ConditionalHandler));
        reg.register(HandlerKind::Tool, Arc::new(ToolHandler));
        reg.register(HandlerKind::WaitHuman, Arc::new(WaitHumanHandler));
        reg.register(HandlerKind::ParallelFanOut, Arc::new(FanOutHandler));
        reg.register(HandlerKind::ParallelFanIn, Arc::new(FanInHandler));
        reg.register(HandlerKind::Codergen, Arc::new(CodergenHandler));
        reg
    }

    pub fn register(&mut self, kind: HandlerKind, handler: Arc<dyn Handler>) {
        self.handlers.insert(kind, handler);
    }

    #[must_use]
    pub fn get(&self, kind: HandlerKind) -> Option<Arc<dyn Handler>> {
        self.handlers.get(&kind).cloned()
    }
}

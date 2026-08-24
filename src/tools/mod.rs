mod common;
pub mod params;
pub mod readonly;
mod session_tools;
pub mod shared;

#[cfg(test)]
mod tests;

pub use common::ToolError;

use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool_handler,
};

use crate::AppState;

#[derive(Clone)]
pub struct DeskForemanService {
    pub(crate) state: AppState,
    pub(crate) tool_router: ToolRouter<Self>,
}

impl DeskForemanService {
    pub fn new(state: AppState) -> Self {
        Self {
            state: state.clone(),
            tool_router: Self::tool_router_for_state(),
        }
    }

    fn tool_router_for_state() -> ToolRouter<Self> {
        let mut router = ToolRouter::new();
        readonly::register_routes(&mut router);
        session_tools::register_routes(&mut router);
        router
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DeskForemanService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("Desk Foreman", env!("CARGO_PKG_VERSION"))
                    .with_description(
                        "Workspace-scoped coding tools for the authenticated user's isolated Desk Foreman workspace, exposing shell, patch, and file operations with a Codex-compatible surface.",
                    ),
            )
            .with_instructions(
                "Use this server for work inside the authenticated user's isolated Desk Foreman workspace, especially from clients that do not already provide equivalent local shell, patch, or file capabilities. Prefer host-native tools when they offer the same operation directly. Paths are workspace-relative and commands run only inside that user's provisioned workspace root.",
            )
    }
}

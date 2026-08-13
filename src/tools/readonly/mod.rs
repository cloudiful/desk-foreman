pub(crate) mod data;
pub(crate) mod search;
#[cfg(test)]
mod test_data;
pub mod types;

#[cfg(test)]
pub(crate) use test_data::{read_file_result, search_files_result, stat_path_result};

use rmcp::{
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
    },
    model::{CallToolResponse, CallToolResult},
};

use crate::actor::actor_from_mcp_context;
use crate::tools::{
    DeskForemanService,
    common::{mcp_error, parse_and_validate_tool_params, readonly_tool, structured_text_result},
    params::{GlobParams, GrepParams, ReadParams},
    shared,
};

pub(super) fn register_routes(router: &mut ToolRouter<DeskForemanService>) {
    router.add_route(read_route());
    router.add_route(glob_route());
    router.add_route(grep_route());
}

fn read_route() -> ToolRoute<DeskForemanService> {
    ToolRoute::new_dyn(
        readonly_tool::<ReadParams, types::ReadOutput>(
            "read",
            "Read a file or directory from the managed workspace with bounded, paginated output.",
        ),
        |ctx| Box::pin(async move { read_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

fn glob_route() -> ToolRoute<DeskForemanService> {
    ToolRoute::new_dyn(
        readonly_tool::<GlobParams, types::GlobOutput>(
            "glob",
            "Find workspace files by glob pattern.",
        ),
        |ctx| Box::pin(async move { glob_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

fn grep_route() -> ToolRoute<DeskForemanService> {
    ToolRoute::new_dyn(
        readonly_tool::<GrepParams, types::GrepOutput>(
            "grep",
            "Search workspace file contents with ripgrep.",
        ),
        |ctx| Box::pin(async move { grep_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

async fn read_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: ReadParams = parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::read(&context.service.state, &actor, &params).map_err(mcp_error)?;
    structured_text_result(data::read_output_text(&output), &output)
}

async fn glob_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: GlobParams = parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::glob(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    structured_text_result(output.matches.join("\n"), &output)
}

async fn grep_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: GrepParams = parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::grep(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    let text = output
        .matches
        .iter()
        .map(|entry| format!("{}:{}:{}", entry.path, entry.line_number, entry.line))
        .collect::<Vec<_>>()
        .join("\n");
    structured_text_result(text, &output)
}

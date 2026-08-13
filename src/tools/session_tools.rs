use std::borrow::Cow;

use rmcp::{
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::{ToolCallContext, schema_for_output},
    },
    model::{CallToolResponse, CallToolResult, Tool},
};

use crate::{
    actor::actor_from_mcp_context,
    shell::ShellToolOutput,
    tools::{
        DeskForemanService,
        common::{
            mcp_error, parse_and_validate_tool_params, schema_for_input, shell_call_result,
            structured_text_result, write_annotations,
        },
        params::{
            ApplyPatchParams, CancelSessionParams, EditParams, ShellParams, WriteStdinParams,
        },
        shared,
    },
};

pub(super) fn register_routes(router: &mut ToolRouter<DeskForemanService>) {
    router.add_route(exec_command_route());
    router.add_route(write_stdin_route());
    router.add_route(cancel_session_route());
    router.add_route(apply_patch_route());
    router.add_route(edit_route());
}

fn exec_command_route() -> rmcp::handler::server::router::tool::ToolRoute<DeskForemanService> {
    let output_schema = schema_for_output::<ShellToolOutput>();
    ToolRoute::new_dyn(
        Tool::new_with_raw(
            "shell",
            Some(Cow::Borrowed(
                "Execute a shell command inside the managed workspace. Use workdir and timeout instead of changing directories or relying on an unbounded command.",
            )),
            schema_for_input::<ShellParams>(),
        )
        .with_annotations(write_annotations())
        .with_raw_output_schema(output_schema),
        |ctx| Box::pin(async move { exec_command_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

fn write_stdin_route() -> rmcp::handler::server::router::tool::ToolRoute<DeskForemanService> {
    let output_schema = schema_for_output::<ShellToolOutput>();
    ToolRoute::new_dyn(
        Tool::new_with_raw(
            "write_stdin",
            Some(Cow::Borrowed(
                "Write to an existing managed workspace shell session or poll for more output. Pair with `shell` for interactive command streaming.",
            )),
            schema_for_input::<WriteStdinParams>(),
        )
        .with_annotations(write_annotations())
        .with_raw_output_schema(output_schema),
        |ctx| Box::pin(async move { write_stdin_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

fn cancel_session_route() -> rmcp::handler::server::router::tool::ToolRoute<DeskForemanService> {
    let output_schema = schema_for_output::<shared::CancelSessionOutput>();
    ToolRoute::new_dyn(
        Tool::new_with_raw(
            "cancel_session",
            Some(Cow::Borrowed(
                "Terminate an active managed workspace shell session.",
            )),
            schema_for_input::<CancelSessionParams>(),
        )
        .with_annotations(write_annotations())
        .with_raw_output_schema(output_schema),
        |ctx| {
            Box::pin(async move {
                cancel_session_handler(ctx)
                    .await
                    .map(CallToolResponse::from)
            })
        },
    )
}

fn apply_patch_route() -> rmcp::handler::server::router::tool::ToolRoute<DeskForemanService> {
    ToolRoute::new_dyn(
        Tool::new_with_raw(
            "apply_patch",
            Some(Cow::Borrowed(
                "Apply a Codex patch to files in the managed workspace. The patchText value must use the Codex patch DSL.",
            )),
            schema_for_input::<ApplyPatchParams>(),
        )
        .with_annotations(write_annotations()),
        |ctx| Box::pin(async move { apply_patch_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

fn edit_route() -> rmcp::handler::server::router::tool::ToolRoute<DeskForemanService> {
    ToolRoute::new_dyn(
        Tool::new_with_raw(
            "edit",
            Some(Cow::Borrowed(
                "Replace exact text in one workspace file. Re-read the file when the text is missing or ambiguous; use apply_patch for new files or multi-file changes.",
            )),
            schema_for_input::<EditParams>(),
        )
        .with_annotations(write_annotations()),
        |ctx| Box::pin(async move { edit_handler(ctx).await.map(CallToolResponse::from) }),
    )
}

async fn exec_command_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: crate::tools::params::ShellParams =
        parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::shell(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    Ok(shell_call_result(output))
}

async fn write_stdin_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: WriteStdinParams =
        parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::write_stdin(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    Ok(shell_call_result(output))
}

async fn cancel_session_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: CancelSessionParams =
        parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::cancel_session(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    structured_text_result(output.state.clone(), &output)
}

async fn apply_patch_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: ApplyPatchParams =
        parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::apply_patch(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    structured_text_result(output.summary.clone(), &output)
}

async fn edit_handler(
    context: ToolCallContext<'_, DeskForemanService>,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let actor = actor_from_mcp_context(&context.service.state, &context.request_context)?;
    let params: EditParams = parse_and_validate_tool_params(context.arguments.unwrap_or_default())?;
    let output = shared::edit(&context.service.state, &actor, &params)
        .await
        .map_err(mcp_error)?;
    structured_text_result(format!("Edited {}", output.path), &output)
}

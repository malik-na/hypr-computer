//! One agent session's MCP server, built on the official Rust SDK (rmcp).
//! Translates between MCP and the `tools` registry; nothing else.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tools::{CallContext, Content, Registry, ToolOutput, ToolSpec};

pub struct SessionServer {
    ctx: CallContext,
    registry: Arc<Registry>,
}

impl SessionServer {
    pub fn new(ctx: CallContext, registry: Arc<Registry>) -> Self {
        Self { ctx, registry }
    }
}

impl ServerHandler for SessionServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("hypr-computer", env!("CARGO_PKG_VERSION"));
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.registry.list().into_iter().map(to_mcp_tool).collect(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.unwrap_or_default();
        match self.registry.call(&self.ctx, &request.name, args).await {
            Ok(output) => Ok(to_mcp_result(output).into()),
            // MCP: unknown tools and bad arguments are invalid-params errors.
            Err(e) => Err(ErrorData::invalid_params(e.to_string(), None)),
        }
    }
}

fn to_mcp_tool(spec: ToolSpec) -> Tool {
    Tool::new(spec.name, spec.description, spec.input_schema)
}

fn to_mcp_result(output: ToolOutput) -> CallToolResult {
    let content = output
        .content
        .into_iter()
        .map(|c| match c {
            Content::Text(t) => ContentBlock::text(t),
        })
        .collect();
    if output.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

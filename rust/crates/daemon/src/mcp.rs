//! One agent session's MCP server, built on the official Rust SDK (rmcp).
//! Translates between MCP and the `tools` registry; nothing else.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tools::{Content, Registry, ToolOutput, ToolSpec};

use crate::owner::Gate;

pub struct SessionServer {
    session: Arc<Gate>,
    registry: Arc<Registry>,
    /// The owner has been told who the agent is.
    met: AtomicBool,
}

impl SessionServer {
    pub fn new(session: Arc<Gate>, registry: Arc<Registry>) -> Self {
        Self {
            session,
            registry,
            met: AtomicBool::new(false),
        }
    }

    /// Tells the owner who the agent is, on its first request: the name it
    /// gave in `initialize` or, from MCP 2026-07-28 on, in the request's
    /// `_meta`, where it is optional; "" if it gave none.
    fn meet(&self, context: &RequestContext<RoleServer>) {
        if !self.met.swap(true, Ordering::Relaxed) {
            let name = context.client_info().map(|info| info.name);
            self.session.introduce(name.unwrap_or_default());
        }
    }
}

impl ServerHandler for SessionServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("hypr-computer", env!("CARGO_PKG_VERSION"));
        info
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        self.meet(&context);
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.meet(&context);
        Ok(ListToolsResult {
            tools: self.registry.list().into_iter().map(to_mcp_tool).collect(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.meet(&context);
        let args = request.arguments.unwrap_or_default();
        match self
            .registry
            .call(self.session.as_ref(), &request.name, args)
            .await
        {
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
            Content::Image(png) => ContentBlock::image(STANDARD.encode(png.0), "image/png"),
        })
        .collect();
    if output.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

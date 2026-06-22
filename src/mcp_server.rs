use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ssh_config::list_ssh_hosts;
use crate::ssh_pool::ConnectionPool;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Id {
    String(String),
    Number(i64),
    Null,
}

#[derive(Deserialize, Debug)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Id>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

#[derive(Serialize, Debug)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize, Debug)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Abbreviates output by truncating the middle portion if it exceeds max_lines.
fn abbreviate_output(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= max_lines {
        return output.to_string();
    }

    let keep = max_lines / 2;
    if keep == 0 {
        return format!("... [{} lines truncated] ...\n", lines.len());
    }

    let head = &lines[..keep];
    let tail = &lines[lines.len() - keep..];

    let mut result = String::new();
    for line in head {
        result.push_str(line);
        result.push('\n');
    }
    result.push_str(&format!(
        "... [{} lines truncated] ...\n",
        lines.len() - max_lines
    ));
    for line in tail {
        result.push_str(line);
        result.push('\n');
    }
    result
}

pub struct McpServer {
    pool: Arc<ConnectionPool>,
}

impl McpServer {
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            pool: Arc::new(ConnectionPool::new(idle_timeout)),
        }
    }

    pub async fn run(&self) -> Result<()> {
        eprintln!("agentic_ssh MCP server starting up...");
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut stdout = tokio::io::stdout();
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                eprintln!("stdin reached EOF, shutting down MCP server");
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!(
                        "Failed to parse JSON-RPC request: {:?}. Raw line: {}",
                        e, trimmed
                    );
                    // If parsing fails, we cannot send a response if we don't have an ID,
                    // but we can send a parse error response with null ID.
                    let resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: Some(Id::Null),
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                            data: None,
                        }),
                    };
                    self.send_response(&mut stdout, &resp).await?;
                    continue;
                }
            };

            if req.jsonrpc != "2.0" {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32600,
                        message: "Invalid Request: jsonrpc version must be '2.0'".to_string(),
                        data: None,
                    }),
                };
                self.send_response(&mut stdout, &resp).await?;
                continue;
            }

            // Handle the request
            let resp = self.handle_request(req).await;

            // If the request had an ID, send response (JSON-RPC notification has no ID and expects no response)
            if resp.id.is_some() {
                self.send_response(&mut stdout, &resp).await?;
            }
        }

        Ok(())
    }

    async fn send_response(
        &self,
        stdout: &mut tokio::io::Stdout,
        resp: &JsonRpcResponse,
    ) -> Result<()> {
        let serialized = serde_json::to_vec(resp)?;
        stdout.write_all(&serialized).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
        Ok(())
    }

    async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let method = req.method.as_str();
        let id = req.id.clone();

        match method {
            "initialize" => {
                let result = serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "agentic_ssh",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(result),
                    error: None,
                }
            }
            "initialized" => {
                // Initialized is a notification, return dummy response (won't be sent because id is None)
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: None,
                }
            }
            "ping" => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::json!({})),
                error: None,
            },
            "tools/list" => {
                let tools = serde_json::json!({
                    "tools": [
                        {
                            "name": "list_hosts",
                            "description": "List all SSH hosts - USE THIS instead of parsing ~/.ssh/config manually. Respects allow_hosts/ignore_hosts filtering.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "run_command",
                            "description": "Execute a shell command on a single host via 'host' or multiple hosts concurrently via 'hosts'. Prefer 'hosts' to query multiple machines simultaneously. If 'hosts' is provided, returns a JSON object mapping hostnames to their respective results (status, stdout, stderr, exit_code).",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    },
                                    "command": {
                                        "type": "string",
                                        "description": "The command to run on the remote host(s)"
                                    },
                                    "abbreviate": {
                                        "type": "boolean",
                                        "description": "If true, abbreviate extremely long stdout (defaults to false)"
                                    },
                                    "max_lines": {
                                        "type": "integer",
                                        "description": "Maximum lines of stdout to retain when abbreviate is true (default: 100)"
                                    }
                                },
                                "required": ["command"]
                            }
                        },
                        {
                            "name": "search_processes",
                            "description": "Search running processes matching a pattern/regex on a single host via 'host' or multiple hosts concurrently via 'hosts'. Filter processes by regex 'pattern' case-insensitively. If using 'hosts', returns a JSON map mapping hostnames to their respective array of matching process objects. Prefer 'hosts' to query multiple machines simultaneously.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    },
                                    "pattern": {
                                        "type": "string",
                                        "description": "The regex or substring pattern to filter process command lines (case-insensitive)"
                                    },
                                    "full_info": {
                                        "type": "boolean",
                                        "description": "If true, returns detailed stats (PID, USER, %CPU, %MEM, Command). If false, returns a concise list of PIDs and commands. Default: false."
                                    }
                                },
                                "required": ["pattern"]
                            }
                        },
                        {
                            "name": "tail_log",
                            "description": "Fetch the last N lines of a remote log file from a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their success status and log output. Prefer 'hosts' to monitor logs on multiple machines simultaneously.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    },
                                    "file_path": {
                                        "type": "string",
                                        "description": "The absolute path of the log file"
                                    },
                                    "lines": {
                                        "type": "integer",
                                        "description": "Number of lines to retrieve (default: 50)"
                                    }
                                },
                                "required": ["file_path"]
                            }
                        },
                        {
                            "name": "tail_container_logs",
                            "description": "Fetch the last N lines of logs from a remote Docker container on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their success status and container log output. Prefer 'hosts' to query container logs across multiple machines simultaneously.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    },
                                    "container": {
                                        "type": "string",
                                        "description": "The Docker container name or ID"
                                    },
                                    "lines": {
                                        "type": "integer",
                                        "description": "Number of lines to retrieve (default: 50)"
                                    },
                                    "timestamps": {
                                        "type": "boolean",
                                        "description": "Include timestamps in the log output (default: false)"
                                    }
                                },
                                "required": ["container"]
                            }
                        },
                        {
                            "name": "wait_for_log_pattern",
                            "description": "Blocks and streams a remote log file or Docker container logs on a single host ('host') or multiple hosts concurrently ('hosts') until a regex 'pattern' is matched or a timeout is reached. If using 'hosts', returns a JSON map of hostnames to success/error/timeout statuses containing the matched line. Extremely useful for verifying startup or events across cluster nodes without polling.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    },
                                    "file_path": {
                                        "type": "string",
                                        "description": "The absolute path of the log file to stream (provide either file_path or container)"
                                    },
                                    "container": {
                                        "type": "string",
                                        "description": "The Docker container name or ID to stream (provide either file_path or container)"
                                    },
                                    "pattern": {
                                        "type": "string",
                                        "description": "The regex pattern to wait for (case-insensitive)"
                                    },
                                    "timeout_secs": {
                                        "type": "integer",
                                        "description": "Maximum seconds to wait before timing out (default: 30)"
                                    }
                                },
                                "required": ["pattern"]
                            }
                        },
                        {
                            "name": "get_system_stats",
                            "description": "Fetch remote system statistics (load averages, memory, and disk usage) on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to system stats objects. Prefer 'hosts' to get health/metrics for multiple servers concurrently.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    }
                                },
                                "required": []
                            }
                        },
                        {
                            "name": "list_ports",
                            "description": "List active listening TCP and UDP ports on a single host ('host') or multiple hosts concurrently ('hosts'), with optional port filtering. If using 'hosts', returns a JSON map mapping hostnames to listening ports arrays. Prefer 'hosts' to scan ports or debug connections across multiple systems simultaneously.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "SSH host alias from ~/.ssh/config. Provide either 'host' or 'hosts', but not both."
                                    },
                                    "hosts": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Array of SSH host aliases from ~/.ssh/config to query concurrently. Returns tagged JSON mapping host to result."
                                    },
                                    "port": {
                                        "type": "integer",
                                        "description": "Optional port number to filter by"
                                    }
                                },
                                "required": []
                            }
                        }
                    ]
                });

                // Add custom tools from config
                let mut tools_val = tools;
                if let Some(tools_arr) = tools_val.get_mut("tools").and_then(|t| t.as_array_mut()) {
                    let config = crate::ssh_pool::load_config();
                    let mut sorted_tools: Vec<(&String, &crate::ssh_pool::PreparedTool)> =
                        config.tools.iter().collect();
                    sorted_tools.sort_by(|a, b| a.0.cmp(b.0));

                    for (name, tool) in sorted_tools {
                        // Remove native tool with same name to enforce custom precedence/override
                        tools_arr.retain(|t| {
                            t.get("name").and_then(|n| n.as_str()) != Some(name.as_str())
                        });

                        let mut properties = serde_json::Map::new();
                        properties.insert("host".to_string(), serde_json::json!({
                            "type": "string",
                            "description": "The SSH host to query. Provide either 'host' or 'hosts', but not both."
                        }));
                        properties.insert(
                            "hosts".to_string(),
                            serde_json::json!({
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional: list of SSH hosts to query concurrently"
                            }),
                        );

                        let mut required = Vec::new();
                        let mut sorted_params: Vec<(&String, &crate::ssh_pool::ParamInfo)> =
                            tool.params.iter().collect();
                        sorted_params.sort_by(|a, b| a.0.cmp(b.0));
                        for (param_name, param_info) in sorted_params {
                            properties.insert(param_name.clone(), serde_json::json!({
                                "type": "string",
                                "description": format!("Parameter: {} (validation: {})", param_name, param_info.validation)
                            }));
                            required.push(param_name.clone());
                        }

                        tools_arr.push(serde_json::json!({
                            "name": name,
                            "description": tool.description,
                            "inputSchema": {
                                "type": "object",
                                "properties": properties,
                                "required": required
                            }
                        }));
                    }
                }

                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(tools_val),
                    error: None,
                }
            }
            "tools/call" => match self.handle_tools_call(req.params).await {
                Ok(res) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(res),
                    error: None,
                },
                Err(e) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: e.to_string(),
                        data: None,
                    }),
                },
            },
            _ => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", method),
                    data: None,
                }),
            },
        }
    }

    async fn handle_tools_call(
        &self,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let params = params.ok_or_else(|| anyhow::anyhow!("Missing params for tools/call"))?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid name field in tools/call"))?;

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        // Check for custom tool first to enforce custom precedence/override
        let config = crate::ssh_pool::load_config();
        if let Some(tool) = config.tools.get(name) {
            let (hosts, is_multi) = parse_hosts(&arguments)?;

            // Extract, validate, and collect parameter values
            let mut param_values = std::collections::HashMap::new();
            for (param_name, param_info) in &tool.params {
                let val_opt = arguments.get(param_name).and_then(|v| v.as_str());
                let val = match val_opt {
                    Some(v) => v.to_string(),
                    None => {
                        return Ok(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": format!("Error: Missing required parameter '{}' for tool '{}'", param_name, name)
                            }],
                            "isError": true
                        }));
                    }
                };

                // Validate the parameter value
                if !crate::ssh_pool::validate_param_value(&val, &param_info.validation) {
                    return Ok(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "Error: Parameter '{}' failed validation with rule '{}'. Provided value: {:?}",
                                param_name,
                                param_info.validation,
                                val
                            )
                        }],
                        "isError": true
                    }));
                }

                param_values.insert(param_name.clone(), val);
            }

            // Verify hosts against allow_hosts if specified on the tool
            let ssh_config = crate::ssh_config::load_ssh_config().unwrap_or_default();
            for host in &hosts {
                let real_host = ssh_config
                    .query(host)
                    .host_name
                    .as_deref()
                    .unwrap_or(host)
                    .to_string();
                if !crate::ssh_pool::is_host_allowed_for_tool(
                    host,
                    Some(&real_host),
                    &tool.allow_hosts,
                ) {
                    return Ok(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Error: Access to host '{}' is not allowed for tool '{}'", host, name)
                        }],
                        "isError": true
                    }));
                }
            }

            // Construct the final command to run
            let cmd_to_run = match &tool.command {
                crate::ssh_pool::CommandTemplate::Simple(cmd_str) => {
                    match crate::ssh_pool::replace_placeholders(cmd_str, &param_values) {
                        Ok(cmd) => cmd,
                        Err(e) => {
                            return Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": format!("Error interpolating command: {:#}", e) }],
                                "isError": true
                            }));
                        }
                    }
                }
                crate::ssh_pool::CommandTemplate::Array(cmd_array) => {
                    let mut substituted_args = Vec::new();
                    let mut interpolate_err = None;
                    for arg in cmd_array {
                        match crate::ssh_pool::replace_placeholders(arg, &param_values) {
                            Ok(subbed) => substituted_args.push(subbed),
                            Err(e) => {
                                interpolate_err = Some(e);
                                break;
                            }
                        }
                    }
                    if let Some(e) = interpolate_err {
                        return Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error interpolating command: {:#}", e) }],
                            "isError": true
                        }));
                    }
                    crate::ssh_pool::shell_join(&substituted_args)
                }
            };

            let run_custom = {
                let pool = self.pool.clone();
                let cmd_to_run = cmd_to_run.clone();
                let name = name.to_string();
                move |host: String| {
                    let pool = pool.clone();
                    let cmd_to_run = cmd_to_run.clone();
                    let name = name.clone();
                    async move {
                        let (stdout, stderr, exit_code) =
                            pool.execute_command(&host, &cmd_to_run).await?;
                        if exit_code != 0 {
                            anyhow::bail!(
                                "Error executing custom tool '{}' (exit code {}):\n{}",
                                name,
                                exit_code,
                                stderr
                            );
                        }
                        Ok(serde_json::json!(stdout))
                    }
                }
            };

            if is_multi {
                let results = execute_on_hosts(hosts, 15, run_custom).await?;
                let text = serde_json::to_string_pretty(&results)?;
                return Ok(serde_json::json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }));
            } else {
                let host = &hosts[0];
                match run_custom(host.to_string()).await {
                    Ok(stdout_val) => {
                        let text = stdout_val.as_str().unwrap_or("").to_string();
                        return Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": text }],
                            "isError": false
                        }));
                    }
                    Err(e) => {
                        return Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        }));
                    }
                }
            }
        }

        match name {
            "list_hosts" => match list_ssh_hosts() {
                Ok(hosts) => {
                    let ssh_config = crate::ssh_config::load_ssh_config().unwrap_or_default();
                    let filtered_hosts: Vec<String> = hosts
                        .into_iter()
                        .filter(|h| {
                            let real_host = ssh_config
                                .query(h)
                                .host_name
                                .as_deref()
                                .unwrap_or(h)
                                .to_string();
                            !crate::ssh_pool::is_host_ignored(h, Some(&real_host))
                        })
                        .collect();
                    let text = serde_json::to_string_pretty(&filtered_hosts)?;
                    Ok(serde_json::json!({
                        "content": [
                            {
                                "type": "text",
                                "text": text
                            }
                        ],
                        "isError": false
                    }))
                }
                Err(e) => Ok(serde_json::json!({
                    "content": [
                        {
                            "type": "text",
                            "text": format!("Error listing hosts: {}", e)
                        }
                    ],
                    "isError": true
                })),
            },
            "get_system_stats" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let run_stats = {
                    let pool = self.pool.clone();
                    move |host: String| {
                        let pool = pool.clone();
                        async move {
                            let cmd = "echo '=== LOAD ===' && (cat /proc/loadavg 2>/dev/null || uptime) && echo '=== MEM ===' && (cat /proc/meminfo 2>/dev/null || free -k 2>/dev/null) && echo '=== DISK ===' && df -kP /";
                            let (stdout, stderr, exit_code) =
                                pool.execute_command(&host, cmd).await?;
                            if exit_code != 0 {
                                anyhow::bail!(
                                    "Error executing stats command (exit code {}):\n{}",
                                    exit_code,
                                    stderr
                                );
                            }
                            let stats = parse_system_stats(&stdout);
                            Ok(serde_json::to_value(&stats)?)
                        }
                    }
                };

                if is_multi {
                    let results = execute_on_hosts(hosts, 15, run_stats).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_stats(host.to_string()).await {
                        Ok(stats_val) => {
                            let text = serde_json::to_string_pretty(&stats_val)?;
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        })),
                    }
                }
            }
            "list_ports" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let filter_port = arguments
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32);

                let run_ports = {
                    let pool = self.pool.clone();
                    move |host: String| {
                        let pool = pool.clone();
                        async move {
                            let cmd = "ss -tulpn 2>/dev/null || ss -tuln 2>/dev/null || netstat -tulpn 2>/dev/null || netstat -tuln 2>/dev/null";
                            let (stdout, stderr, exit_code) =
                                pool.execute_command(&host, cmd).await?;
                            if exit_code != 0 {
                                anyhow::bail!(
                                    "Error executing ports command (exit code {}):\n{}",
                                    exit_code,
                                    stderr
                                );
                            }
                            let ports = parse_listening_ports(&stdout, filter_port);
                            Ok(serde_json::to_value(&ports)?)
                        }
                    }
                };

                if is_multi {
                    let results = execute_on_hosts(hosts, 15, run_ports).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_ports(host.to_string()).await {
                        Ok(ports_val) => {
                            let text = serde_json::to_string_pretty(&ports_val)?;
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        })),
                    }
                }
            }
            "run_command" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let command = arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;

                let abbreviate = arguments
                    .get("abbreviate")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let max_lines = arguments
                    .get("max_lines")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(100);

                let run_cmd = {
                    let pool = self.pool.clone();
                    let command = command.to_string();
                    move |host: String| {
                        let pool = pool.clone();
                        let command = command.clone();
                        async move {
                            let (stdout, stderr, exit_code) =
                                pool.execute_command(&host, &command).await?;
                            let stdout_final = if abbreviate {
                                abbreviate_output(&stdout, max_lines)
                            } else {
                                stdout
                            };
                            Ok(serde_json::json!({
                                "stdout": stdout_final,
                                "stderr": stderr,
                                "exit_code": exit_code
                            }))
                        }
                    }
                };

                if is_multi {
                    let results = execute_on_hosts(hosts, 15, run_cmd).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_cmd(host.to_string()).await {
                        Ok(result_payload) => {
                            let text = serde_json::to_string_pretty(&result_payload)?;
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        })),
                    }
                }
            }
            "search_processes" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let pattern = arguments
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;

                let full_info = arguments
                    .get("full_info")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Build case-insensitive regex
                let re = regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| anyhow::anyhow!("Invalid regex pattern: {}", e))?;

                let run_search = {
                    let pool = self.pool.clone();
                    let re = re.clone();
                    move |host: String| {
                        let pool = pool.clone();
                        let re = re.clone();
                        async move {
                            // POSIX-standard process listing
                            let (stdout, stderr, exit_code) = pool
                                .execute_command(&host, "ps -eo pid,user,%cpu,%mem,args")
                                .await?;
                            if exit_code != 0 {
                                anyhow::bail!(
                                    "Error running ps command (exit code {}):\n{}",
                                    exit_code,
                                    stderr
                                );
                            }

                            let mut results = Vec::new();
                            for line in stdout.lines() {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }

                                // Split by whitespace
                                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                                if parts.len() < 5 {
                                    continue;
                                }

                                // If the first column doesn't parse as PID, it's the header row or invalid
                                let pid = match parts[0].parse::<u32>() {
                                    Ok(p) => p,
                                    Err(_) => continue,
                                };

                                let user = parts[1];
                                let cpu = parts[2];
                                let mem = parts[3];
                                let command = parts[4..].join(" ");

                                // Filter using the regex on the command line
                                if re.is_match(&command) {
                                    if full_info {
                                        results.push(serde_json::json!({
                                            "pid": pid,
                                            "user": user,
                                            "cpu": cpu,
                                            "mem": mem,
                                            "command": command
                                        }));
                                    } else {
                                        results.push(serde_json::json!({
                                            "pid": pid,
                                            "command": command
                                        }));
                                    }
                                }
                            }
                            Ok(serde_json::Value::Array(results))
                        }
                    }
                };

                if is_multi {
                    let results = execute_on_hosts(hosts, 15, run_search).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_search(host.to_string()).await {
                        Ok(results_val) => {
                            let text = serde_json::to_string_pretty(&results_val)?;
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        })),
                    }
                }
            }
            "tail_log" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let file_path = arguments
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' argument"))?;

                let lines = arguments
                    .get("lines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50);

                let run_tail = {
                    let pool = self.pool.clone();
                    let file_path = file_path.to_string();
                    move |host: String| {
                        let pool = pool.clone();
                        let file_path = file_path.clone();
                        async move {
                            let command = format!("tail -n {} {}", lines, file_path);
                            let (stdout, stderr, exit_code) =
                                pool.execute_command(&host, &command).await?;
                            if exit_code != 0 {
                                anyhow::bail!(
                                    "Error tailing file (exit code {}):\n{}",
                                    exit_code,
                                    stderr
                                );
                            }
                            Ok(serde_json::json!(stdout))
                        }
                    }
                };

                if is_multi {
                    let results = execute_on_hosts(hosts, 15, run_tail).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_tail(host.to_string()).await {
                        Ok(stdout_val) => {
                            let text = stdout_val.as_str().unwrap_or("").to_string();
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        })),
                    }
                }
            }
            "tail_container_logs" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let container = arguments
                    .get("container")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'container' argument"))?;

                let lines = arguments
                    .get("lines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50);

                let timestamps = arguments
                    .get("timestamps")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let run_container_logs = {
                    let pool = self.pool.clone();
                    let container = container.to_string();
                    move |host: String| {
                        let pool = pool.clone();
                        let container = container.clone();
                        async move {
                            let ts_flag = if timestamps { "-t" } else { "" };
                            let command =
                                format!("docker logs --tail {} {} {}", lines, ts_flag, container);
                            let (stdout, stderr, exit_code) =
                                pool.execute_command(&host, &command).await?;
                            if exit_code != 0 {
                                anyhow::bail!(
                                    "Error fetching container logs (exit code {}):\n{}",
                                    exit_code,
                                    stderr
                                );
                            }
                            let text = if stdout.is_empty() && !stderr.is_empty() {
                                stderr
                            } else {
                                stdout
                            };
                            Ok(serde_json::json!(text))
                        }
                    }
                };

                if is_multi {
                    let results = execute_on_hosts(hosts, 15, run_container_logs).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_container_logs(host.to_string()).await {
                        Ok(stdout_val) => {
                            let text = stdout_val.as_str().unwrap_or("").to_string();
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        })),
                    }
                }
            }
            "wait_for_log_pattern" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let file_path = arguments
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let container = arguments
                    .get("container")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if file_path.is_none() && container.is_none() {
                    return Err(anyhow::anyhow!(
                        "Provide either 'file_path' or 'container' argument"
                    ));
                }
                if file_path.is_some() && container.is_some() {
                    return Err(anyhow::anyhow!(
                        "Provide either 'file_path' or 'container', not both"
                    ));
                }

                let pattern = arguments
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;

                let timeout_secs = arguments
                    .get("timeout_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30);

                let re = regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| anyhow::anyhow!("Invalid regex pattern: {}", e))?;

                let run_wait_pattern = {
                    let pool = self.pool.clone();
                    let file_path = file_path.clone();
                    let container = container.clone();
                    let re = re.clone();
                    let pattern = pattern.to_string();
                    move |host: String| {
                        let pool = pool.clone();
                        let file_path = file_path.clone();
                        let container = container.clone();
                        let re = re.clone();
                        let pattern = pattern.clone();
                        async move {
                            let cmd = if let Some(path) = &file_path {
                                format!("tail -f -n 10 {}", path)
                            } else {
                                format!("docker logs -f --tail 10 {}", container.as_ref().unwrap())
                            };

                            let handle = pool.get_connection(&host).await?;
                            let _guard = pool.start_operation(&host).await;
                            let mut channel = handle
                                .channel_open_session()
                                .await
                                .context("Failed to open SSH channel")?;

                            channel
                                .exec(true, cmd)
                                .await
                                .context("Failed to execute tail/log command")?;

                            let mut stdout_buf = Vec::new();
                            let mut matched_line = None;
                            let mut error_msg = None;

                            let sleep_duration = Duration::from_millis(50);
                            let start_time = std::time::Instant::now();
                            let timeout = Duration::from_secs(timeout_secs);

                            loop {
                                if start_time.elapsed() >= timeout {
                                    error_msg = Some(format!(
                                        "Timed out after {} seconds waiting for pattern '{}'",
                                        timeout_secs, pattern
                                    ));
                                    break;
                                }

                                match tokio::time::timeout(sleep_duration, channel.wait()).await {
                                    Ok(Some(russh::ChannelMsg::Data { data })) => {
                                        stdout_buf.extend_from_slice(&data);

                                        let mut found = false;
                                        while let Some(pos) =
                                            stdout_buf.iter().position(|&b| b == b'\n')
                                        {
                                            let line_bytes: Vec<u8> =
                                                stdout_buf.drain(..=pos).collect();
                                            let line_str = String::from_utf8_lossy(
                                                &line_bytes[..line_bytes.len() - 1],
                                            )
                                            .into_owned();
                                            if re.is_match(&line_str) {
                                                matched_line = Some(line_str);
                                                found = true;
                                                break;
                                            }
                                        }
                                        if found {
                                            break;
                                        }
                                    }
                                    Ok(Some(russh::ChannelMsg::ExtendedData { data, ext })) => {
                                        if ext == 1 {
                                            stdout_buf.extend_from_slice(&data);

                                            let mut found = false;
                                            while let Some(pos) =
                                                stdout_buf.iter().position(|&b| b == b'\n')
                                            {
                                                let line_bytes: Vec<u8> =
                                                    stdout_buf.drain(..=pos).collect();
                                                let line_str = String::from_utf8_lossy(
                                                    &line_bytes[..line_bytes.len() - 1],
                                                )
                                                .into_owned();
                                                if re.is_match(&line_str) {
                                                    matched_line = Some(line_str);
                                                    found = true;
                                                    break;
                                                }
                                            }
                                            if found {
                                                break;
                                            }
                                        }
                                    }
                                    Ok(Some(russh::ChannelMsg::ExitStatus { exit_status })) => {
                                        if exit_status != 0 {
                                            error_msg = Some(format!(
                                                "Command exited with status {}",
                                                exit_status
                                            ));
                                        }
                                        break;
                                    }
                                    Ok(None) => {
                                        break;
                                    }
                                    Err(_) => {
                                        // Timeout elapsed, loop again to check total timeout
                                    }
                                    _ => {}
                                }
                            }

                            if matched_line.is_none()
                                && error_msg.is_none()
                                && !stdout_buf.is_empty()
                            {
                                let line_str = String::from_utf8_lossy(&stdout_buf).into_owned();
                                if re.is_match(&line_str) {
                                    matched_line = Some(line_str);
                                }
                            }

                            let _ = channel.close().await;

                            if let Some(line) = matched_line {
                                Ok(serde_json::json!(line))
                            } else {
                                let err = error_msg.unwrap_or_else(|| {
                                    "Connection closed before pattern was matched".to_string()
                                });
                                anyhow::bail!("{}", err);
                            }
                        }
                    }
                };

                if is_multi {
                    let results =
                        execute_on_hosts(hosts, timeout_secs + 5, run_wait_pattern).await?;
                    let text = serde_json::to_string_pretty(&results)?;
                    Ok(serde_json::json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }))
                } else {
                    let host = &hosts[0];
                    match run_wait_pattern(host.to_string()).await {
                        Ok(matched_line_val) => {
                            let line = matched_line_val.as_str().unwrap_or("").to_string();
                            Ok(serde_json::json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!("Pattern matched! Line found:\n{}", line)
                                }],
                                "isError": false
                            }))
                        }
                        Err(e) => Ok(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": format!("{:#}", e)
                            }],
                            "isError": true
                        })),
                    }
                }
            }
            _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SystemStats {
    pub load_averages: Vec<f64>,
    pub memory: MemoryStats,
    pub disks: Vec<DiskStats>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MemoryStats {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: Option<u64>,
    pub used_kb: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DiskStats {
    pub filesystem: String,
    pub size_kb: u64,
    pub used_kb: u64,
    pub available_kb: u64,
    pub use_percent: u32,
    pub mount_point: String,
}

fn parse_system_stats(raw_output: &str) -> SystemStats {
    let mut load_averages = Vec::new();
    let mut memory = MemoryStats {
        total_kb: 0,
        free_kb: 0,
        available_kb: None,
        used_kb: 0,
    };
    let mut disks = Vec::new();

    let parts: Vec<&str> = raw_output.split("=== ").collect();
    for part in parts {
        if part.starts_with("LOAD ===\n") {
            let content = part.trim_start_matches("LOAD ===\n");
            if let Some(first_line) = content.lines().next() {
                let tokens: Vec<&str> = first_line.split_whitespace().collect();
                if tokens.len() >= 3 && tokens[0].parse::<f64>().is_ok() {
                    for t in &tokens[..3] {
                        if let Ok(val) = t.parse::<f64>() {
                            load_averages.push(val);
                        }
                    }
                } else if let Some(pos) = first_line.rfind("load average:") {
                    let avg_str = &first_line[pos + 13..];
                    for t in avg_str.split(',') {
                        if let Ok(val) = t.trim().parse::<f64>() {
                            load_averages.push(val);
                        }
                    }
                } else if let Some(pos) = first_line.rfind("load averages:") {
                    let avg_str = &first_line[pos + 14..];
                    for t in avg_str.split_whitespace() {
                        if let Ok(val) = t.trim_matches(',').parse::<f64>() {
                            load_averages.push(val);
                        }
                    }
                }
            }
        } else if part.starts_with("MEM ===\n") {
            let content = part.trim_start_matches("MEM ===\n");
            let mut total = None;
            let mut free = None;
            let mut avail = None;

            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("MemTotal:") {
                    total = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok());
                } else if trimmed.starts_with("MemFree:") {
                    free = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok());
                } else if trimmed.starts_with("MemAvailable:") {
                    avail = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok());
                }
            }

            if let (Some(t), Some(f)) = (total, free) {
                memory.total_kb = t;
                memory.free_kb = f;
                memory.available_kb = avail;
                memory.used_kb = t.saturating_sub(avail.unwrap_or(f));
            } else {
                for line in content.lines() {
                    let parts_mem: Vec<&str> = line.split_whitespace().collect();
                    if parts_mem.len() >= 4 && parts_mem[0].starts_with("Mem:") {
                        let parsed = (
                            parts_mem[1].parse::<u64>(),
                            parts_mem[2].parse::<u64>(),
                            parts_mem[3].parse::<u64>(),
                        );
                        if let (Ok(t), Ok(u), Ok(f)) = parsed {
                            memory.total_kb = t;
                            memory.free_kb = f;
                            memory.used_kb = u;
                            if parts_mem.len() >= 7 {
                                memory.available_kb = parts_mem[6].parse::<u64>().ok();
                            }
                        }
                    }
                }
            }
        } else if part.starts_with("DISK ===\n") {
            let content = part.trim_start_matches("DISK ===\n");
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with("Filesystem") {
                    continue;
                }
                let parts_disk: Vec<&str> = line.split_whitespace().collect();
                if parts_disk.len() >= 6 {
                    let fs = parts_disk[0].to_string();
                    if fs == "tmpfs"
                        || fs == "devtmpfs"
                        || fs == "udev"
                        || fs.starts_with("/dev/loop")
                    {
                        continue;
                    }
                    if let (Ok(size), Ok(used), Ok(avail)) = (
                        parts_disk[1].parse::<u64>(),
                        parts_disk[2].parse::<u64>(),
                        parts_disk[3].parse::<u64>(),
                    ) {
                        let pct = parts_disk[4]
                            .trim_end_matches('%')
                            .parse::<u32>()
                            .unwrap_or(0);
                        let mount = parts_disk[5].to_string();
                        disks.push(DiskStats {
                            filesystem: fs,
                            size_kb: size,
                            used_kb: used,
                            available_kb: avail,
                            use_percent: pct,
                            mount_point: mount,
                        });
                    }
                }
            }
        }
    }

    SystemStats {
        load_averages,
        memory,
        disks,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ListeningPort {
    pub proto: String,
    pub local_address: String,
    pub port: u32,
    pub process: Option<String>,
    pub pid: Option<u32>,
}

fn parse_listening_ports(raw_output: &str, filter_port: Option<u32>) -> Vec<ListeningPort> {
    let mut results = Vec::new();
    for line in raw_output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Active")
            || line.starts_with("Proto")
            || line.starts_with("Netid")
        {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }

        let proto = parts[0].to_lowercase();
        if !proto.contains("tcp") && !proto.contains("udp") {
            continue;
        }
        let clean_proto = if proto.contains("tcp") {
            "tcp".to_string()
        } else {
            "udp".to_string()
        };

        let local_addr_str = if parts.len() >= 5 && parts[4].contains(':') {
            parts[4]
        } else if parts[3].contains(':') {
            parts[3]
        } else {
            let mut found = None;
            for p in &parts[3..] {
                if p.contains(':') {
                    found = Some(*p);
                    break;
                }
            }
            match found {
                Some(f) => f,
                None => continue,
            }
        };

        let last_colon = match local_addr_str.rfind(':') {
            Some(idx) => idx,
            None => continue,
        };

        let local_address = local_addr_str[..last_colon].to_string();
        let port_str = &local_addr_str[last_colon + 1..];
        let port = match port_str.parse::<u32>() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if filter_port.is_some_and(|fp| port != fp) {
            continue;
        }

        let mut process = None;
        let mut pid = None;

        let remaining_line = line;
        if let Some(pos) = remaining_line.find('/') {
            let parts_slash: Vec<&str> = remaining_line[..pos].split_whitespace().collect();
            if let Some(pid_val) = parts_slash.last().and_then(|t| t.parse::<u32>().ok()) {
                pid = Some(pid_val);
                let after_slash = &remaining_line[pos + 1..];
                if let Some(space_pos) = after_slash.find(char::is_whitespace) {
                    process = Some(after_slash[..space_pos].to_string());
                } else {
                    process = Some(after_slash.to_string());
                }
            }
        } else if let Some(pid_idx) = remaining_line.find("pid=") {
            let pid_str = &remaining_line[pid_idx + 4..];
            if let Some(pid_val) = pid_str
                .split(',')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
            {
                pid = Some(pid_val);
            }
            if let Some(users_idx) = remaining_line.find("users:((\"") {
                let proc_str = &remaining_line[users_idx + 9..];
                if let Some(quote_pos) = proc_str.find('"') {
                    process = Some(proc_str[..quote_pos].to_string());
                }
            }
        }

        results.push(ListeningPort {
            proto: clean_proto,
            local_address,
            port,
            process,
            pid,
        });
    }

    let mut seen = std::collections::HashSet::new();
    results.retain(|p| seen.insert((p.proto.clone(), p.port)));

    results
}

pub fn parse_hosts(arguments: &serde_json::Value) -> anyhow::Result<(Vec<String>, bool)> {
    if let Some(hosts_val) = arguments.get("hosts") {
        let hosts: Vec<String> = if let Some(arr) = hosts_val.as_array() {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        } else if let Some(s) = hosts_val.as_str() {
            vec![s.to_string()]
        } else {
            anyhow::bail!("Invalid 'hosts' argument: must be an array of strings");
        };
        if hosts.is_empty() {
            anyhow::bail!("The 'hosts' list cannot be empty");
        }
        Ok((hosts, true))
    } else if let Some(host_val) = arguments.get("host").and_then(|v| v.as_str()) {
        Ok((vec![host_val.to_string()], false))
    } else {
        anyhow::bail!(
            "Missing target host(s): specify either 'host' (string) or 'hosts' (array of strings)"
        );
    }
}

pub async fn execute_on_hosts<F, Fut>(
    hosts: Vec<String>,
    timeout_secs: u64,
    f: F,
) -> anyhow::Result<serde_json::Value>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<serde_json::Value>> + Send,
{
    let mut join_set = tokio::task::JoinSet::new();
    let f_arc = std::sync::Arc::new(f);
    for host in hosts {
        let f_clone = f_arc.clone();
        join_set.spawn(async move {
            let fut = f_clone(host.clone());
            let timeout_dur = std::time::Duration::from_secs(timeout_secs);
            let result = tokio::time::timeout(timeout_dur, fut).await;

            let payload = match result {
                Ok(Ok(val)) => serde_json::json!({
                    "status": "success",
                    "data": val
                }),
                Ok(Err(e)) => serde_json::json!({
                    "status": "error",
                    "error": format!("{:#}", e)
                }),
                Err(_) => serde_json::json!({
                    "status": "timeout",
                    "error": format!("Timed out after {} seconds", timeout_secs)
                }),
            };
            (host, payload)
        });
    }

    let mut results_map = serde_json::Map::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok((host, payload)) => {
                results_map.insert(host, payload);
            }
            Err(e) => {
                eprintln!("Task join error in execute_on_hosts: {:?}", e);
            }
        }
    }
    Ok(serde_json::Value::Object(results_map))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abbreviate_output() {
        let input =
            "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10";
        // Max 4 lines (keeping 2 at head, 2 at tail)
        let output = abbreviate_output(input, 4);
        let expected = "line 1\nline 2\n... [6 lines truncated] ...\nline 9\nline 10\n";
        assert_eq!(output, expected);

        // Under limit should be unmodified
        let output_under = abbreviate_output(input, 20);
        assert_eq!(output_under, input.to_string());
    }

    #[test]
    fn test_parse_ps_line() {
        // Mock ps output line
        let line = " 1234 richard   0.5  1.2 /usr/local/bin/localmail serve --port 80";
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts[0].parse::<u32>().unwrap(), 1234);
        assert_eq!(parts[1], "richard");
        assert_eq!(parts[2], "0.5");
        assert_eq!(parts[3], "1.2");
        assert_eq!(
            parts[4..].join(" "),
            "/usr/local/bin/localmail serve --port 80"
        );

        // Header line should not parse as PID
        let header = "  PID USER      %CPU %MEM COMMAND";
        let parts_header: Vec<&str> = header.split_whitespace().collect();
        assert!(parts_header[0].parse::<u32>().is_err());
    }

    #[test]
    fn test_custom_command_interpolation() {
        // Test template replacement and appending
        let cmd_template = "grep -i '{args}' /var/log/syslog";
        let args = "error";
        let cmd_to_run = if cmd_template.contains("{args}") {
            cmd_template.replace("{args}", args)
        } else if !args.is_empty() {
            format!("{} {}", cmd_template, args)
        } else {
            cmd_template.to_string()
        };
        assert_eq!(cmd_to_run, "grep -i 'error' /var/log/syslog");

        let cmd_simple = "apt list --upgradable";
        let args_simple = "some_extra";
        let cmd_to_run_simple = if cmd_simple.contains("{args}") {
            cmd_simple.replace("{args}", args_simple)
        } else if !args_simple.is_empty() {
            format!("{} {}", cmd_simple, args_simple)
        } else {
            cmd_simple.to_string()
        };
        assert_eq!(cmd_to_run_simple, "apt list --upgradable some_extra");
    }

    #[test]
    fn test_parse_system_stats() {
        let raw = "\
=== LOAD ===
0.15 0.08 0.05 1/450 12345
=== MEM ===
MemTotal:       16278272 kB
MemFree:         4829104 kB
MemAvailable:   11000200 kB
=== DISK ===
Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/sda1        105291040  45192040  60099000      43% /
tmpfs              8139136         0   8139136       0% /dev/shm
";
        let stats = parse_system_stats(raw);
        assert_eq!(stats.load_averages, vec![0.15, 0.08, 0.05]);
        assert_eq!(stats.memory.total_kb, 16278272);
        assert_eq!(stats.memory.free_kb, 4829104);
        assert_eq!(stats.memory.available_kb, Some(11000200));
        assert_eq!(stats.memory.used_kb, 16278272 - 11000200);

        assert_eq!(stats.disks.len(), 1);
        assert_eq!(stats.disks[0].filesystem, "/dev/sda1");
        assert_eq!(stats.disks[0].size_kb, 105291040);
        assert_eq!(stats.disks[0].used_kb, 45192040);
        assert_eq!(stats.disks[0].available_kb, 60099000);
        assert_eq!(stats.disks[0].use_percent, 43);
        assert_eq!(stats.disks[0].mount_point, "/");
    }

    #[test]
    fn test_parse_listening_ports() {
        let raw_ss = "\
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
tcp   LISTEN 0      4096         0.0.0.0:80          0.0.0.0:*     users:((\"nginx\",pid=123,fd=6))
tcp   LISTEN 0      4096            [::]:80             [::]:*     users:((\"nginx\",pid=123,fd=6))
udp   UNCONN 0      0            0.0.0.0:53          0.0.0.0:*     users:((\"named\",pid=456,fd=7))
";
        let ports = parse_listening_ports(raw_ss, None);
        assert_eq!(ports.len(), 2);

        assert_eq!(ports[0].proto, "tcp");
        assert_eq!(ports[0].local_address, "0.0.0.0");
        assert_eq!(ports[0].port, 80);
        assert_eq!(ports[0].process, Some("nginx".to_string()));
        assert_eq!(ports[0].pid, Some(123));

        assert_eq!(ports[1].proto, "udp");
        assert_eq!(ports[1].local_address, "0.0.0.0");
        assert_eq!(ports[1].port, 53);
        assert_eq!(ports[1].process, Some("named".to_string()));
        assert_eq!(ports[1].pid, Some(456));

        // Test filter port
        let filtered = parse_listening_ports(raw_ss, Some(53));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].port, 53);
    }

    #[test]
    fn test_parse_hosts_helper() {
        // Test single host
        let args_single = serde_json::json!({ "host": "localhost" });
        let (hosts, is_multi) = parse_hosts(&args_single).unwrap();
        assert!(!is_multi);
        assert_eq!(hosts, vec!["localhost".to_string()]);

        // Test multiple hosts as array
        let args_multi = serde_json::json!({ "hosts": ["host1", "host2"] });
        let (hosts, is_multi) = parse_hosts(&args_multi).unwrap();
        assert!(is_multi);
        assert_eq!(hosts, vec!["host1".to_string(), "host2".to_string()]);

        // Test multiple hosts as string
        let args_multi_str = serde_json::json!({ "hosts": "host1" });
        let (hosts, is_multi) = parse_hosts(&args_multi_str).unwrap();
        assert!(is_multi);
        assert_eq!(hosts, vec!["host1".to_string()]);

        // Test error cases
        let args_none = serde_json::json!({});
        assert!(parse_hosts(&args_none).is_err());

        let args_empty_arr = serde_json::json!({ "hosts": [] });
        assert!(parse_hosts(&args_empty_arr).is_err());
    }

    #[tokio::test]
    async fn test_execute_on_hosts_helper() {
        let hosts = vec!["host1".to_string(), "host2".to_string()];
        let run_fn = |_host: String| async { Ok(serde_json::json!("hello")) };

        let result = execute_on_hosts(hosts, 5, run_fn).await.unwrap();
        let map = result.as_object().unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("host1").unwrap().get("status").unwrap(), "success");
        assert_eq!(map.get("host1").unwrap().get("data").unwrap(), "hello");
        assert_eq!(map.get("host2").unwrap().get("status").unwrap(), "success");
        assert_eq!(map.get("host2").unwrap().get("data").unwrap(), "hello");
    }
}

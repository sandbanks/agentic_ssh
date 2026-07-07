use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ssh_config::list_ssh_hosts;
use crate::ssh_pool::ConnectionPool;

pub mod parsers;
pub mod tools;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Id {
    String(String),
    Number(i64),
    Null,
}

#[derive(Deserialize, Debug)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
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
pub fn abbreviate_output(output: &str, max_lines: usize) -> String {
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
        lines.len() - (keep * 2)
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
                    let resp_str = serde_json::to_string(&resp)?;
                    stdout.write_all(resp_str.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                    continue;
                }
            };

            let resp = self.handle_request(req).await;
            if resp.id.is_some() {
                let resp_str = serde_json::to_string(&resp)?;
                stdout.write_all(resp_str.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
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
            "initialized" => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: None,
            },
            "ping" => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::json!({})),
                error: None,
            },
            "tools/list" => {
                let config = crate::ssh_pool::load_config();
                let mut tools_arr = vec![
                    serde_json::json!({
                        "name": "list_hosts",
                        "description": "Returns the list of configured remote SSH hosts. Useful to see what remote machines are available to target.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }),
                    serde_json::json!({
                        "name": "get_system_stats",
                        "description": "Fetch CPU load average, RAM, and disk utilization metrics on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their metrics. Prefer 'hosts' to query cluster status in parallel.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                }
                            }
                        }
                    }),
                    serde_json::json!({
                        "name": "list_ports",
                        "description": "Lists active listening TCP/UDP ports, matching processes, and PIDs on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their active port list. Optionally filter by 'port'. Prefer 'hosts' to scan service availability across multiple machines simultaneously.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                },
                                "port": {
                                    "type": "integer",
                                    "description": "Optional port number to filter by"
                                }
                            }
                        }
                    }),
                    serde_json::json!({
                        "name": "run_command",
                        "description": "Executes a shell command on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their stdout, stderr, and exit codes. Features optional 'background' execution (starting the command and returning a local log file path immediately for async tracking), 'quiet' execution (suppressing progress logs), and output abbreviation controls. Prefer 'hosts' to execute commands across cluster nodes simultaneously.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                },
                                "command": {
                                    "type": "string",
                                    "description": "The shell command to run on target"
                                },
                                "quiet": {
                                    "type": "boolean",
                                    "description": "If true, suppresses terminal progress logging in background files (default: false)"
                                },
                                "progress_interval_secs": {
                                    "type": "integer",
                                    "description": "Number of seconds between progress reporting updates (default: 5)"
                                },
                                "background": {
                                    "type": "boolean",
                                    "description": "If true, runs command in background and returns log path immediately (default: false)"
                                },
                                "abbreviate": {
                                    "type": "boolean",
                                    "description": "If true, limits long stdout output (default: true)"
                                },
                                "max_lines": {
                                    "type": "integer",
                                    "description": "Max lines to return if abbreviate is true (default: 100)"
                                }
                            },
                            "required": ["command"]
                        }
                    }),
                    serde_json::json!({
                        "name": "search_processes",
                        "description": "Searches running processes on a single host ('host') or multiple hosts concurrently ('hosts') matching a regex 'pattern'. If using 'hosts', returns a JSON map mapping hostnames to their matched process list. Optionally returns full user/CPU/mem stats if 'full_info' is true. Prefer 'hosts' to find running services across multiple cluster nodes simultaneously.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                },
                                "pattern": {
                                    "type": "string",
                                    "description": "Regex pattern to match against command lines (case-insensitive)"
                                },
                                "full_info": {
                                    "type": "boolean",
                                    "description": "If true, includes user, %cpu, %mem in output (default: false)"
                                }
                            },
                            "required": ["pattern"]
                        }
                    }),
                    serde_json::json!({
                        "name": "tail_log",
                        "description": "Fetch the last N lines of a remote log file on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their log output. Prefer 'hosts' to query logs across multiple machines simultaneously.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                },
                                "file_path": {
                                    "type": "string",
                                    "description": "Absolute path to log file on remote target"
                                },
                                "lines": {
                                    "type": "integer",
                                    "description": "Number of lines to read from the end (default: 100)"
                                }
                            },
                            "required": ["file_path"]
                        }
                    }),
                    serde_json::json!({
                        "name": "tail_container_logs",
                        "description": "Fetch the last N lines of logs from a remote Docker container on a single host ('host') or multiple hosts concurrently ('hosts'). If using 'hosts', returns a JSON map mapping hostnames to their success status and container log output. Prefer 'hosts' to query container logs across multiple machines simultaneously.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                },
                                "container": {
                                    "type": "string",
                                    "description": "The Docker container name or ID"
                                },
                                "lines": {
                                    "type": "integer",
                                    "description": "Number of lines to read from the end (default: 100)"
                                },
                                "timestamps": {
                                    "type": "boolean",
                                    "description": "If true, includes timestamps in output (default: false)"
                                }
                            },
                            "required": ["container"]
                        }
                    }),
                    serde_json::json!({
                        "name": "wait_for_log_pattern",
                        "description": "Blocks and streams a remote log file or Docker container logs on a single host ('host') or multiple hosts concurrently ('hosts') until a regex 'pattern' is matched or a timeout is reached. If using 'hosts', returns a JSON map of hostnames to success/error/timeout statuses containing the matched line. Extremely useful for verifying startup or events across cluster nodes without polling.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "host": {
                                    "type": "string",
                                    "description": "The target hostname or IP address"
                                },
                                "hosts": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "A list of hostname targets to query concurrently"
                                },
                                "file_path": {
                                    "type": "string",
                                    "description": "Absolute path to log file on remote target (provide either file_path or container)"
                                },
                                "container": {
                                    "type": "string",
                                    "description": "The Docker container name or ID to stream (provide either file_path or container)"
                                },
                                "pattern": {
                                    "type": "string",
                                    "description": "Regex pattern to match"
                                },
                                "timeout_secs": {
                                    "type": "integer",
                                    "description": "Maximum time to block (default: 60)"
                                }
                            },
                            "required": ["pattern"]
                        }
                    }),
                ];

                for (name, tool) in &config.tools {
                    let mut properties = serde_json::Map::new();
                    let mut required = Vec::new();

                    properties.insert(
                        "host".to_string(),
                        serde_json::json!({
                            "type": "string",
                            "description": "The target hostname or IP address"
                        }),
                    );
                    properties.insert(
                        "hosts".to_string(),
                        serde_json::json!({
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "A list of hostname targets to query concurrently"
                        }),
                    );

                    for (param_name, param_info) in &tool.params {
                        properties.insert(
                            param_name.clone(),
                            serde_json::json!({
                                "type": "string",
                                "description": format!("Custom parameter (validation rule: {})", param_info.validation)
                            }),
                        );
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

                let tools_val = serde_json::json!({
                    "tools": tools_arr
                });

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

        let config = crate::ssh_pool::load_config();
        if let Some(tool) = config.tools.get(name) {
            let (hosts, is_multi) = parse_hosts(&arguments)?;

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

            let cmd_to_run = match &tool.command {
                crate::ssh_pool::CommandTemplate::Simple(template) => {
                    let mut substituted = template.clone();
                    for (param_name, val) in &param_values {
                        let placeholder = format!("{{{{{}}}}}", param_name);
                        substituted = substituted.replace(&placeholder, val);
                    }
                    substituted
                }
                crate::ssh_pool::CommandTemplate::Array(args_templates) => {
                    let mut substituted_args = Vec::new();
                    for arg_tpl in args_templates {
                        let mut substituted = arg_tpl.clone();
                        for (param_name, val) in &param_values {
                            let placeholder = format!("{{{{{}}}}}", param_name);
                            substituted = substituted.replace(&placeholder, val);
                        }
                        substituted_args.push(substituted);
                    }
                    if !tool.allow_shell {
                        return Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": "Error interpolating command: Array template format requires allow_shell = true".to_string() }],
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
                        let (stdout, stderr, exit_code) = pool
                            .execute_command(
                                &host,
                                &cmd_to_run,
                                true,
                                5,
                                std::env::temp_dir().join("agentic_ssh_temp.log"),
                            )
                            .await?;
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

            return run_tool_on_hosts(hosts, is_multi, 15, run_custom).await;
        }

        match name {
            "list_groups" => {
                let config = crate::ssh_pool::load_config();
                let text = serde_json::to_string_pretty(&config.groups)?;
                Ok(serde_json::json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }))
            }
            "list_hosts" => {
                let hosts = list_ssh_hosts()?;
                let text = serde_json::to_string_pretty(&hosts)?;
                Ok(serde_json::json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }))
            }
            "get_system_stats" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let pool = self.pool.clone();
                let run_stats = move |host: String| {
                    let pool = pool.clone();
                    async move { tools::run_get_system_stats(&pool, &host).await }
                };
                run_tool_on_hosts(hosts, is_multi, 15, run_stats).await
            }
            "list_ports" => {
                let (hosts, is_multi) = parse_hosts(&arguments)?;
                let filter_port = arguments
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                let pool = self.pool.clone();
                let run_ports = move |host: String| {
                    let pool = pool.clone();
                    async move { tools::run_list_ports(&pool, &host, filter_port).await }
                };
                run_tool_on_hosts(hosts, is_multi, 15, run_ports).await
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
                    .unwrap_or(true);
                let max_lines = arguments
                    .get("max_lines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100) as usize;
                let quiet = arguments
                    .get("quiet")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let progress_interval_secs = arguments
                    .get("progress_interval_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5);
                let background = arguments
                    .get("background")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let pool = self.pool.clone();
                let command = command.to_string();
                let run_cmd = move |host: String| {
                    let pool = pool.clone();
                    let command = command.clone();
                    async move {
                        tools::run_run_command(
                            &pool,
                            &host,
                            &command,
                            quiet,
                            progress_interval_secs,
                            background,
                            abbreviate,
                            max_lines,
                        )
                        .await
                    }
                };

                run_tool_on_hosts(hosts, is_multi, 15, run_cmd).await
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

                let re = regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| anyhow::anyhow!("Invalid regex pattern: {}", e))?;

                let pool = self.pool.clone();
                let re = re.clone();
                let run_search = move |host: String| {
                    let pool = pool.clone();
                    let re = re.clone();
                    async move { tools::run_search_processes(&pool, &host, re, full_info).await }
                };

                run_tool_on_hosts(hosts, is_multi, 15, run_search).await
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
                    .unwrap_or(100) as usize;

                let pool = self.pool.clone();
                let file_path = file_path.to_string();
                let run_tail = move |host: String| {
                    let pool = pool.clone();
                    let file_path = file_path.clone();
                    async move { tools::run_tail_log(&pool, &host, &file_path, lines).await }
                };

                run_tool_on_hosts(hosts, is_multi, 15, run_tail).await
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
                    .unwrap_or(100) as usize;
                let timestamps = arguments
                    .get("timestamps")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let pool = self.pool.clone();
                let container = container.to_string();
                let run_container_logs = move |host: String| {
                    let pool = pool.clone();
                    let container = container.clone();
                    async move {
                        tools::run_tail_container_logs(&pool, &host, &container, lines, timestamps)
                            .await
                    }
                };

                run_tool_on_hosts(hosts, is_multi, 15, run_container_logs).await
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
                        "Specify either 'file_path' (string) or 'container' (string)"
                    ));
                }

                let pattern = arguments
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;
                let timeout_secs = arguments
                    .get("timeout_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(60);

                let re = regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| anyhow::anyhow!("Invalid regex pattern: {}", e))?;

                let pool = self.pool.clone();
                let file_path = file_path.clone();
                let container = container.clone();
                let re = re.clone();
                let pattern_str = pattern.to_string();
                let run_wait_pattern = move |host: String| {
                    let pool = pool.clone();
                    let file_path = file_path.clone();
                    let container = container.clone();
                    let re = re.clone();
                    let pattern_str = pattern_str.clone();
                    async move {
                        tools::run_wait_for_log_pattern(
                            &pool,
                            &host,
                            file_path,
                            container,
                            re,
                            &pattern_str,
                            timeout_secs,
                        )
                        .await
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

pub fn expand_host_recursive(
    host_or_group: &str,
    groups: &std::collections::HashMap<String, Vec<String>>,
    visited: &mut std::collections::HashSet<String>,
    expanded: &mut Vec<String>,
) {
    if groups.contains_key(host_or_group) {
        if visited.insert(host_or_group.to_string()) {
            if let Some(members) = groups.get(host_or_group) {
                for member in members {
                    expand_host_recursive(member, groups, visited, expanded);
                }
            }
            visited.remove(host_or_group);
        }
    } else {
        expanded.push(host_or_group.to_string());
    }
}

pub fn resolve_hosts(
    hosts_input: &[String],
    groups: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut expanded = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for host in hosts_input {
        expand_host_recursive(host, groups, &mut visited, &mut expanded);
    }

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for h in expanded {
        if seen.insert(h.clone()) {
            result.push(h);
        }
    }
    result
}

pub fn parse_hosts(arguments: &serde_json::Value) -> Result<(Vec<String>, bool)> {
    let config = crate::ssh_pool::load_config();
    let groups = &config.groups;

    if let Some(hosts_val) = arguments.get("hosts") {
        let input_hosts: Vec<String> = if let Some(arr) = hosts_val.as_array() {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        } else if let Some(s) = hosts_val.as_str() {
            vec![s.to_string()]
        } else {
            anyhow::bail!("Invalid 'hosts' argument: must be an array of strings");
        };
        if input_hosts.is_empty() {
            anyhow::bail!("The 'hosts' list cannot be empty");
        }
        let expanded = resolve_hosts(&input_hosts, groups);
        Ok((expanded, true))
    } else if let Some(host_val) = arguments.get("host").and_then(|v| v.as_str()) {
        if groups.contains_key(host_val) {
            let expanded = resolve_hosts(&[host_val.to_string()], groups);
            Ok((expanded, true))
        } else {
            Ok((vec![host_val.to_string()], false))
        }
    } else {
        anyhow::bail!(
            "Missing target host(s): specify either 'host' (string) or 'hosts' (array of strings)"
        );
    }
}

pub fn find_matched_line(buf: &mut Vec<u8>, re: &regex::Regex) -> Option<String> {
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
        let line_str = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]).into_owned();
        if re.is_match(&line_str) {
            return Some(line_str);
        }
    }
    None
}

async fn run_tool_on_hosts<F, Fut>(
    hosts: Vec<String>,
    is_multi: bool,
    timeout_secs: u64,
    f: F,
) -> Result<serde_json::Value>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<serde_json::Value>> + Send,
{
    if is_multi {
        let results = execute_on_hosts(hosts, timeout_secs, f).await?;
        let text = serde_json::to_string_pretty(&results)?;
        Ok(serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false
        }))
    } else {
        let host = &hosts[0];
        match f(host.to_string()).await {
            Ok(val) => {
                let text = if let serde_json::Value::String(s) = val {
                    s
                } else {
                    serde_json::to_string_pretty(&val)?
                };
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

pub async fn execute_on_hosts<F, Fut>(
    hosts: Vec<String>,
    timeout_secs: u64,
    f: F,
) -> Result<serde_json::Value>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<serde_json::Value>> + Send,
{
    let mut join_set = tokio::task::JoinSet::new();
    let f_arc = Arc::new(f);
    for host in hosts {
        let f_clone = f_arc.clone();
        join_set.spawn(async move {
            let fut = f_clone(host.clone());
            let timeout_dur = Duration::from_secs(timeout_secs);
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
        let output = abbreviate_output(input, 4);
        let expected = "line 1\nline 2\n... [6 lines truncated] ...\nline 9\nline 10\n";
        assert_eq!(output, expected);

        let output_under = abbreviate_output(input, 20);
        assert_eq!(output_under, input.to_string());
    }

    #[test]
    fn test_custom_command_interpolation() {
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
    fn test_parse_hosts_helper() {
        let args_single = serde_json::json!({ "host": "localhost" });
        let (hosts, is_multi) = parse_hosts(&args_single).unwrap();
        assert!(!is_multi);
        assert_eq!(hosts, vec!["localhost".to_string()]);

        let args_multi = serde_json::json!({ "hosts": ["host1", "host2"] });
        let (hosts, is_multi) = parse_hosts(&args_multi).unwrap();
        assert!(is_multi);
        assert_eq!(hosts, vec!["host1".to_string(), "host2".to_string()]);

        let args_multi_str = serde_json::json!({ "hosts": "host1" });
        let (hosts, is_multi) = parse_hosts(&args_multi_str).unwrap();
        assert!(is_multi);
        assert_eq!(hosts, vec!["host1".to_string()]);

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

    #[test]
    fn test_resolve_hosts_recursive() {
        let mut groups = std::collections::HashMap::new();
        groups.insert(
            "ubuntu".to_string(),
            vec!["server-a".to_string(), "server-b".to_string()],
        );
        groups.insert("nix".to_string(), vec!["server-c".to_string()]);
        groups.insert(
            "all".to_string(),
            vec![
                "ubuntu".to_string(),
                "nix".to_string(),
                "other-host".to_string(),
            ],
        );
        groups.insert("circular-a".to_string(), vec!["circular-b".to_string()]);
        groups.insert(
            "circular-b".to_string(),
            vec!["circular-a".to_string(), "target-host".to_string()],
        );

        let resolved = resolve_hosts(&["ubuntu".to_string()], &groups);
        assert_eq!(
            resolved,
            vec!["server-a".to_string(), "server-b".to_string()]
        );

        let resolved = resolve_hosts(&["all".to_string(), "server-a".to_string()], &groups);
        assert_eq!(
            resolved,
            vec![
                "server-a".to_string(),
                "server-b".to_string(),
                "server-c".to_string(),
                "other-host".to_string()
            ]
        );

        let resolved = resolve_hosts(&["circular-a".to_string()], &groups);
        assert_eq!(resolved, vec!["target-host".to_string()]);
    }

    #[tokio::test]
    async fn test_list_groups_tool() {
        let server = McpServer::new(Duration::from_secs(300));
        let params = serde_json::json!({
            "name": "list_groups",
            "arguments": {}
        });
        let result = server.handle_tools_call(Some(params)).await.unwrap();
        assert!(!result.get("isError").unwrap().as_bool().unwrap());
        let content = result.get("content").unwrap().as_array().unwrap();
        assert_eq!(content.len(), 1);
        let text_val = content[0].get("text").unwrap().as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text_val).unwrap();
        assert!(parsed.is_object());
    }
}

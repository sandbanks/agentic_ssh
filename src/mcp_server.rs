use std::sync::Arc;
use std::time::Duration;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
    result.push_str(&format!("... [{} lines truncated] ...\n", lines.len() - max_lines));
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
                    eprintln!("Failed to parse JSON-RPC request: {:?}. Raw line: {}", e, trimmed);
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

    async fn send_response(&self, stdout: &mut tokio::io::Stdout, resp: &JsonRpcResponse) -> Result<()> {
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
                        "version": "0.1.0"
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
            "ping" => {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(serde_json::json!({})),
                    error: None,
                }
            }
            "tools/list" => {
                let tools = serde_json::json!({
                    "tools": [
                        {
                            "name": "list_hosts",
                            "description": "List all configured SSH hosts found in ~/.ssh/config",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "run_command",
                            "description": "Execute a shell command on an SSH host. Uses pooled connection.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "The SSH host alias from ~/.ssh/config to run the command on"
                                    },
                                    "command": {
                                        "type": "string",
                                        "description": "The command to run on the remote host"
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
                                "required": ["host", "command"]
                            }
                        },
                        {
                            "name": "search_processes",
                            "description": "Search running processes on a remote host matching a pattern/regex, returning structured JSON results to save tokens.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "The SSH host to query"
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
                                "required": ["host", "pattern"]
                            }
                        },
                        {
                            "name": "tail_log",
                            "description": "Fetch the last N lines of a remote log file.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "The SSH host to query"
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
                                "required": ["host", "file_path"]
                            }
                        },
                        {
                            "name": "tail_container_logs",
                            "description": "Fetch the last N lines of logs from a remote Docker container.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "The SSH host to query"
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
                                "required": ["host", "container"]
                            }
                        },
                        {
                            "name": "wait_for_log_pattern",
                            "description": "Streams a log file or Docker container logs and blocks until a regex pattern is matched or timeout occurs. Efficiently alerts the agent when an event happens.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "The SSH host to query"
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
                                "required": ["host", "pattern"]
                            }
                        }
                    ]
                });

                // Add custom tools from config
                let mut tools_val = tools;
                if let Some(tools_arr) = tools_val.get_mut("tools").and_then(|t| t.as_array_mut()) {
                    let config = crate::ssh_pool::load_config();
                    for custom in config.custom_tools {
                        tools_arr.push(serde_json::json!({
                            "name": custom.name,
                            "description": custom.description,
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "host": {
                                        "type": "string",
                                        "description": "The SSH host to query"
                                    },
                                    "args": {
                                        "type": "string",
                                        "description": "Optional arguments/parameters to pass to the command (replaces {args} in the template or is appended)"
                                    }
                                },
                                "required": ["host"]
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
            "tools/call" => {
                match self.handle_tools_call(req.params).await {
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
                }
            }
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

    async fn handle_tools_call(&self, params: Option<serde_json::Value>) -> Result<serde_json::Value> {
        let params = params.ok_or_else(|| anyhow::anyhow!("Missing params for tools/call"))?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing or invalid name field in tools/call"))?;
        
        let arguments = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

        match name {
            "list_hosts" => {
                match list_ssh_hosts() {
                    Ok(hosts) => {
                        let text = serde_json::to_string_pretty(&hosts)?;
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
                    Err(e) => {
                        Ok(serde_json::json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("Error listing hosts: {}", e)
                                }
                            ],
                            "isError": true
                        }))
                    }
                }
            }
            "run_command" => {
                let host = arguments
                    .get("host")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'host' argument"))?;
                
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

                match self.pool.execute_command(host, command).await {
                    Ok((stdout, stderr, exit_code)) => {
                        let stdout_final = if abbreviate {
                            abbreviate_output(&stdout, max_lines)
                        } else {
                            stdout
                        };

                        let result_payload = serde_json::json!({
                            "stdout": stdout_final,
                            "stderr": stderr,
                            "exit_code": exit_code
                        });

                        let text = serde_json::to_string_pretty(&result_payload)?;
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
                    Err(e) => {
                        Ok(serde_json::json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("Error: {:#}", e)
                                }
                            ],
                            "isError": true
                        }))
                    }
                }
            }
            "search_processes" => {
                let host = arguments
                    .get("host")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'host' argument"))?;
                
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

                // POSIX-standard process listing
                match self.pool.execute_command(host, "ps -eo pid,user,%cpu,%mem,args").await {
                    Ok((stdout, stderr, exit_code)) => {
                        if exit_code != 0 {
                            let text = format!("Error running ps command (exit code {}):\n{}", exit_code, stderr);
                            return Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": true
                            }));
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

                        let text = serde_json::to_string_pretty(&results)?;
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
                    Err(e) => {
                        Ok(serde_json::json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("Error: {:#}", e)
                                }
                            ],
                            "isError": true
                        }))
                    }
                }
            }
            "tail_log" => {
                let host = arguments
                    .get("host")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'host' argument"))?;
                
                let file_path = arguments
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' argument"))?;

                let lines = arguments
                    .get("lines")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50);

                let command = format!("tail -n {} {}", lines, file_path);

                match self.pool.execute_command(host, &command).await {
                    Ok((stdout, stderr, exit_code)) => {
                        let is_error = exit_code != 0;
                        let text = if is_error {
                            format!("Error tailing file (exit code {}):\n{}", exit_code, stderr)
                        } else {
                            stdout
                        };
                        Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": text }],
                            "isError": is_error
                        }))
                    }
                    Err(e) => {
                        Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        }))
                    }
                }
            }
            "tail_container_logs" => {
                let host = arguments
                    .get("host")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'host' argument"))?;
                
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

                let ts_flag = if timestamps { "-t" } else { "" };
                let command = format!("docker logs --tail {} {} {}", lines, ts_flag, container);

                match self.pool.execute_command(host, &command).await {
                    Ok((stdout, stderr, exit_code)) => {
                        let is_error = exit_code != 0;
                        let text = if is_error {
                            format!("Error fetching container logs (exit code {}):\n{}", exit_code, stderr)
                        } else {
                            if stdout.is_empty() && !stderr.is_empty() {
                                stderr
                            } else {
                                stdout
                            }
                        };
                        Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": text }],
                            "isError": is_error
                        }))
                    }
                    Err(e) => {
                        Ok(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                            "isError": true
                        }))
                    }
                }
            }
            "wait_for_log_pattern" => {
                let host = arguments
                    .get("host")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'host' argument"))?;
                
                let file_path = arguments.get("file_path").and_then(|v| v.as_str());
                let container = arguments.get("container").and_then(|v| v.as_str());

                if file_path.is_none() && container.is_none() {
                    return Err(anyhow::anyhow!("Provide either 'file_path' or 'container' argument"));
                }
                if file_path.is_some() && container.is_some() {
                    return Err(anyhow::anyhow!("Provide either 'file_path' or 'container', not both"));
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

                let cmd = if let Some(path) = file_path {
                    format!("tail -f -n 10 {}", path)
                } else {
                    format!("docker logs -f --tail 10 {}", container.unwrap())
                };

                let handle = self.pool.get_connection(host).await?;
                let mut channel = handle.channel_open_session().await
                    .context("Failed to open SSH channel")?;
                
                channel.exec(true, cmd).await
                    .context("Failed to execute tail/log command")?;

                let mut stdout_buf = Vec::new();
                let mut matched_line = None;
                let mut error_msg = None;

                let sleep_duration = Duration::from_millis(50);
                let start_time = std::time::Instant::now();
                let timeout = Duration::from_secs(timeout_secs);

                loop {
                    if start_time.elapsed() >= timeout {
                        error_msg = Some(format!("Timed out after {} seconds waiting for pattern '{}'", timeout_secs, pattern));
                        break;
                    }

                    match tokio::time::timeout(sleep_duration, channel.wait()).await {
                        Ok(Some(russh::ChannelMsg::Data { data })) => {
                            stdout_buf.extend_from_slice(&data);
                            
                            let mut found = false;
                            while let Some(pos) = stdout_buf.iter().position(|&b| b == b'\n') {
                                let line_bytes: Vec<u8> = stdout_buf.drain(..=pos).collect();
                                let line_str = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]).into_owned();
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
                                while let Some(pos) = stdout_buf.iter().position(|&b| b == b'\n') {
                                    let line_bytes: Vec<u8> = stdout_buf.drain(..=pos).collect();
                                    let line_str = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]).into_owned();
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
                                error_msg = Some(format!("Command exited with status {}", exit_status));
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

                if matched_line.is_none() && error_msg.is_none() && !stdout_buf.is_empty() {
                    let line_str = String::from_utf8_lossy(&stdout_buf).into_owned();
                    if re.is_match(&line_str) {
                        matched_line = Some(line_str);
                    }
                }

                let _ = channel.close().await;

                if let Some(line) = matched_line {
                    Ok(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Pattern matched! Line found:\n{}", line)
                        }],
                        "isError": false
                    }))
                } else {
                    let err = error_msg.unwrap_or_else(|| "Connection closed before pattern was matched".to_string());
                    Ok(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": err
                        }],
                        "isError": true
                    }))
                }
            }
            _ => {
                let config = crate::ssh_pool::load_config();
                if let Some(custom) = config.custom_tools.iter().find(|t| t.name == name) {
                    let host = arguments
                        .get("host")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("Missing 'host' argument"))?;

                    let args = arguments
                        .get("args")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let cmd_to_run = if custom.command.contains("{args}") {
                        custom.command.replace("{args}", args)
                    } else if !args.is_empty() {
                        format!("{} {}", custom.command, args)
                    } else {
                        custom.command.clone()
                    };

                    match self.pool.execute_command(host, &cmd_to_run).await {
                        Ok((stdout, stderr, exit_code)) => {
                            let is_error = exit_code != 0;
                            let text = if is_error {
                                format!("Error executing custom tool '{}' (exit code {}):\n{}", name, exit_code, stderr)
                            } else {
                                stdout
                            };
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": is_error
                            }))
                        }
                        Err(e) => {
                            Ok(serde_json::json!({
                                "content": [{ "type": "text", "text": format!("Error: {:#}", e) }],
                                "isError": true
                            }))
                        }
                    }
                } else {
                    Err(anyhow::anyhow!("Unknown tool: {}", name))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abbreviate_output() {
        let input = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10";
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
        assert_eq!(parts[4..].join(" "), "/usr/local/bin/localmail serve --port 80");
        
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
}


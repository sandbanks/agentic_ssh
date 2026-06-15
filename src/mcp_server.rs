use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
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
                        }
                    ]
                });
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(tools),
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
            _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
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
}


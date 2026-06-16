# agentic_ssh

`agentic_ssh` is a Model Context Protocol (MCP) server written in Rust that simplifies SSH access for AI agents. It parses your local SSH configuration files to discover hosts, automatically manages a pool of authenticated SSH connections (closing them after 5 minutes of inactivity), and allows agents to execute remote commands without needing to handle login credentials or session teardown.

It also supports output abbreviation, which helps save tokens by truncating extremely long stdout results.

---

## Features

- **Automatic Host Discovery**: Parses `~/.ssh/config` recursively (including `Include` directives, wildcards, and path expansion) to extract explicit hosts.
- **Connection Pooling**: Maintains open SSH sessions and reuses them. Automatically reconnects if a connection drops.
- **Auto-Cleanup**: Closes pooled connections after 5 minutes of inactivity to conserve resource usage.
- **Key-Based Authentication**: Seamlessly authenticates using the `IdentityFile` paths configured in your SSH config, falling back to standard default keys (`~/.ssh/id_rsa`, `~/.ssh/id_ed25519`, etc.).
- **Token Saving / Output Abbreviation**: Offers an option to truncate large stdout outputs (e.g., keeping only the first and last $N$ lines, with a truncation notice in the middle).
- **High Performance**: Built with async Rust ([tokio](https://crates.io/crates/tokio) & [russh](https://crates.io/crates/russh)) and utilizes the `mimalloc` allocator.

---

## Installation & Building

Make sure you have Rust and Cargo installed (or use the Nix environment).

1. Clone or navigate to the repository.
2. Build the binary in release mode:
   ```bash
   cargo build --release
   ```
   The compiled binary will be located at:
   `target/release/agentic_ssh`
3. Copy the binary to a location in your PATH, e.g.:
   ```bash
   sudo cp target/release/agentic_ssh /usr/local/bin/
   ```

---

## MCP Configuration

To register `agentic_ssh` with an MCP client (such as Claude Desktop), add it to your configuration file:

### Claude Desktop Configuration
On macOS, edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "agentic_ssh": {
      "command": "/usr/local/bin/agentic_ssh",
      "args": []
    }
  }
}
```

---

## Configuration

`agentic_ssh` can be configured globally using a TOML config file located at `~/.config/agentic_ssh/config.toml` (this directory and file are optional).

### Configuration Options
- `pool_status_path` (string): The path to write/read the connection pool status JSON file. Supports absolute paths or tilde expansion (`~/`). Defaults to `~/.agentic_ssh_pool_status.json`.
- `custom_tools` (array of tables): Defines custom subcommands that dynamically register as first-class MCP tools.

Example `~/.config/agentic_ssh/config.toml`:
```toml
# Configuration for agentic_ssh

# Custom path to write the connection pool status JSON file
pool_status_path = "~/.agentic_ssh_pool_status.json"

# Register custom command abbreviations as first-class MCP tools
[[custom_tools]]
name = "list_upgradable"
description = "List all packages that can be upgraded on the host via apt"
command = "apt list --upgradable"

[[custom_tools]]
name = "grep_syslog"
description = "Grep syslog for a specific pattern"
command = "grep -i '{args}' /var/log/syslog"
```

### Custom Tools / Command Abbreviations
When you define a table in `custom_tools`, the MCP server dynamically registers it as a first-class tool when the client calls `tools/list`. 
Each custom tool automatically supports:
1. `host` (string, required): The target SSH host.
2. `args` (string, optional): Optional arguments. If the command template contains `{args}`, the placeholder is replaced by the value of `args`. Otherwise, if `args` is provided, it is appended to the command (separated by a space).

This allows developers to extend the MCP server without writing Rust code, simplifying agent usage and saving prompt tokens.

### Environment Overrides
For development or automated setups, you can override settings using environment variables, which take precedence over the configuration file:
- `AGENTIC_SSH_POOL_STATUS`: Path to the pool status file (e.g. `/tmp/pool_status.json`).

---

## Available Tools

The MCP server exposes two main tools to connected agents:

### 1. `list_hosts`
Lists all explicit SSH host aliases configured in your `~/.ssh/config` file.
- **Arguments**: None
- **Returns**: A JSON array of host strings (e.g., `["server-a", "web-server", "db-prod"]`).

### 2. `run_command`
Executes a shell command on one of the configured hosts.
- **Arguments**:
  - `host` (string, required): The SSH host alias from `~/.ssh/config` to run the command on.
  - `command` (string, required): The command to execute.
  - `abbreviate` (boolean, optional): If `true`, truncates extremely long stdout outputs. Defaults to `false`.
  - `max_lines` (integer, optional): Maximum number of lines to retain when `abbreviate` is true. Defaults to `100`.
- **Returns**: A JSON object containing:
  ```json
  {
    "stdout": "...",
    "stderr": "...",
    "exit_code": 0
  }
  ```

### 3. `search_processes`
Searches running processes on a remote host matching a pattern or regex, returning a structured JSON result list. Saves a massive amount of context tokens compared to dumping raw `ps` lists.
- **Arguments**:
  - `host` (string, required): The SSH host alias from `~/.ssh/config` to query.
  - `pattern` (string, required): A regex or substring pattern to filter process command lines (case-insensitive).
  - `full_info` (boolean, optional): If `true`, returns detailed statistics (`pid`, `user`, `cpu`, `mem`, `command`). If `false`, returns a concise list of `pid` and `command`. Defaults to `false`.
- **Returns**: A JSON array of matching process objects, e.g.:
  ```json
  [
    {
      "pid": 512,
      "command": "/usr/local/bin/localmail serve --port 80"
    }
  ]
  ```

### 4. `tail_log`
Fetch the last N lines of a remote log file.
- **Arguments**:
  - `host` (string, required): The SSH host alias from `~/.ssh/config` to query.
  - `file_path` (string, required): The absolute path of the remote log file.
  - `lines` (integer, optional): Number of lines to retrieve (default: 50).
- **Returns**: The tail output text.

### 5. `tail_container_logs`
Fetch the last N lines of logs from a remote Docker container.
- **Arguments**:
  - `host` (string, required): The SSH host alias from `~/.ssh/config` to query.
  - `container` (string, required): The Docker container name or ID.
  - `lines` (integer, optional): Number of lines to retrieve (default: 50).
  - `timestamps` (boolean, optional): Include timestamps in log output (default: false).
- **Returns**: The container logs output text.

### 6. `wait_for_log_pattern`
Streams a remote log file or Docker container logs and blocks until a regex pattern is matched or a timeout occurs. Extremely efficient for waiting for specific events (e.g. "server started") without polling or streaming full logs to the LLM.
- **Arguments**:
  - `host` (string, required): The SSH host alias from `~/.ssh/config` to query.
  - `file_path` (string, optional): Absolute path of log file (specify either `file_path` or `container`).
  - `container` (string, optional): Docker container name/ID (specify either `file_path` or `container`).
  - `pattern` (string, required): The regex pattern to wait for (case-insensitive).
  - `timeout_secs` (integer, optional): Maximum seconds to wait before timeout (default: 30).
- **Returns**: A confirmation string showing the matching line, or a timeout message.

### 7. `get_system_stats`
Fetch remote system statistics (load average, memory usage, disk space) as structured JSON.
- **Arguments**:
  - `host` (string, required): The SSH host alias to query.
- **Returns**: A JSON object containing `load_averages`, `memory` stats, and a list of `disks` filesystems and their usage.

### 8. `list_ports`
List active listening TCP and UDP ports on a remote host, with optional filtering by port number.
- **Arguments**:
  - `host` (string, required): The SSH host alias to query.
  - `port` (integer, optional): Optional port number to filter by.
- **Returns**: A JSON array of active port listener objects (protocol, local address, port, and process/PID if permission allows).

---

## Running Tests

To run the unit tests:
```bash
cargo test
```

---

## TUI Dashboard

You can run a live-updating TUI dashboard in your terminal to see the active SSH connections in the pool, how long ago they were last used, and how much time remains before they are automatically closed due to inactivity:

```bash
cargo run --release -- tui
# or:
./target/release/agentic_ssh tui
```

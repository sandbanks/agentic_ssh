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

## Installation

Ensure you have Rust and Cargo installed (or use the Nix environment).

### Install via Git
You can install `agentic_ssh` directly from the GitHub repository:
```bash
cargo install --git https://github.com/sandbanks/agentic_ssh
```

### Install from Source
Alternatively, you can clone the repository and install it locally:
```bash
git clone https://github.com/sandbanks/agentic_ssh.git
cd agentic_ssh
cargo install --path .
```
This builds the binary and places it in your Cargo binary directory (typically `~/.cargo/bin/`).

---

## MCP Configuration

To register `agentic_ssh` with an MCP client (such as Claude Desktop), add it to your configuration file:

### Claude Desktop Configuration
On macOS, edit `~/Library/Application Support/Claude/claude_desktop_config.json`. Note that Claude Desktop may require the absolute path to your home directory instead of `~`:

```json
{
  "mcpServers": {
    "agentic_ssh": {
      "command": "/Users/YOUR_USER_NAME/.cargo/bin/agentic_ssh",
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
- `ignore_hosts` (array of strings): A list of wildcard/glob patterns matching host aliases or resolved hostname destinations that agents should be strictly prevented from listing or connecting to (denylist).
- `allow_hosts` (array of strings): A list of wildcard/glob patterns (allowlist). If specified and not empty, the agent is strictly restricted *only* to hosts matching these patterns, blocking all others by default. (Also supports the alias `include_hosts`).

Example `~/.config/agentic_ssh/config.toml`:
```toml
# Configuration for agentic_ssh

# Custom path to write the connection pool status JSON file
pool_status_path = "~/.agentic_ssh_pool_status.json"

# Option A: Block specific hosts, allowing others by default
ignore_hosts = ["*.prod.company.com", "db-prod", "secure-*"]

# Option B: Block all hosts by default, only allowing specific ones
# ignore_hosts = ["*"]
# allow_hosts = ["*.staging.company.com", "dev-sandbox"]

# Register custom command abbreviations as first-class MCP tools
[[custom_tools]]
name = "list_upgradable"
description = "List all upgradable packages via apt. USE THIS instead of running apt manually."
command = "apt list --upgradable"

[[custom_tools]]
name = "grep_syslog"
description = "Search syslog for a pattern. USE THIS instead of ssh + grep."
command = "grep -i '{args}' /var/log/syslog"

[[custom_tools]]
name = "check_service_status"
description = "Check if a system service is running and its status"
command = "systemctl status {args} 2>/dev/null || service {args} status 2>/dev/null || echo 'Service command not found'"
#Use case: Use instead of: ssh host "systemctl status nginx"

[[custom_tools]]
name = "find_large_files"
description = ""Find files larger than N MB for disk cleanup. USE THIS instead of ssh + find. Pass size in MB (e.g., find_large_files 100)."
command = "find / -type f -size +{args}M -exec ls -lh {} + 2>/dev/null | sort -k5 -h | tail -20"
#Use case: Agent needs to identify space hogs when a disk is full. Call with find_large_files 100 or find_large_files 500.

[[custom_tools]]
name = "list_network_connections"
description = "List all active network connections with process info"
command = "ss -tulnp 2>/dev/null || netstat -tulnp 2>/dev/null"
#Use case: Agent needs to debug network issues, check what's listening on a port, or identify suspicious connections.

[[custom_tools]]
name = "check_docker_status"
description = "Check Docker container statuses"
command = "docker ps -a --format '{{.Names}}|{{.Status}}|{{.Ports}}' 2>/dev/null"
#Use case: Agent needs to check the status of Docker containers.

[[custom_tools]]
name = "list_cron_jobs"
description = "List all cron jobs for current user and root"
command = "crontab -l 2>/dev/null; echo '=== ROOT ==='; sudo crontab -l 2>/dev/null"
#Use case: Agent needs to list cron jobs for current user and root.
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

---

## Acknowledgments

*Standing on the shoulders of giants...*

- **Enzo** - MCP server installation code from tokensave
- [Future credits here...]

### License
MIT / Apache 2.0

[![Release](https://github.com/sandbanks/agentic_ssh/actions/workflows/release.yml/badge.svg)](https://github.com/sandbanks/agentic_ssh/actions/workflows/release.yml)

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
- **Zero-Quoting Headaches**: Structured argument lists (`allow_shell = false`) are automatically escaped and joined behind the scenes, completely eliminating the painful double/triple shell escaping normally required when running remote commands via `ssh host "command"`.
- **High Performance**: Built with async Rust ([tokio](https://crates.io/crates/tokio) & [russh](https://crates.io/crates/russh)) and utilizes the `mimalloc` allocator.

---

## Quick Start
```bash
# Install (recommended: pre-compiled binary via cargo-binstall)
cargo binstall agentic_ssh

# Or compile from source via cargo
cargo install agentic_ssh

# Auto-configure for your agent(s)
agentic_ssh install

# Verify it works
# (in your agent: list_hosts)
```

---

## Installation

Ensure you have Rust and Cargo installed (or use the Nix environment).

### Install via Cargo Binstall (Recommended)
You can install the pre-compiled binary for `agentic_ssh` using [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):
```bash
cargo binstall agentic_ssh
```

### Install via Cargo (Compile from Source)
You can also compile and install `agentic_ssh` directly from crates.io:
```bash
cargo install agentic_ssh
```

### Install via Git
You can also install `agentic_ssh` directly from the GitHub repository:
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

### Automated Installation (Recommended)

`agentic_ssh` provides built-in commands to automatically detect and register itself with supported AI agents (e.g. Claude Desktop, Claude Code, Cursor, Zed, Cline, Roo Code, Gemini CLI, Grok, etc.).

```bash
# Auto-detect all supported agents on your system and register agentic_ssh
agentic_ssh install

# Register specifically for a particular agent:
agentic_ssh install --agent claude
agentic_ssh install --agent cursor
agentic_ssh install --agent zed

# Or install project-scoped (local) configs:
agentic_ssh install --agent zed --local
```

Use `agentic_ssh uninstall` to remove the registration:
```bash
agentic_ssh uninstall
# Or for a specific agent:
agentic_ssh uninstall --agent claude
```

Supported agents for the `--agent` flag:
- `claude` (Claude Code / Claude Desktop)
- `cursor`
- `zed`
- `cline`
- `roo-code`
- `gemini`
- `copilot` (GitHub Copilot CLI)
- `grok`
- `kimi`
- `kilo`
- `kiro`
- `vibe`
- `opencode`
- `codex`
- `pi`
- `antigravity`

### Manual Configuration
To manually register `agentic_ssh` with an MCP client (such as Claude Desktop), add it to your configuration file:

#### Claude Desktop Configuration
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
- `tools` (table): Defines custom parameterized commands that dynamically register as first-class MCP tools.
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

# Register custom prepared commands as first-class MCP tools
[tools.list_upgradable]
description = "List all upgradable packages via apt. USE THIS instead of running apt manually."
command = ["apt", "list", "--upgradable"]

[tools.grep_syslog]
description = "Search syslog for a pattern. USE WITH CAUTION."
command = "grep -i '{{args}}' /var/log/syslog"
allow_shell = true
[tools.grep_syslog.params.args]
validation = "permissive"

[tools.check_service_status]
description = "Check if a system service is running and its status"
command = "systemctl status {{args}} 2>/dev/null || service {{args}} status 2>/dev/null || echo 'Service command not found'"
allow_shell = true
[tools.check_service_status.params.args]
validation = "permissive"

[tools.find_large_files]
description = "Find files larger than N MB for disk cleanup. Pass size in MB (e.g., find_large_files 100)."
command = "find / -type f -size +{{args}}M -exec ls -lh {} + 2>/dev/null | sort -k5 -h | tail -20"
allow_shell = true
[tools.find_large_files.params.args]
validation = "permissive"

[tools.list_network_connections]
description = "List all active network connections with process info"
command = ["ss", "-tulnp"]

[tools.check_docker_status]
description = "Check Docker container statuses"
command = "docker ps -a --format '{{.Names}}|{{.Status}}|{{.Ports}}' 2>/dev/null"
allow_shell = true

[tools.list_cron_jobs]
description = "List all cron jobs for current user and root"
command = "crontab -l 2>/dev/null; echo '=== ROOT ==='; sudo crontab -l 2>/dev/null"
allow_shell = true

[tools.git_pull]
description = "Fetches and merges latest changes from branch."
command = ["git", "-C", "/home/richard/Projects/ldk", "pull", "origin", "{{branch}}"]
allow_hosts = ["stan"]
[tools.git_pull.params.branch]
validation = "strict"
```

### Parameterized SSH Tools (SQL-Style Prepared Statements)
When you define a tool under `[tools]`, the MCP server dynamically registers it as a first-class tool when the client calls `tools/list`. 

Each custom tool automatically supports:
1. `host` (string, optional): The target SSH host alias. Provide either `host` or `hosts`, but not both.
2. `hosts` (array of strings, optional): Optional list of SSH host aliases from `~/.ssh/config` to query concurrently.
3. Command parameters: Any parameter placeholders (denoted by double curly braces `{{param_name}}`) defined in the `command` template. These are passed as key-value fields in the tool call arguments and must be defined in the `[tools.name.params]` block.

#### Security & Shell-Injection Defense
To eliminate shell injection vulnerabilities:
* **Argument Array Sandboxing**: By default, `allow_shell` is `false` and the command template is defined as an Array of strings. All parameters are evaluated strictly as data arguments, shell-escaped (rendering metacharacters like `;`, `&&`, or `|` inert), and executed directly.
* **Tiered Parameter Validation**: Before hitting the network, parameters are validated locally in Rust using three rules:
  - `strict`: Enforces pure alphanumeric + hyphens (perfect for branch names, docker tags, and service names).
  - `path`: Enforces alphanumeric plus safe path characters (`/`, `.`, `-`, `_`).
  - `permissive`: Bypasses character checks for trusted or complex payloads.
* **Explicit Shell Escape Hatch (`allow_shell = true`)**: If you explicitly need shell features (such as pipes or redirection), set `allow_shell = true` and define `command` as a single String.
* **Zero-Quoting Headaches**: Executing commands over standard SSH command line (`ssh host "command"`) is notoriously painful due to double-evaluation (first by the local shell, then by the remote login shell). Because parameters are passed via clean JSON-RPC arguments, `agentic_ssh` automatically quotes and escapes parameters behind the scenes, sparing you and your agent from command-escaping hell.

If `hosts` is used, the custom tool executes concurrently on all specified hosts and returns a JSON map mapping each host to its result status and output data.

This allows developers to extend the MCP server safely without writing Rust code, simplifying agent usage and saving prompt tokens.

### Environment Overrides
For development or automated setups, you can override settings using environment variables, which take precedence over the configuration file:
- `AGENTIC_SSH_POOL_STATUS`: Path to the pool status file (e.g. `/tmp/pool_status.json`).

---

## Available Tools

The MCP server exposes native tools supporting both **single-host execution** (using the `host` parameter) and **concurrent multi-host execution** (using the `hosts` parameter). 

> [!NOTE]
> When executing on multiple hosts concurrently via the `hosts` argument, the response is a tagged JSON object mapping each host to its result payload:
> ```json
> {
>   "host1": {
>     "status": "success",
>     "data": <normal_data_payload_or_stdout>
>   },
>   "host2": {
>     "status": "error",
>     "error": "Failed to connect: ..."
>   }
> }
> ```
> Single-host execution using the `host` argument retains the original return format (plain text or structured JSON) for backward compatibility.

### 1. `list_hosts`
Lists all explicit SSH host aliases configured in your `~/.ssh/config` file.
- **Arguments**: None
- **Returns**: A JSON array of host strings (e.g., `["server-a", "web-server", "db-prod"]`).

### 2. `run_command`
Executes a shell command on one or multiple configured hosts.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to run the command on.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to run the command on concurrently.
  - `command` (string, required): The command to execute.
  - `abbreviate` (boolean, optional): If `true`, truncates extremely long stdout outputs. Defaults to `false`.
  - `max_lines` (integer, optional): Maximum number of lines to retain when `abbreviate` is true. Defaults to `100`.
- **Returns**:
  - *Single host*: A JSON object containing `stdout`, `stderr`, and `exit_code`.
  - *Multi-host*: A JSON object mapping hostnames to their respective results.

### 3. `search_processes`
Searches running processes on a single host or multiple hosts matching a pattern or regex, returning a structured JSON result list.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to query.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to query concurrently.
  - `pattern` (string, required): A regex or substring pattern to filter process command lines (case-insensitive).
  - `full_info` (boolean, optional): If `true`, returns detailed statistics (`pid`, `user`, `cpu`, `mem`, `command`). If `false`, returns a concise list of `pid` and `command`. Defaults to `false`.
- **Returns**:
  - *Single host*: A JSON array of matching process objects.
  - *Multi-host*: A JSON object mapping hostnames to their respective arrays of process objects.

### 4. `tail_log`
Fetch the last N lines of a remote log file.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to query.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to query concurrently.
  - `file_path` (string, required): The absolute path of the remote log file.
  - `lines` (integer, optional): Number of lines to retrieve (default: 50).
- **Returns**:
  - *Single host*: The tail output text.
  - *Multi-host*: A JSON object mapping hostnames to their respective statuses and log outputs.

### 5. `tail_container_logs`
Fetch the last N lines of logs from a remote Docker container.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to query.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to query concurrently.
  - `container` (string, required): The Docker container name or ID.
  - `lines` (integer, optional): Number of lines to retrieve (default: 50).
  - `timestamps` (boolean, optional): Include timestamps in log output (default: false).
- **Returns**:
  - *Single host*: The container logs output text.
  - *Multi-host*: A JSON object mapping hostnames to their respective statuses and container logs.

### 6. `wait_for_log_pattern`
Streams a remote log file or Docker container logs and blocks until a regex pattern is matched or a timeout occurs.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to query.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to query concurrently.
  - `file_path` (string, optional): Absolute path of log file (specify either `file_path` or `container`).
  - `container` (string, optional): Docker container name/ID (specify either `file_path` or `container`).
  - `pattern` (string, required): The regex pattern to wait for (case-insensitive).
  - `timeout_secs` (integer, optional): Maximum seconds to wait before timeout (default: 30).
- **Returns**:
  - *Single host*: A confirmation string showing the matching line, or a timeout message.
  - *Multi-host*: A JSON object mapping hostnames to their respective matched line or error/timeout messages.

### 7. `get_system_stats`
Fetch remote system statistics (load average, memory usage, disk space) as structured JSON.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to query.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to query concurrently.
- **Returns**:
  - *Single host*: A JSON object containing `load_averages`, `memory` stats, and a list of `disks` filesystems and their usage.
  - *Multi-host*: A JSON object mapping hostnames to system stats objects.

### 8. `list_ports`
List active listening TCP and UDP ports on a remote host, with optional filtering by port number.
- **Arguments**:
  - `host` (string, optional): The SSH host alias to query.
  - `hosts` (array of strings, optional): Optional list of SSH host aliases to query concurrently.
  - `port` (integer, optional): Optional port number to filter by.
- **Returns**:
  - *Single host*: A JSON array of active port listener objects.
  - *Multi-host*: A JSON object mapping hostnames to listener arrays.

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

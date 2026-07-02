[![Release](https://github.com/sandbanks/agentic_ssh/actions/workflows/release.yml/badge.svg)](https://github.com/sandbanks/agentic_ssh/actions/workflows/release.yml)
[![Crates.io Recent Downloads](https://img.shields.io/crates/dr/agentic_ssh?color=orange "Crates.io Recent Downloads (90 Days)")](https://crates.io/crates/agentic_ssh)
[![Crates.io Total Downloads](https://img.shields.io/crates/d/agentic_ssh?color=orange "Crates.io Total Downloads")](https://crates.io/crates/agentic_ssh)
[![Homebrew Installs](https://img.shields.io/homebrew/installs/dy/sandbanks/tap/agentic_ssh?color=orange "Homebrew Installs (90 Days)")](https://github.com/sandbanks/homebrew-tap)

# agentic_ssh

`agentic_ssh` is a high-performance Model Context Protocol (MCP) server written in Rust specifically engineered to provide **agent-hardened, token-efficient, and asynchronous SSH orchestrations** for AI agents. 

Unlike generic terminal tools that block agent execution or flood context windows with verbose compiler output, `agentic_ssh` acts as a smart runtime layer. It automatically discovers hosts from your local SSH configurations, manages connection heartbeats, handles silent network dropouts, and supports detached background job scheduling with isolated session logging.

---

## Quick Start

```bash
# Install (recommended: pre-compiled binary via cargo-binstall)
cargo binstall agentic_ssh

# Or compile from source via cargo
cargo install agentic_ssh

# Auto-configure and register with your active agent environments
agentic_ssh install

```

---

## High-Leverage Features

* **Asynchronous Background Orchestration (`background: true`)**: Instantly hands control back to the agent runner when long-running scripts (such as massive Nix setups, system updates, or heavy compilation tasks) are fired. The agent is immediately freed to process other workflows concurrently while the remote task crunches in the background.
* **Token-Efficient Session Telemetry**: Completely routes around agent frameworks that swallow local `stderr`. Long jobs stream progress to unique, self-cleaning session log files (`~/.agentic_ssh/sessions/*.log`), preventing concurrent agent instances from clobbering each other's outputs.
* **Smart Progress Tickers (`quiet: true`)**: Keeps human terminal dashboards updated via a configurable time-interval progress loop without transferring a single line of raw compilation text over the internet, saving massive amounts of LLM context tokens.
* **Hardened Connection Heartbeats**: Automatically configures `russh` keepalives to ping remote environments every 30 seconds, preventing intermediate overlays (like Tailscale, cloud virtual routers, or firewalls) from silently dropping connections during long, quiet CPU-heavy builds.
* **Automatic Host Discovery**: Recursively parses `~/.ssh/config` (including `Include` directives, wildcards, and path expansion) to securely extract explicit host aliases.
* **Zero-Quoting Headaches**: Structured argument templates are automatically escaped and joined natively behind the scenes, completely eliminating the painful double/triple shell escaping errors normally generated when agents try to quote remote utilities over standard SSH shells.

---

## MCP Configuration

### Automated Installation (Recommended)

`agentic_ssh` can automatically detect, register, and patch configuration schemas for a wide ecosystem of agent tools:

```bash
# Auto-detect all supported agents on your system and register agentic_ssh
agentic_ssh install

# Register specifically for a particular agent framework:
agentic_ssh install --agent claude
agentic_ssh install --agent cursor
agentic_ssh install --agent antigravity

```

Supported agents for the `--agent` flag include: `claude` (Claude Code / Desktop), `cursor`, `zed`, `cline`, `roo-code`, `gemini`, `copilot`, `grok`, and `antigravity`.

### Manual Configuration

To manually register `agentic_ssh` with an MCP client (such as Claude Desktop), append it to your local configuration routing block:

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

## Master Domain Tooling Schema

### 1. `run_command`

Executes a shell command on one or multiple configured hosts. Supports both synchronous inline collection and asynchronous background detached tracking.

* **Arguments**:
* `host` (string, optional): The SSH host alias to target.
* `hosts` (array of strings, optional): Optional list of multiple host targets to query concurrently.
* `command` (string, required): The command payload to execute.
* `background` (boolean, optional, **Defaults to false**): When set to `true`, the command instantly detaches into a background Tokio thread and returns an isolated session log path immediately, freeing the agent for parallel tasks.
* `quiet` (boolean, optional, **Defaults to true**): When `true`, avoids heavy terminal output by outputting a lean progress metrics heartbeat. When `false`, real-time verbose raw standard output stream blocks are preserved in telemetry files.
* `progress_interval_secs` (integer, optional, **Defaults to 5**): Defines the polling interval sequence for the progress ticker if `quiet = true`.
* `abbreviate` (boolean, optional): Truncates massive output strings for synchronous paths to optimize token consumption.


* **Returns**:
* *Detached Background Mode (`background: true`)*:
```json
{
  "status": "started",
  "log_path": "/Users/richard/.agentic_ssh/sessions/job_20260627_1649_delight_5f58.log",
  "message": "🚀 Command started in background. To watch live progress, run:\n  tail -f /Users/richard/.agentic_ssh/sessions/job_20260627_1649_delight_5f58.log"
}

```


* *Synchronous Mode*: A JSON payload detailing structured `stdout`, `stderr`, and remote numeric `exit_code`.



### 2. `list_hosts`

Lists all explicit SSH host aliases configured in your `~/.ssh/config` file matching security criteria.

* **Arguments**: None
* **Returns**: A JSON array of secure string aliases (e.g., `["stan", "delight", "edge-router"]`).

### 3. Diagnostic & Inspection Tools

All built-in diagnostic primitives are pre-configured to handle background tracking implicitly (`quiet = true`), keeping operational latency low and protecting system context windows:

* **`get_system_stats`**: Retrieves core system stats (CPU load averages, active memory constraints, disk partitions) formatted as high-integrity JSON.
* **`list_ports`**: Discovers active listening TCP and UDP sockets with explicit structural filter criteria.
* **`search_processes`**: Evaluates active remote process layers using high-performance regex or matching string criteria.
* **`tail_log` / `tail_container_logs**`: Fetches explicit tail frames from standard system paths or target Docker engine containers.

---

## Writing Agent-Friendly Tool Descriptions

When declaring custom tools inside your configurations, writing high-quality `description` strings is critical. While humans rely on context and intuition, AI agents interpret these descriptions literally to decide when, why, and how to invoke a tool.

To write effective tool descriptions, follow these five core design pillars:
* **Action Verbs First**: Always lead with explicit actions (e.g., `"Fetches"`, `"Runs"`, `"Deploys"`).
* **Describe Mechanics, Not Just Intent**: Focus on the concrete command execution details (e.g., `"Runs cargo test"` vs. `"Checks code"`).
* **State Explicit Constraints**: Clearly document prerequisites and failure conditions (e.g., `"Requires sudo"`, `"Fails if git status is dirty"`).
* **Loud Warnings for High-Blast-Radius Actions**: Use uppercase words to signal critical risk parameters (e.g., `"DANGEROUS: Forcefully terminates process"`, `"CRITICAL: Modifies remote state"`).
* **Define Return Format Expectations**: Detail exactly what output telemetry data the agent should expect (e.g., `"Returns plain text"`, `"Returns systemd status summary"`).

---

## Advanced Configuration Overrides

Customize matching rule engines or specify layout configurations globally using an optional TOML asset located at `~/.config/agentic_ssh/config.toml`:

```toml
# Custom path to look for pooling state tables
pool_status_path = "~/.agentic_ssh_pool_status.json"

# Strict isolation security boundaries
ignore_hosts = ["*.prod.company.com", "secure-gateway"]
allow_hosts = ["stan", "delight", "*.local"]

# Inject custom parameterized statements directly into the agent toolkit
[tools.git_pull]
description = "Fetches and merges latest changes from an explicit branch configuration."
command = ["git", "-C", "/home/richard/Projects/ldk", "pull", "origin", "{{branch}}"]
allow_hosts = ["stan"]
[tools.git_pull.params.branch]
validation = "strict"

```

---

## TUI Monitoring Dashboard

Inspect connection pool tracking states, idle intervals, and active time-to-live metrics directly using the built-in terminal monitoring component:

```bash
agentic_ssh tui

```

## Acknowledgments

*Standing on the shoulders of giants...*

- **Enzo** - MCP server installation code from tokensave

### License

MIT / Apache 2.0

```

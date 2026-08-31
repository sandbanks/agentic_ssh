[![Release](https://github.com/sandbanks/agentic_ssh/actions/workflows/release.yml/badge.svg)](https://github.com/sandbanks/agentic_ssh/actions/workflows/release.yml)
[![MCP Toplist](https://mcptoplist.com/badge/io.github.sandbanks%2Fagentic_ssh.svg)](https://mcptoplist.com/server/io.github.sandbanks%2Fagentic_ssh)
[![agentic_ssh MCP server](https://glama.ai/mcp/servers/sandbanks/agentic_ssh/badges/score.svg)](https://glama.ai/mcp/servers/sandbanks/agentic_ssh)
[![Crates.io](https://img.shields.io/crates/v/agentic_ssh.svg)](https://crates.io/crates/agentic_ssh)
[![Homebrew](https://img.shields.io/badge/brew-sandbanks%2Ftap-orange?logo=homebrew)](https://github.com/sandbanks/agentic_ssh#quick-start)

# `agentic_ssh` 🛰️⚡️

> **Give your AI agents SSH superpowers across your servers—without melting your context window or blowing up your infrastructure.**

`agentic_ssh` is a fast, lightweight **Model Context Protocol (MCP) server & CLI written in Rust** that gives AI coding assistants (Claude Code, Cursor, Gemini, Antigravity, Copilot, Zed, Cline) safe, token-efficient, and asynchronous SSH access to your homelab, servers, or cloud clusters.

![AI Agent Multi-Host Interrogation & Security Guardrail](https://assets.sandbanks.tech/agentic_ssh/agentic_ssh_hero.gif)

### 📊 Token & Quota Impact: Raw SSH vs. `agentic_ssh`

| Metric / Scenario | Raw SSH in Agent Prompt | With `agentic_ssh` MCP | The Difference |
| :--- | :--- | :--- | :--- |
| **Context Window Consumption** | 15,000+ tokens (raw stdout dump) | **185 tokens** (structured JSON telemetry) | **98.8% token savings** 📉 |
| **API Cost per Query** | ~$0.05 – $0.15 | **~$0.0004** | Fraction of a cent 💰 |
| **Security & Blast Radius** | Full access to every host in `~/.ssh/config` | **Strict Whitelist Guardrail** (`allow_hosts`) | Unauthorized hosts blocked 🛡️ |
| **Connection Latency** | 800ms – 1.5s reconnect handshake | **0ms** (pooled Keepalive socket) | Instant response ⚡️ |
| **Long-Running Builds** | Blocks agent reasoning / silent dropout | **Detached async** (`background: true`) | Parallel workflows 🚀 |

---

## 🛑 Why AI Agents + Raw SSH Is a Bad Idea

If you've ever watched an AI agent try to use standard terminal SSH, you've probably seen:

* 💥 **The Context Avalanche**: The agent runs `apt upgrade` or `cargo build`, and **15,000 lines of compiler noise** dump straight into your context window—wiping out memory, hitting limits, and costing money.
* 👻 **Silent Death by Dropout**: Cloud NATs and Tailscale love to silently drop idle SSH sockets during a 10-minute compile. The agent hangs forever waiting for output that will never arrive.
* 😵‍💫 **Nested Escaping Hell**: Asking an LLM to quote bash inside an SSH string inside an MCP JSON payload (`ssh host "bash -c \"echo 'hello'\""`) invariably leads to broken quotes and syntax errors.
* 💣 **The Rogue Command**: A hallucinating agent with unrestricted SSH access can accidentally reboot or delete the wrong machine.

---

## 🪄 How `agentic_ssh` Fixes This

| The Problem | `agentic_ssh` Solution |
| :--- | :--- |
| **Bloated Context Windows** | Automatically summarizes verbose outputs and redirects large streams to isolated local session logs (`~/.agentic_ssh/sessions/`). |
| **Dropped Sockets & Lag** | Rust-native connection pooling (`russh`) with automatic 30s keepalives and zero-latency session reuse. |
| **Broken Quoting** | Arguments are structured and escaped natively behind the scenes—zero escaping headaches for the model. |
| **Blocking Long Tasks** | Supports `background: true`—fires long builds or migrations into detached threads so the agent can keep working. |
| **Rogue Actions** | Strict whitelist boundaries (`allow_hosts`, `ignore_hosts`) and cryptographically signed local configs. |

---

## ⚡️ Quick Start (30 Seconds)

### 1. Install

```bash
# macOS & Linux (Homebrew)
brew install sandbanks/tap/agentic_ssh

# Or with cargo-binstall (pre-compiled binary)
cargo binstall agentic_ssh

# Or run instantly with Nix (zero compile)
nix run github:sandbanks/agentic_ssh -- doctor
```

### 2. Auto-Register with Your AI Agents

One command detects your installed AI tools and registers the MCP server automatically:

```bash
agentic_ssh install
```

*(Supports **Claude Code / Desktop**, **Cursor**, **Gemini**, **Antigravity**, **Copilot**, **Zed**, **Cline**, and **Roo-Code**).*

### 3. Verify Health

```bash
agentic_ssh doctor
```

---

## 🤖 Prompt Recipes: What Your AI Agent Can Do

Once installed, just talk to your agent naturally. Here are real-world prompts you can copy & paste:

### 🔍 1. Cluster Health & Resource Audit
> *"Check the CPU load, RAM usage, and available disk space across stan, cartman, and aruba. Report any bottlenecks."*
> 
> ⚡️ *Agent calls `get_system_stats` concurrently across all 3 nodes and gives you a structured comparison table in 2 seconds.*

### 🛡️ 2. Security & Port Exposure Check
> *"Inspect all active listening TCP and UDP ports on our staging server. Flag anything open on 0.0.0.0 that shouldn't be."*
> 
> ⚡️ *Agent calls `list_ports` with process attribution (PID + binary name) for instant auditing.*

### 🐳 3. Container Status & Error Log Tailing
> *"Check if any Docker containers crashed on cartman, and tail the last 50 lines of the auth-service logs."*
> 
> ⚡️ *Agent calls `check_docker_status` and `tail_container_logs` without flooding your context window.*

### ⏳ 4. Detached Background Jobs
> *"Deploy the latest git commit on stan in the background and notify me when it finishes."*
> 
> ⚡️ *Agent runs `run_command` with `background: true`, frees up your chat immediately, and tracks the output in a local session log.*

---

## 🖥️ CLI Superpowers for Humans (Built-in Terminal Tools)

`agentic_ssh` isn't just an MCP server for AI—it includes powerful CLI tools for human developers too:

> 💡 **Tip:** Add `alias ash="agentic_ssh"` to your `~/.zshrc` or `~/.bashrc` to save keystrokes!

### `agentic_ssh watch`: Multi-Host Live Streaming TUI
Watch commands run concurrently across multiple servers with live streaming panes and post-run log inspection:

![Multi-Host Command Watcher TUI](https://assets.sandbanks.tech/agentic_ssh/agentic_ssh_watch.gif)

```bash
# Watch a command on multiple hosts concurrently
agentic_ssh watch stan,cartman,aruba "pnpm --version"

# Watch an entire host group defined in your config
agentic_ssh watch web-fleet "docker compose ps"
```

### `agentic_ssh json`: Instant Multi-Host Scripting
Call any built-in MCP diagnostic tool directly from your terminal and get clean, parseable JSON:

```bash
# Get structured system stats across hosts
agentic_ssh json get_system_stats stan,cartman

# Query listening ports
agentic_ssh json list_ports '{"hosts": ["stan", "aruba"]}'
```

### `agentic_ssh tui`: Live Connection Pool Dashboard
Inspect active SSH sockets, heartbeat metrics, and connection lifetimes:

```bash
agentic_ssh tui
```

---

## 🧰 Built-in Tool Catalog

| MCP Tool | Description |
| :--- | :--- |
| `list_hosts` | Discovers and returns all authorized SSH host aliases from `~/.ssh/config`. |
| `list_groups` | Returns defined multi-host server groups (e.g., `web-fleet`, `db-cluster`). |
| `run_command` | Executes commands concurrently across hosts (supports sync or detached async background mode). |
| `get_system_stats` | Fetches CPU load, RAM utilization, and disk partition stats. |
| `list_ports` | Scans active listening TCP/UDP sockets with process & PID attribution. |
| `search_processes` | Evaluates and filters running processes with regex matching. |
| `tail_log` | Safely tails standard system log files without loading multi-gigabyte files. |
| `tail_container_logs` | Fetches real-time log frames from Docker containers. |
| `check_docker_status` | Returns Docker daemon health, running containers, and image counts. |
| `check_service_status` | Queries `systemd` / `systemctl` service states. |
| `list_upgradable` | Lists pending OS package updates across remote machines. |
| `git_pull` | Safely fetches and updates a remote Git repository. |
| `find_large_files` | Scans for disk-hogging files exceeding a size threshold. |
| `grep_syslog` | Searches remote syslog / journalctl streams for specific error patterns. |
| `list_cron_jobs` | Inspects system and user crontabs. |
| `list_network_connections` | Lists active network connections and remote endpoints. |

---

## ⚙️ Configuration (`~/.config/agentic_ssh/config.toml`)

You can define host groups and security boundaries in an optional configuration file:

```toml
# Security boundaries: Only allow safe hosts
allow_hosts = ["stan", "cartman", "kyle", "*.local"]
ignore_hosts = ["prod-db-primary", "secure-vault"]

# Multi-host groups for easy targeting
[groups]
fleet = ["stan", "cartman", "aruba", "kyle"]
pis = ["stan", "cartman", "kyle"]
web = ["aruba", "stan"]

# Custom parameterized agent tools
[tools.deploy_stack]
description = "Pulls latest compose repo and updates stack containers."
command = ["docker", "compose", "-f", "/opt/app/docker-compose.yml", "up", "-d"]
allow_hosts = ["stan", "cartman"]
```

---

## 🤝 Acknowledgments

* Standing on the shoulders of giants:
  * **Russh**: High-performance pure Rust SSH client.
  * **Ratatui**: Gorgeous terminal user interfaces.
  * **Enzo**: MCP installer inspiration from `tokensave`.

---

## 📄 License

Dual-licensed under MIT and Apache 2.0.

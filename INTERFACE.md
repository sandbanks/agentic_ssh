# Interface Contract: agentic_ssh (AI Agent SSH Primitive)

> **Repository:** `https://github.com/sandbanks/agentic_ssh`  
> **Binary Names:** `agentic_ssh` (canonical), `ash` (recommended CLI alias)  
> **Protocols:** Model Context Protocol (MCP stdio), OpenSSH client  
> **Platforms:** macOS (Apple Silicon & Intel), Linux (x86_64, aarch64)  

---

## 1. Scope & Ownership

* **Owns:**
  * Fast, persistent SSH connection pooling and multi-host multiplexing for AI agents and humans.
  * Standardized MCP server implementation over `stdio` exposing remote server observation and diagnostic primitives.
  * Robust detached execution: process-group tree termination (PGID `kill`), output abbreviation, and log-file backgrounding.
  * Secure-by-default execution policy: mandatory host authorization (`allow_hosts`) and cryptographic project configuration signing (`ash trust`).
  * Automatic MCP installer/uninstaller across 17+ AI agent harnesses (Claude Code, Cursor, Cline, OpenCode, Codex, Gemini, etc.).
* **Does NOT Own:**
  * Remote infrastructure provisioning or orchestration engines (Ansible, Terraform, NOC).
  * System-level OpenSSH server configuration (`/etc/ssh/sshd_config`).
  * Container runtime scheduling (Docker Compose, Kubernetes, Nomad).

---

## 2. Exported Binaries & CLI Surface

### 2.1 CLI Subcommands (`agentic_ssh` / `ash`)

| Subcommand | Arguments / Flags | Description |
|---|---|---|
| `serve` | *(default when run without args)* | Starts the MCP server on `stdio` for agent harnesses. |
| `watch` | `<target> "<command>"` | Streams real-time execution across single host, comma-separated hosts, or host groups. |
| `tools` | `[--json]` | Lists all registered built-in and dynamically configured MCP tools. |
| `json` | `<tool> [arguments]` | Directly invokes any MCP tool from the terminal and formats stdout as JSON. |
| `tui` | None | Launches an interactive Ratatui dashboard displaying live pool metrics, active SSH connections, and background sessions. |
| `doctor` | `[--agent <name>]` | Runs comprehensive environment diagnostics: OpenSSH keys, known hosts, agent harnesses, and config validity. |
| `clean` | `[-a\|--all]` `[-n\|--numdays <days>]` | Prunes historical session logs from `~/.agentic_ssh/sessions`. |
| `trust` | `[path]` | Cryptographically hashes and signs a project-local `.agentic_ssh.toml` config into the user's secure signature store. |
| `install` | `[--agent <name>] [--local]` | Inspects installed AI coding tools and registers `agentic_ssh` in their MCP settings. |
| `uninstall`| `[--agent <name>] [--local]` | Removes `agentic_ssh` from target agent configurations. |

### 2.2 Global Flags
* `-c, --config <path>` — Specify custom configuration file path.
* `--no-global` — Ignore global user config in `~/.config/agentic_ssh/config.toml`.
* `--no-update-check` — Disable background crates.io version check.
* `-v, --version` — Display current release version and repository update status.

---

## 3. Model Context Protocol (MCP) Surface

When running via `agentic_ssh serve`, the following standard MCP tools are exposed:

### Core Execution Primitives
* **`run_command`**: Execute arbitrary shell command on single (`host`) or multiple concurrent (`hosts`) targets. Supports `background: true` (returns immediately with log path) and output abbreviation.
* **`get_system_stats`**: Structured CPU load, memory utilization, swap, disk partitions, and uptime.
* **`list_ports`**: Discovers open listening ports, protocols, and bound process names.
* **`check_resource_pressure`**: Evaluates Linux PSI (Pressure Stall Information) across CPU, memory, and I/O.
* **`search_processes`**: Filters running processes by name, user, PID, or resource consumption.

### Observability & Streaming
* **`tail_log`**: Streams or extracts the last $N$ lines of any log file.
* **`tail_container_logs`**: Streams stdout/stderr from Docker/Podman container runtimes.
* **`wait_for_log_pattern`**: Blocks until a regex pattern appears in a log or times out.

### Dynamic User Tools (`config.toml`)
Users can expose custom fleet tools by defining `[[tools]]` in configuration (e.g., `check_upgrade_status`, `safe_apt_upgrade`, `docker_prune`).

---

## 4. Configuration Schema & Security Policy

### 4.1 Configuration File Precedence
1. Explicit CLI argument (`--config <path>`).
2. Project-local `.agentic_ssh.toml` *(Requires prior human cryptographic signature via `ash trust` before agents can load it)*.
3. User global: `~/.config/agentic_ssh/config.toml` (or platform equivalent).

### 4.2 Security Guardrails
* **Host Authorization (`allow_hosts`):** If defined, `agentic_ssh` strictly forbids execution against any host or IP not explicitly whitelisted.
* **Project Trust (`.agentic_ssh.sig`):** Prevents malicious repositories from running arbitrary commands on cloned machines by requiring human verification.
* **Connection Lifecycle:** Connections pooled via `russh` maintain persistent SSH multiplexing; dropped processes cleanly terminate remote process trees via process group signaling.

---

## 5. Build, Package & CI/CD Pipeline

* **Repository:** `sandbanks/agentic_ssh`
* **Rust Toolchain:** MSRV 1.96+, Edition 2024.
* **Nix Flake:**
  * Package: `.#agentic_ssh` (default package).
  * DevShell: `nix develop` provides rustc, cargo, rust-analyzer, clippy, and rustfmt.
  * Binary Substituter: Cached via `sandbanks.cachix.org`.
* **CI Matrix (`.github/workflows/ci.yml`):**
  * `cargo test --workspace` on Ubuntu and macOS.
  * `cargo clippy` and `cargo fmt`.
  * `nix build .#` on Linux (x86_64, aarch64) and macOS Apple Silicon.

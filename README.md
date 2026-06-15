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

---

## MCP Configuration

To register `agentic_ssh` with an MCP client (such as Claude Desktop), add it to your configuration file:

### Claude Desktop Configuration
On macOS, edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "agentic_ssh": {
      "command": "/Users/richard/projects/rust/agentic_ssh/target/release/agentic_ssh",
      "args": []
    }
  }
}
```
*(Make sure to update the absolute path to point to your compiled binary location).*

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


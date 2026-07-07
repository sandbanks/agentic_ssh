// Rust guideline compliant 2025-10-17
//! `OpenCode` agent integration.
//!
//! Handles registration of the agentic_ssh MCP server in `OpenCode`'s config
//! file (`$HOME/.config/opencode/opencode.json` or `$XDG_CONFIG_HOME/opencode/opencode.json`),
//! and prompt rules via `$HOME/.config/opencode/AGENTS.md`. `OpenCode` has no hook system or
//! declarative tool permissions — it uses interactive runtime approval.

use std::io::Write;
use std::path::Path;

use serde_json::json;

use crate::errors::{AgenticSshError, Result};

use super::{
    AgentIntegration, DoctorCounters, HealthcheckContext, InstallContext, InstallScope,
    backup_and_write_json, backup_config_file, load_json_file, load_json_file_strict,
    safe_write_json_file,
};

/// `OpenCode` agent.
pub struct OpenCodeIntegration;

impl AgentIntegration for OpenCodeIntegration {
    fn name(&self) -> &'static str {
        "OpenCode"
    }

    fn id(&self) -> &'static str {
        "opencode"
    }

    fn install(&self, ctx: &InstallContext) -> Result<()> {
        let config_path = opencode_config_path_for(ctx);
        install_mcp_server(&config_path, &ctx.agentic_ssh_bin)?;

        let prompt = opencode_prompt_path_for(ctx);
        install_prompt_rules(&prompt)?;

        eprintln!();
        eprintln!("Setup complete. Next steps:");
        eprintln!("  1. Start a new OpenCode session — agentic_ssh tools are now available");
        eprintln!("  3. OpenCode will prompt for approval on first use of each tool");
        Ok(())
    }

    fn uninstall(&self, ctx: &InstallContext) -> Result<()> {
        let config_path = opencode_config_path_for(ctx);
        uninstall_mcp_server(&config_path);

        let prompt = opencode_prompt_path_for(ctx);
        uninstall_prompt_rules(&prompt);

        eprintln!();
        eprintln!("Uninstall complete. AgenticSsh has been removed from OpenCode.");
        eprintln!("Start a new OpenCode session for changes to take effect.");
        Ok(())
    }

    fn healthcheck(&self, dc: &mut DoctorCounters, ctx: &HealthcheckContext) {
        eprintln!("\n\x1b[1mOpenCode integration\x1b[0m");
        doctor_check_config(dc, &ctx.home);
        doctor_check_prompt(dc, &ctx.home);
    }

    fn is_detected(&self, home: &Path) -> bool {
        home.join(".config").join("opencode").is_dir()
    }

    fn has_agentic_ssh(&self, home: &Path) -> bool {
        let config_path = opencode_config_path(home);
        if !config_path.exists() {
            return false;
        }
        let json = load_json_file(&config_path);
        if let Some(agentic_ssh) = json.get("mcp").and_then(|v| v.get("agentic_ssh")) {
            if let Some(current_bin) = super::which_agentic_ssh() {
                let cmd = agentic_ssh
                    .get("command")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                cmd == current_bin
            } else {
                true
            }
        } else {
            false
        }
    }

    fn supports_local(&self) -> bool {
        true
    }

    fn primary_config_path(&self, home: &Path) -> Option<std::path::PathBuf> {
        Some(opencode_config_path(home))
    }
}

// ---------------------------------------------------------------------------
// Config path resolution
// ---------------------------------------------------------------------------

/// Returns the path to opencode config (global).
/// Prefers `$HOME/.config/opencode/opencode.json`. Falls back to
/// `$XDG_CONFIG_HOME/opencode/opencode.json` only when the XDG path
/// is under `home` (so tests with temp-dir homes are never polluted by
/// the real user's environment).
/// opencode.json path for this install: global config path, or
/// `<project>/opencode.json` for `--local`.
fn opencode_config_path_for(ctx: &InstallContext) -> std::path::PathBuf {
    match &ctx.scope {
        InstallScope::Global => opencode_config_path(&ctx.home),
        InstallScope::Local { project_path } => project_path.join("opencode.json"),
    }
}

/// AGENTS.md path for this install: global prompt path, or
/// `<project>/AGENTS.md` for `--local`.
fn opencode_prompt_path_for(ctx: &InstallContext) -> std::path::PathBuf {
    match &ctx.scope {
        InstallScope::Global => opencode_prompt_path(&ctx.home),
        InstallScope::Local { project_path } => project_path.join("AGENTS.md"),
    }
}

fn opencode_config_path(home: &Path) -> std::path::PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg_path = std::path::PathBuf::from(&xdg);
        if xdg_path.starts_with(home) {
            return xdg_path.join("opencode/opencode.json");
        }
    }
    home.join(".config/opencode/opencode.json")
}

/// Returns the path to the global AGENTS.md prompt file.
fn opencode_prompt_path(home: &Path) -> std::path::PathBuf {
    let modern = home.join(".config/opencode/AGENTS.md");
    if modern.exists() || home.join(".config/opencode").exists() {
        return modern;
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg_path = std::path::PathBuf::from(&xdg);
        if xdg_path.starts_with(home) {
            let xdg_dir = xdg_path.join("opencode");
            if xdg_dir.exists() {
                return xdg_dir.join("AGENTS.md");
            }
        }
    }
    home.join("AGENTS.md")
}

// ---------------------------------------------------------------------------
// Install helpers
// ---------------------------------------------------------------------------

/// Register MCP server in opencode.json.
///
/// Safety: creates a `.bak` backup before writing and restores it on any
/// error. Uses strict JSON parsing so an existing file with invalid syntax
/// is never silently replaced with an empty object.
fn install_mcp_server(config_path: &Path, agentic_ssh_bin: &str) -> Result<()> {
    let backup = backup_config_file(config_path)?;
    let mut config = match load_json_file_strict(config_path) {
        Ok(v) => v,
        Err(e) => {
            if let Some(ref b) = backup {
                eprintln!("  Backup preserved at: {}", b.display());
            }
            return Err(e);
        }
    };

    config["mcp"]["agentic_ssh"] = json!({
        "type": "local",
        "command": [agentic_ssh_bin, "serve"]
    });

    safe_write_json_file(config_path, &config, backup.as_deref())?;
    eprintln!(
        "\x1b[32m✔\x1b[0m Added agentic_ssh MCP server to {}",
        config_path.display()
    );
    Ok(())
}

/// Append prompt rules to AGENTS.md (idempotent).
fn install_prompt_rules(prompt_path: &Path) -> Result<()> {
    let marker = "## Prefer agentic_ssh MCP tools";
    let existing = if prompt_path.exists() {
        std::fs::read_to_string(prompt_path).unwrap_or_default()
    } else {
        String::new()
    };
    if existing.contains(marker) {
        if existing.contains("agentic_ssh_context") || existing.contains("knowledge graph") {
            uninstall_prompt_rules(prompt_path);
        } else {
            eprintln!("  AGENTS.md already contains agentic_ssh rules, skipping");
            return Ok(());
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(prompt_path)
        .map_err(|e| AgenticSshError::Config {
            message: format!("failed to open AGENTS.md: {e}"),
        })?;
    write!(
        f,
        "\n{marker}\n\n\
        When you need to discover, query, monitor, or execute commands on remote SSH hosts, \
        ALWAYS use the `agentic_ssh` MCP tools:\n\
        - **Discovering Hosts:** Use the `list_hosts` tool to retrieve the list of configured remote SSH hosts. Do NOT read or parse `~/.ssh/config` manually.\n\
        - **Executing Commands:** Use the `run_command` tool to run shell commands on one or more hosts concurrently.\n\
        - **Monitoring Logs:** Use `tail_log` (for files) or `tail_container_logs` (for Docker containers) to read recent logs. To verify startup, services, or events across cluster nodes without polling, use `wait_for_log_pattern` to block and stream logs until a regex pattern is matched.\n\
        - **Checking System & Network Status:** Use `get_system_stats` to fetch structured CPU, memory, and disk usage metrics. Use `list_ports` to see active listening TCP/UDP ports. Use `search_processes` to find running processes.\n\
        - **Custom Tools:** Use custom commands registered dynamically through the configuration file (e.g., `find_large_files`, `check_service_status`, `check_docker_status`).\n\n\
        These tools leverage an automatic connection pool (reusing active sessions and closing them after 5 minutes of inactivity), handle SSH key-based authentication seamlessly, and support output abbreviation to prevent token bloat.\n"
    )
    .ok();
    eprintln!(
        "\x1b[32m✔\x1b[0m Appended agentic_ssh rules to {}",
        prompt_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Uninstall helpers
// ---------------------------------------------------------------------------

/// Remove MCP server from opencode.json.
fn uninstall_mcp_server(config_path: &Path) {
    if !config_path.exists() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(config_path) else {
        return;
    };
    let Ok(mut config) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return;
    };
    let Some(mcp) = config.get_mut("mcp").and_then(|v| v.as_object_mut()) else {
        return;
    };
    if mcp.remove("agentic_ssh").is_none() {
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            config_path.display()
        );
        return;
    }
    if mcp.is_empty() {
        config.as_object_mut().map(|o| o.remove("mcp"));
    }
    let is_empty = config.as_object().is_some_and(serde_json::Map::is_empty);
    if is_empty {
        std::fs::remove_file(config_path).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            config_path.display()
        );
    } else if backup_and_write_json(config_path, &config) {
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh MCP server from {}",
            config_path.display()
        );
    }
}

/// Remove agentic_ssh rules from AGENTS.md.
fn uninstall_prompt_rules(prompt_path: &Path) {
    if !prompt_path.exists() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(prompt_path) else {
        return;
    };
    if !contents.contains("agentic_ssh") {
        eprintln!("  AGENTS.md does not contain agentic_ssh rules, skipping");
        return;
    }
    let marker = "## Prefer agentic_ssh MCP tools";
    let Some(start) = contents.find(marker) else {
        return;
    };
    let after_marker = start + marker.len();
    let end = contents[after_marker..]
        .find("\n## ")
        .map_or(contents.len(), |pos| after_marker + pos);
    let mut new_contents = String::new();
    new_contents.push_str(contents[..start].trim_end());
    let remainder = &contents[end..];
    if !remainder.is_empty() {
        new_contents.push_str("\n\n");
        new_contents.push_str(remainder.trim_start());
    }
    let new_contents = new_contents.trim().to_string();
    if new_contents.is_empty() {
        std::fs::remove_file(prompt_path).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            prompt_path.display()
        );
    } else {
        std::fs::write(prompt_path, format!("{new_contents}\n")).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh rules from {}",
            prompt_path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Healthcheck helpers
// ---------------------------------------------------------------------------

/// Check opencode.json has agentic_ssh registered.
fn doctor_check_config(dc: &mut DoctorCounters, home: &Path) {
    let config_path = opencode_config_path(home);
    if !config_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent opencode` if you use OpenCode",
            config_path.display()
        ));
        return;
    }

    let config = load_json_file(&config_path);
    let mcp_entry = &config["mcp"]["agentic_ssh"];
    if !mcp_entry.is_object() {
        dc.fail(&format!(
            "MCP server NOT registered in {} — run `agentic_ssh install --agent opencode`",
            config_path.display()
        ));
        return;
    }
    dc.pass(&format!(
        "MCP server registered in {}",
        config_path.display()
    ));

    let command = mcp_entry["command"].as_array();
    let has_serve = command.is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("serve")));
    if has_serve {
        dc.pass("MCP server args include \"serve\"");
    } else {
        dc.fail("MCP server args missing \"serve\" — run `agentic_ssh install --agent opencode`");
    }
}

/// Check AGENTS.md contains agentic_ssh rules.
fn doctor_check_prompt(dc: &mut DoctorCounters, home: &Path) {
    let prompt_path = opencode_prompt_path(home);
    if prompt_path.exists() {
        let has_rules = std::fs::read_to_string(&prompt_path)
            .unwrap_or_default()
            .contains("agentic_ssh");
        if has_rules {
            dc.pass("AGENTS.md contains agentic_ssh rules");
        } else {
            dc.fail(
                "AGENTS.md missing agentic_ssh rules — run `agentic_ssh install --agent opencode`",
            );
        }
    } else {
        dc.warn("AGENTS.md does not exist");
    }
}

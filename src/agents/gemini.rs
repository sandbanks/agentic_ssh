//! Gemini CLI agent integration.
//!
//! Handles registration of the agentic_ssh MCP server in Gemini CLI's config
//! file (`~/.gemini/settings.json`), and prompt rules via `~/.gemini/GEMINI.md`.
//! Gemini CLI has no hook system. Tool auto-approval is handled via the
//! `trust: true` flag on the MCP server entry.

use std::io::Write;
use std::path::Path;

use serde_json::json;

use crate::errors::{AgenticSshError, Result};

use super::{
    AgentIntegration, DoctorCounters, HealthcheckContext, InstallContext, backup_and_write_json,
    backup_config_file, load_json_file, load_json_file_strict, safe_write_json_file,
};

/// Gemini CLI agent.
pub struct GeminiIntegration;

impl AgentIntegration for GeminiIntegration {
    fn name(&self) -> &'static str {
        "Gemini CLI"
    }

    fn id(&self) -> &'static str {
        "gemini"
    }

    fn install(&self, ctx: &InstallContext) -> Result<()> {
        let gemini_dir = ctx.base_dir().join(".gemini");
        std::fs::create_dir_all(&gemini_dir).ok();
        let settings_path = gemini_dir.join("settings.json");

        install_mcp_server(&settings_path, &ctx.agentic_ssh_bin)?;

        let gemini_md = gemini_dir.join("GEMINI.md");
        install_prompt_rules(&gemini_md)?;

        eprintln!();
        eprintln!("Setup complete. Next steps:");
        eprintln!("  1. Start a new Gemini CLI session — agentic_ssh tools are now available");
        Ok(())
    }

    fn uninstall(&self, ctx: &InstallContext) -> Result<()> {
        let gemini_dir = ctx.base_dir().join(".gemini");
        let settings_path = gemini_dir.join("settings.json");

        uninstall_mcp_server(&settings_path);

        let gemini_md = gemini_dir.join("GEMINI.md");
        uninstall_prompt_rules(&gemini_md);

        eprintln!();
        eprintln!("Uninstall complete. AgenticSsh has been removed from Gemini CLI.");
        eprintln!("Start a new Gemini CLI session for changes to take effect.");
        Ok(())
    }

    fn healthcheck(&self, dc: &mut DoctorCounters, ctx: &HealthcheckContext) {
        eprintln!("\n\x1b[1mGemini CLI integration\x1b[0m");
        doctor_check_settings(dc, &ctx.home);
        doctor_check_prompt(dc, &ctx.home);
    }

    fn is_detected(&self, home: &Path) -> bool {
        home.join(".gemini").is_dir()
    }

    fn has_agentic_ssh(&self, home: &Path) -> bool {
        let settings = home.join(".gemini").join("settings.json");
        if !settings.exists() {
            return false;
        }
        let json = load_json_file(&settings);
        if let Some(agentic_ssh) = json.get("mcpServers").and_then(|v| v.get("agentic_ssh")) {
            if let Some(current_bin) = super::which_agentic_ssh() {
                let cmd = agentic_ssh
                    .get("command")
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
        Some(home.join(".gemini/settings.json"))
    }
}

// ---------------------------------------------------------------------------
// Install helpers
// ---------------------------------------------------------------------------

/// Register MCP server in ~/.gemini/settings.json.
fn install_mcp_server(settings_path: &Path, agentic_ssh_bin: &str) -> Result<()> {
    let backup = backup_config_file(settings_path)?;
    let mut settings = match load_json_file_strict(settings_path) {
        Ok(v) => v,
        Err(e) => {
            if let Some(ref b) = backup {
                eprintln!("  Backup preserved at: {}", b.display());
            }
            return Err(e);
        }
    };

    settings["mcpServers"]["agentic_ssh"] = json!({
        "command": agentic_ssh_bin,
        "args": ["serve"],
        "trust": true
    });

    safe_write_json_file(settings_path, &settings, backup.as_deref())?;
    eprintln!(
        "\x1b[32m✔\x1b[0m Added agentic_ssh MCP server to {}",
        settings_path.display()
    );
    Ok(())
}

/// Append prompt rules to GEMINI.md (idempotent).
fn install_prompt_rules(gemini_md: &Path) -> Result<()> {
    let marker = "## Prefer agentic_ssh MCP tools";
    let existing = if gemini_md.exists() {
        std::fs::read_to_string(gemini_md).unwrap_or_default()
    } else {
        String::new()
    };
    if existing.contains(marker) {
        if existing.contains("agentic_ssh_context") || existing.contains("knowledge graph") {
            uninstall_prompt_rules(gemini_md);
        } else {
            eprintln!("  GEMINI.md already contains agentic_ssh rules, skipping");
            return Ok(());
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(gemini_md)
        .map_err(|e| AgenticSshError::Config {
            message: format!("failed to open GEMINI.md: {e}"),
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
        gemini_md.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Uninstall helpers
// ---------------------------------------------------------------------------

/// Remove MCP server from ~/.gemini/settings.json.
fn uninstall_mcp_server(settings_path: &Path) {
    if !settings_path.exists() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(settings_path) else {
        return;
    };
    let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return;
    };
    let Some(servers) = settings
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
    else {
        return;
    };
    if servers.remove("agentic_ssh").is_none() {
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            settings_path.display()
        );
        return;
    }
    if servers.is_empty() {
        settings.as_object_mut().map(|o| o.remove("mcpServers"));
    }
    let is_empty = settings.as_object().is_some_and(serde_json::Map::is_empty);
    if is_empty {
        std::fs::remove_file(settings_path).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            settings_path.display()
        );
    } else if backup_and_write_json(settings_path, &settings) {
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh MCP server from {}",
            settings_path.display()
        );
    }
}

/// Remove agentic_ssh rules from GEMINI.md.
fn uninstall_prompt_rules(gemini_md: &Path) {
    if !gemini_md.exists() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(gemini_md) else {
        return;
    };
    if !contents.contains("agentic_ssh") {
        eprintln!("  GEMINI.md does not contain agentic_ssh rules, skipping");
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
        std::fs::remove_file(gemini_md).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            gemini_md.display()
        );
    } else {
        std::fs::write(gemini_md, format!("{new_contents}\n")).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh rules from {}",
            gemini_md.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Healthcheck helpers
// ---------------------------------------------------------------------------

/// Check settings.json has agentic_ssh registered.
fn doctor_check_settings(dc: &mut DoctorCounters, home: &Path) {
    let settings_path = home.join(".gemini").join("settings.json");
    if !settings_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent gemini` if you use Gemini CLI",
            settings_path.display()
        ));
        return;
    }

    let settings = load_json_file(&settings_path);
    let server = settings
        .get("mcpServers")
        .and_then(|v| v.get("agentic_ssh"));

    let Some(server) = server.and_then(|v| v.as_object()) else {
        dc.fail(&format!(
            "MCP server NOT registered in {} — run `agentic_ssh install --agent gemini`",
            settings_path.display()
        ));
        return;
    };
    dc.pass(&format!(
        "MCP server registered in {}",
        settings_path.display()
    ));

    // Check command includes "serve"
    let has_serve = server
        .get("args")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some("serve")));
    if has_serve {
        dc.pass("MCP server args include \"serve\"");
    } else {
        dc.fail("MCP server args missing \"serve\" — run `agentic_ssh install --agent gemini`");
    }

    // Check trust flag
    let is_trusted = server
        .get("trust")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if is_trusted {
        dc.pass("MCP server has trust: true (tools auto-approved)");
    } else {
        dc.warn("MCP server missing trust: true — Gemini will prompt for each tool call");
    }
}

/// Check GEMINI.md contains agentic_ssh rules.
fn doctor_check_prompt(dc: &mut DoctorCounters, home: &Path) {
    let gemini_md = home.join(".gemini").join("GEMINI.md");
    if gemini_md.exists() {
        let has_rules = std::fs::read_to_string(&gemini_md)
            .unwrap_or_default()
            .contains("agentic_ssh");
        if has_rules {
            dc.pass("GEMINI.md contains agentic_ssh rules");
        } else {
            dc.fail(
                "GEMINI.md missing agentic_ssh rules — run `agentic_ssh install --agent gemini`",
            );
        }
    } else {
        dc.warn("~/.gemini/GEMINI.md does not exist");
    }
}

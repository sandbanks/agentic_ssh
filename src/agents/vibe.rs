//! Mistral Vibe agent integration.
//!
//! Handles registration of the agentic_ssh MCP server in Vibe's
//! `~/.vibe/config.toml` as a `[[mcp_servers]]` entry with stdio transport,
//! and prompt rules via `~/.vibe/prompts/cli.md`.

use std::io::Write;
use std::path::Path;

use crate::errors::{AgenticSshError, Result};

use super::{AgentIntegration, DoctorCounters, HealthcheckContext, InstallContext};

/// Mistral Vibe agent.
pub struct VibeIntegration;

/// Returns the Vibe home directory.
/// Respects `VIBE_HOME` only when it falls under `home` (so tests with
/// temp-dir homes are not polluted by the real user's environment).
fn vibe_home(home: &Path) -> std::path::PathBuf {
    if let Ok(vibe) = std::env::var("VIBE_HOME") {
        let vibe_path = std::path::PathBuf::from(&vibe);
        if vibe_path.starts_with(home) {
            return vibe_path;
        }
    }
    home.join(".vibe")
}

fn vibe_config_path(home: &Path) -> std::path::PathBuf {
    vibe_home(home).join("config.toml")
}

fn vibe_prompt_path(home: &Path) -> std::path::PathBuf {
    vibe_home(home).join("prompts/cli.md")
}

/// The TOML marker that identifies a agentic_ssh MCP server entry.
const TOML_MARKER: &str = "name = \"agentic_ssh\"";

impl AgentIntegration for VibeIntegration {
    fn name(&self) -> &'static str {
        "Mistral Vibe"
    }

    fn id(&self) -> &'static str {
        "vibe"
    }

    fn install(&self, ctx: &InstallContext) -> Result<()> {
        let vibe_dir = vibe_home(&ctx.home);
        std::fs::create_dir_all(&vibe_dir).ok();

        let config_path = vibe_config_path(&ctx.home);
        install_mcp_server(&config_path, &ctx.agentic_ssh_bin)?;

        let prompt_dir = vibe_dir.join("prompts");
        std::fs::create_dir_all(&prompt_dir).ok();
        let prompt_path = vibe_prompt_path(&ctx.home);
        install_prompt_rules(&prompt_path)?;

        eprintln!();
        eprintln!("Setup complete. Next steps:");
        eprintln!("  1. Start a new Vibe session — agentic_ssh tools are now available");
        Ok(())
    }

    fn uninstall(&self, ctx: &InstallContext) -> Result<()> {
        let config_path = vibe_config_path(&ctx.home);
        uninstall_mcp_server(&config_path);
        uninstall_prompt_rules(&vibe_prompt_path(&ctx.home));

        eprintln!();
        eprintln!("Uninstall complete. AgenticSsh has been removed from Mistral Vibe.");
        eprintln!("Start a new Vibe session for changes to take effect.");
        Ok(())
    }

    fn healthcheck(&self, dc: &mut DoctorCounters, ctx: &HealthcheckContext) {
        eprintln!("\n\x1b[1mMistral Vibe integration\x1b[0m");
        doctor_check_config(dc, &ctx.home);
        doctor_check_prompt(dc, &ctx.home);
    }

    fn is_detected(&self, home: &Path) -> bool {
        vibe_home(home).is_dir()
    }

    fn has_agentic_ssh(&self, home: &Path) -> bool {
        let config_path = vibe_config_path(home);
        if !config_path.exists() {
            return false;
        }
        let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
        contents.contains(TOML_MARKER)
    }
}

// ---------------------------------------------------------------------------
// Install helpers
// ---------------------------------------------------------------------------

/// Append a `[[mcp_servers]]` entry for agentic_ssh to `config.toml` (idempotent).
fn install_mcp_server(config_path: &Path, agentic_ssh_bin: &str) -> Result<()> {
    let existing = if config_path.exists() {
        std::fs::read_to_string(config_path).unwrap_or_default()
    } else {
        String::new()
    };

    if existing.contains(TOML_MARKER) {
        eprintln!(
            "  agentic_ssh MCP server already registered in {}, skipping",
            config_path.display()
        );
        return Ok(());
    }

    let block = format!(
        "\n[[mcp_servers]]\n\
         name = \"agentic_ssh\"\n\
         transport = \"stdio\"\n\
         command = \"{agentic_ssh_bin}\"\n\
         args = [\"serve\"]\n"
    );

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config_path)
        .map_err(|e| AgenticSshError::Config {
            message: format!("failed to open {}: {e}", config_path.display()),
        })?;
    f.write_all(block.as_bytes())
        .map_err(|e| AgenticSshError::Config {
            message: format!("failed to write {}: {e}", config_path.display()),
        })?;

    eprintln!(
        "\x1b[32m✔\x1b[0m Added agentic_ssh MCP server to {}",
        config_path.display()
    );
    Ok(())
}

/// Append prompt rules to the Vibe system prompt (idempotent).
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
            eprintln!("  Vibe prompt already contains agentic_ssh rules, skipping");
            return Ok(());
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(prompt_path)
        .map_err(|e| AgenticSshError::Config {
            message: format!("failed to open {}: {e}", prompt_path.display()),
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
    .map_err(|e| AgenticSshError::Config {
        message: format!("failed to write Vibe prompt: {e}"),
    })?;
    eprintln!(
        "\x1b[32m✔\x1b[0m Added agentic_ssh rules to {}",
        prompt_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Uninstall helpers
// ---------------------------------------------------------------------------

/// Remove the agentic_ssh `[[mcp_servers]]` block from `config.toml`.
fn uninstall_mcp_server(config_path: &Path) {
    if !config_path.exists() {
        eprintln!("  {} not found, skipping", config_path.display());
        return;
    }

    let Ok(contents) = std::fs::read_to_string(config_path) else {
        return;
    };

    if !contents.contains(TOML_MARKER) {
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            config_path.display()
        );
        return;
    }

    // Remove the [[mcp_servers]] block that contains name = "agentic_ssh".
    // Strategy: split into lines, find the block, remove it.
    let lines: Vec<&str> = contents.lines().collect();
    let mut result: Vec<&str> = Vec::new();
    let mut skip = false;

    for line in &lines {
        if line.trim() == "[[mcp_servers]]" {
            // Peek ahead to see if this block is the agentic_ssh one.
            // We'll collect the block and decide whether to keep it.
            skip = false;
        }

        if skip {
            // If we hit a new section header, stop skipping.
            let trimmed = line.trim();
            if trimmed.starts_with("[[") || (trimmed.starts_with('[') && !trimmed.starts_with("[["))
            {
                skip = false;
            } else {
                continue;
            }
        }

        if line.contains(TOML_MARKER) {
            // This line is inside the agentic_ssh block — remove it and
            // the preceding [[mcp_servers]] header.
            // Pop the header we already pushed.
            while let Some(last) = result.last() {
                if last.trim() == "[[mcp_servers]]" {
                    result.pop();
                    break;
                }
                // Pop blank lines between header and this line
                if last.trim().is_empty() {
                    result.pop();
                } else {
                    break;
                }
            }
            skip = true;
            continue;
        }

        result.push(line);
    }

    // Trim trailing blank lines
    while result.last().is_some_and(|l| l.trim().is_empty()) {
        result.pop();
    }

    let new_contents = if result.is_empty() {
        String::new()
    } else {
        format!("{}\n", result.join("\n"))
    };

    if new_contents.trim().is_empty() {
        std::fs::remove_file(config_path).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            config_path.display()
        );
    } else {
        std::fs::write(config_path, &new_contents).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh MCP server from {}",
            config_path.display()
        );
    }
}

/// Remove agentic_ssh rules from the Vibe system prompt.
fn uninstall_prompt_rules(prompt_path: &Path) {
    if !prompt_path.exists() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(prompt_path) else {
        return;
    };
    if !contents.contains("agentic_ssh") {
        eprintln!("  Vibe prompt does not contain agentic_ssh rules, skipping");
        return;
    }
    let marker = "## Prefer agentic_ssh MCP tools";
    let Some(start) = contents.find(marker) else {
        return;
    };
    let before = &contents[..start];
    let trimmed = before.trim_end().to_string();
    if trimmed.is_empty() {
        std::fs::remove_file(prompt_path).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            prompt_path.display()
        );
    } else {
        std::fs::write(prompt_path, format!("{trimmed}\n")).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh rules from {}",
            prompt_path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Healthcheck helpers
// ---------------------------------------------------------------------------

fn doctor_check_config(dc: &mut DoctorCounters, home: &Path) {
    let config_path = vibe_config_path(home);

    if !config_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent vibe` if you use Mistral Vibe",
            config_path.display()
        ));
        return;
    }

    let contents = std::fs::read_to_string(&config_path).unwrap_or_default();
    if contents.contains(TOML_MARKER) {
        dc.pass(&format!(
            "MCP server registered in {}",
            config_path.display()
        ));
    } else {
        dc.fail(&format!(
            "MCP server NOT registered in {} — run `agentic_ssh install --agent vibe`",
            config_path.display()
        ));
    }
}

fn doctor_check_prompt(dc: &mut DoctorCounters, home: &Path) {
    let prompt_path = vibe_prompt_path(home);
    if prompt_path.exists() {
        let has_rules = std::fs::read_to_string(&prompt_path)
            .unwrap_or_default()
            .contains("agentic_ssh");
        if has_rules {
            dc.pass("Vibe prompt contains agentic_ssh rules");
        } else {
            dc.fail(
                "Vibe prompt missing agentic_ssh rules — run `agentic_ssh install --agent vibe`",
            );
        }
    } else {
        dc.warn("Vibe prompt does not exist");
    }
}

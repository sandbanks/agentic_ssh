//! Roo Code agent integration.
//!
//! Handles registration of the agentic_ssh MCP server in Roo Code's
//! `cline_mcp_settings.json` under the `mcpServers.agentic_ssh` key.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::errors::Result;

use super::{
    AgentIntegration, DoctorCounters, HealthcheckContext, InstallContext, InstallScope,
    backup_and_write_json, backup_config_file, load_json_file, load_json_file_strict,
    safe_write_json_file,
};

/// Roo Code agent.
pub struct RooCodeIntegration;

/// Returns the Roo Code VS Code extension global storage directory.
fn roo_ext_dir(home: &Path) -> PathBuf {
    super::vscode_data_dir(home).join("User/globalStorage/rooveterinaryinc.roo-cline")
}

/// Roo Code MCP settings path for this install: the extension global storage
/// for a global install, `<project>/.roo/mcp.json` for `--local`.
fn roo_settings_path(ctx: &InstallContext) -> PathBuf {
    match &ctx.scope {
        InstallScope::Global => roo_ext_dir(&ctx.home).join("settings/cline_mcp_settings.json"),
        InstallScope::Local { project_path } => project_path.join(".roo/mcp.json"),
    }
}

impl AgentIntegration for RooCodeIntegration {
    fn name(&self) -> &'static str {
        "Roo Code"
    }

    fn id(&self) -> &'static str {
        "roo-code"
    }

    fn install(&self, ctx: &InstallContext) -> Result<()> {
        let settings_path = roo_settings_path(ctx);

        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let backup = backup_config_file(&settings_path)?;
        let mut settings = match load_json_file_strict(&settings_path) {
            Ok(v) => v,
            Err(e) => {
                if let Some(ref b) = backup {
                    eprintln!("  Backup preserved at: {}", b.display());
                }
                return Err(e);
            }
        };
        settings["mcpServers"]["agentic_ssh"] = json!({
            "command": ctx.agentic_ssh_bin,
            "args": ["serve"],
            "disabled": false
        });

        safe_write_json_file(&settings_path, &settings, backup.as_deref())?;
        eprintln!(
            "\x1b[32m✔\x1b[0m Added agentic_ssh MCP server to {}",
            settings_path.display()
        );

        eprintln!();
        eprintln!("Setup complete. Next steps:");
        eprintln!("  1. Restart VS Code — agentic_ssh tools are now available in Roo Code");
        Ok(())
    }

    fn uninstall(&self, ctx: &InstallContext) -> Result<()> {
        let settings_path = roo_settings_path(ctx);
        uninstall_mcp_server(&settings_path);

        eprintln!();
        eprintln!("Uninstall complete. AgenticSsh has been removed from Roo Code.");
        eprintln!("Restart VS Code for changes to take effect.");
        Ok(())
    }

    fn healthcheck(&self, dc: &mut DoctorCounters, ctx: &HealthcheckContext) {
        eprintln!("\n\x1b[1mRoo Code integration\x1b[0m");
        doctor_check_settings(dc, &ctx.home);
    }

    fn is_detected(&self, home: &Path) -> bool {
        roo_ext_dir(home).is_dir()
    }

    fn has_agentic_ssh(&self, home: &Path) -> bool {
        let settings_path = roo_ext_dir(home).join("settings/cline_mcp_settings.json");
        if !settings_path.exists() {
            return false;
        }
        let json = load_json_file(&settings_path);
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

    fn primary_config_path(&self, home: &Path) -> Option<PathBuf> {
        Some(roo_ext_dir(home).join("settings/cline_mcp_settings.json"))
    }
}

// ---------------------------------------------------------------------------
// Uninstall helpers
// ---------------------------------------------------------------------------

/// Remove MCP server entry from Roo Code's `cline_mcp_settings.json`.
fn uninstall_mcp_server(settings_path: &Path) {
    if !settings_path.exists() {
        eprintln!("  {} not found, skipping", settings_path.display());
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
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            settings_path.display()
        );
        return;
    };

    if servers.remove("agentic_ssh").is_none() {
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            settings_path.display()
        );
        return;
    }

    let is_empty = settings.as_object().is_some_and(|o| {
        o.iter()
            .all(|(k, v)| k == "mcpServers" && v.as_object().is_some_and(serde_json::Map::is_empty))
    });

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

// ---------------------------------------------------------------------------
// Healthcheck helpers
// ---------------------------------------------------------------------------

/// Check Roo Code's `cline_mcp_settings.json` has agentic_ssh MCP server registered.
fn doctor_check_settings(dc: &mut DoctorCounters, home: &Path) {
    let settings_path = roo_ext_dir(home).join("settings/cline_mcp_settings.json");

    if !settings_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent roo-code` if you use Roo Code",
            settings_path.display()
        ));
        return;
    }

    let settings = load_json_file(&settings_path);
    let server = settings
        .get("mcpServers")
        .and_then(|v| v.get("agentic_ssh"));

    if server.and_then(|v| v.as_object()).is_some() {
        dc.pass(&format!(
            "MCP server registered in {}",
            settings_path.display()
        ));
    } else {
        dc.fail(&format!(
            "MCP server NOT registered in {} — run `agentic_ssh install --agent roo-code`",
            settings_path.display()
        ));
    }
}

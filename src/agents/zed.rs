//! Zed agent integration.
//!
//! Handles registration of the agentic_ssh MCP server in Zed's `settings.json`
//! under the `context_servers.agentic_ssh` key.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::errors::Result;

use super::{
    AgentIntegration, DoctorCounters, HealthcheckContext, InstallContext, InstallScope,
    backup_and_write_json, backup_config_file, load_jsonc_file, load_jsonc_file_strict,
    safe_write_json_file,
};

/// Zed agent.
pub struct ZedIntegration;

/// Returns the Zed config directory, platform-specific.
fn zed_config_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Zed")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".config/zed")
    }
}

/// Zed settings.json path for this install: the platform config dir for a
/// global install, `<project>/.zed/settings.json` for `--local`.
fn zed_settings_path(ctx: &InstallContext) -> PathBuf {
    match &ctx.scope {
        InstallScope::Global => zed_config_dir(&ctx.home).join("settings.json"),
        InstallScope::Local { project_path } => project_path.join(".zed/settings.json"),
    }
}

impl AgentIntegration for ZedIntegration {
    fn name(&self) -> &'static str {
        "Zed"
    }

    fn id(&self) -> &'static str {
        "zed"
    }

    fn install(&self, ctx: &InstallContext) -> Result<()> {
        let settings_path = zed_settings_path(ctx);

        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let backup = backup_config_file(&settings_path)?;
        let mut settings = match load_jsonc_file_strict(&settings_path) {
            Ok(v) => v,
            Err(e) => {
                if let Some(ref b) = backup {
                    eprintln!("  Backup preserved at: {}", b.display());
                }
                return Err(e);
            }
        };
        settings["context_servers"]["agentic_ssh"] = json!({
            "command": {
                "path": ctx.agentic_ssh_bin,
                "args": ["serve"]
            }
        });

        safe_write_json_file(&settings_path, &settings, backup.as_deref())?;
        eprintln!(
            "\x1b[32m✔\x1b[0m Added agentic_ssh context server to {}",
            settings_path.display()
        );

        eprintln!();
        eprintln!("Setup complete. Next steps:");
        eprintln!("  1. Restart Zed — agentic_ssh tools are now available");
        Ok(())
    }

    fn uninstall(&self, ctx: &InstallContext) -> Result<()> {
        let settings_path = zed_settings_path(ctx);
        uninstall_context_server(&settings_path);

        eprintln!();
        eprintln!("Uninstall complete. AgenticSsh has been removed from Zed.");
        eprintln!("Restart Zed for changes to take effect.");
        Ok(())
    }

    fn healthcheck(&self, dc: &mut DoctorCounters, ctx: &HealthcheckContext) {
        eprintln!("\n\x1b[1mZed integration\x1b[0m");
        doctor_check_settings(dc, &ctx.home);
    }

    fn is_detected(&self, home: &Path) -> bool {
        zed_config_dir(home).is_dir()
    }

    fn has_agentic_ssh(&self, home: &Path) -> bool {
        let settings_path = zed_config_dir(home).join("settings.json");
        if !settings_path.exists() {
            return false;
        }
        let json = load_jsonc_file(&settings_path);
        if let Some(agentic_ssh) = json
            .get("context_servers")
            .and_then(|v| v.get("agentic_ssh"))
        {
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
        Some(zed_config_dir(home).join("settings.json"))
    }
}

// ---------------------------------------------------------------------------
// Uninstall helpers
// ---------------------------------------------------------------------------

/// Remove context server entry from Zed settings.json.
/// Does not delete settings.json even if object is otherwise empty.
fn uninstall_context_server(settings_path: &Path) {
    if !settings_path.exists() {
        eprintln!("  {} not found, skipping", settings_path.display());
        return;
    }

    let mut settings = load_jsonc_file(settings_path);

    let removed = settings
        .get_mut("context_servers")
        .and_then(|v| v.as_object_mut())
        .and_then(|map| map.remove("agentic_ssh"))
        .is_some();

    if !removed {
        eprintln!(
            "  No agentic_ssh context server in {}, skipping",
            settings_path.display()
        );
        return;
    }

    // Clean up empty "context_servers" object
    let cs_empty = settings
        .get("context_servers")
        .and_then(|v| v.as_object())
        .is_some_and(serde_json::Map::is_empty);
    if cs_empty {
        settings
            .as_object_mut()
            .map(|o| o.remove("context_servers"));
    }

    // Always write back (never delete settings.json — it has other Zed settings).
    // backup_and_write_json leaves a .bak so any mistake is recoverable (issue #63).
    if backup_and_write_json(settings_path, &settings) {
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh context server from {}",
            settings_path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Healthcheck helpers
// ---------------------------------------------------------------------------

/// Check Zed settings.json has agentic_ssh context server registered.
fn doctor_check_settings(dc: &mut DoctorCounters, home: &Path) {
    let settings_path = zed_config_dir(home).join("settings.json");

    if !settings_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent zed` if you use Zed",
            settings_path.display()
        ));
        return;
    }

    let settings = load_jsonc_file(&settings_path);
    let server = settings
        .get("context_servers")
        .and_then(|v| v.get("agentic_ssh"));

    if server.and_then(|v| v.as_object()).is_some() {
        dc.pass(&format!(
            "Context server registered in {}",
            settings_path.display()
        ));
    } else {
        dc.fail(&format!(
            "Context server NOT registered in {} — run `agentic_ssh install --agent zed`",
            settings_path.display()
        ));
    }
}

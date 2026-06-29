use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Enforces that the current process is being run interactively by a human operator
/// by explicitly opening and verifying the controlling terminal device (/dev/tty).
pub fn enforce_human_interaction() -> Result<()> {
    // Attempt to open the controlling terminal directly.
    // This will fail if running inside an automated MCP subshell or non-tty stream pipe.
    let tty_result = OpenOptions::new().read(true).write(true).open("/dev/tty");

    match tty_result {
        Ok(mut tty) => {
            // Write a small challenge directly to the hardware terminal line
            tty.write_all(b"\n\x1b[33m[Security Gate]\x1b[0m Hardware TTY verified. Press [ENTER] to confirm action: ")?;
            tty.flush()?;

            // Wait for a manual newline keyboard stroke
            let mut buffer = [0; 1];
            loop {
                let bytes_read = tty.read(&mut buffer)?;
                if bytes_read == 0 || buffer[0] == b'\n' || buffer[0] == b'\r' {
                    break;
                }
            }

            // Success - execution context has a real human at the keyboard
            Ok(())
        }
        Err(_) => {
            // Exploit attempt blocked! Kill execution.
            Err(anyhow!(
                "Security Violation: This operation requires an interactive controlling terminal (/dev/tty).\n\
                Automated agents and background execution frameworks are structurally forbidden from executing this command."
            ))
        }
    }
}

/// Helper to get the path to ~/.config/agentic_ssh/trusted_locks
pub fn get_trusted_locks_path() -> Result<PathBuf> {
    let home = home::home_dir().ok_or_else(|| anyhow!("Could not resolve user home directory"))?;
    Ok(home
        .join(".config")
        .join("agentic_ssh")
        .join("trusted_locks"))
}

/// Calculates the SHA-256 hash of a file at the given path
pub fn calculate_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).context("Failed to open file for hashing")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 4096];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Cryptographically registers/trusts a local configuration file
pub fn trust_local_config(path: &Path) -> Result<()> {
    // 1. Canonicalize the path
    let canonical = path
        .canonicalize()
        .context("Failed to canonicalize configuration path")?;
    let canonical_str = canonical
        .to_str()
        .ok_or_else(|| anyhow!("Path contains invalid UTF-8"))?;

    // 2. Calculate its SHA-256 hash
    let hash = calculate_sha256(&canonical)?;

    // 3. Ensure the parent directory ~/.config/agentic_ssh exists
    let ledger_path = get_trusted_locks_path()?;
    if let Some(parent) = ledger_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // 4. Read existing trusted locks to update/avoid duplication
    let mut locks = Vec::new();
    if ledger_path.exists() {
        let file = File::open(&ledger_path)?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            locks.push(line);
        }
    }

    // Filter out any existing entries for this canonical path
    let mut new_locks = Vec::new();
    for lock in locks {
        if let Some(pos) = lock.find(':') {
            let existing_path = &lock[pos + 1..];
            if existing_path == canonical_str {
                continue;
            }
        }
        new_locks.push(lock);
    }

    // Append our new lock entry
    new_locks.push(format!("{}:{}", hash, canonical_str));

    // 5. Write the ledger with strict 0600 permissions
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&ledger_path)
            .context("Failed to open trusted_locks with 0600 permissions")?
    };

    #[cfg(not(unix))]
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&ledger_path)
        .context("Failed to open trusted_locks")?;

    for lock in new_locks {
        writeln!(file, "{}", lock)?;
    }

    Ok(())
}

/// Checks if a local configuration path is trusted by checking it against the trusted_locks ledger
pub fn is_config_trusted(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let canonical = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let canonical_str = match canonical.to_str() {
        Some(s) => s,
        None => return Ok(false),
    };

    let hash = calculate_sha256(&canonical)?;

    let ledger_path = get_trusted_locks_path()?;
    if !ledger_path.exists() {
        return Ok(false);
    }

    let file = File::open(&ledger_path)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(pos) = trimmed.find(':') {
            let entry_hash = &trimmed[..pos];
            let entry_path = &trimmed[pos + 1..];
            if entry_path == canonical_str && entry_hash == hash {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

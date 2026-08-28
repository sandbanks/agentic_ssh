use anyhow::{Context, Result};
use glob::glob;
use ssh2_config_rs::SshConfig;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub fn expand_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if let (Some(stripped), Some(home_dir)) = (path_str.strip_prefix("~/"), home::home_dir()) {
        return home_dir.join(stripped);
    }
    path.to_path_buf()
}

/// Recursively parses SSH config files starting from a base path to find all explicit host names/aliases.
/// Respects `Include` directives.
fn find_hosts_in_file(
    path: &Path,
    ssh_dir: &Path,
    visited: &mut Vec<PathBuf>,
) -> Result<Vec<String>> {
    let path = expand_path(path);
    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
    if visited.contains(&canonical) {
        return Ok(Vec::new());
    }
    visited.push(canonical);

    if !path.exists() {
        return Ok(Vec::new());
    }

    let file =
        File::open(&path).with_context(|| format!("Failed to open SSH config file: {:?}", path))?;
    let reader = BufReader::new(file);
    let mut hosts = Vec::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // SSH config is case-insensitive, split by whitespace or '='
        let parts: Vec<&str> = trimmed
            .splitn(2, |c: char| c.is_whitespace() || c == '=')
            .collect();
        if parts.len() < 2 {
            continue;
        }
        let key = parts[0].to_lowercase();
        let val = parts[1].trim().trim_matches('"');

        if key == "host" {
            // Split multiple host patterns on the same line
            for host_pattern in val.split_whitespace() {
                // Ignore wildcard patterns like '*' or '?'-based filters
                if !host_pattern.contains('*') && !host_pattern.contains('?') && host_pattern != "!"
                {
                    hosts.push(host_pattern.to_string());
                }
            }
        } else if key == "include" {
            // Include path can be relative to ~/.ssh or absolute or have tilde
            let include_path = Path::new(val);
            let target_path = if include_path.is_absolute() {
                include_path.to_path_buf()
            } else if val.starts_with("~/") {
                expand_path(include_path)
            } else {
                ssh_dir.join(include_path)
            };

            // Resolve glob pattern
            if let Some(entries) = target_path.to_str().and_then(|s| glob(s).ok()) {
                for entry in entries.flatten() {
                    if let Ok(sub_hosts) = find_hosts_in_file(&entry, ssh_dir, visited) {
                        hosts.extend(sub_hosts);
                    }
                }
            }
        }
    }

    Ok(hosts)
}

/// Recursively parses SSH config files starting from a base path to find `ConnectTimeout` for a host.
fn find_connect_timeout_in_file(
    path: &Path,
    ssh_dir: &Path,
    target_host: &str,
    visited: &mut Vec<PathBuf>,
) -> Result<Option<u64>> {
    let path = expand_path(path);
    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
    if visited.contains(&canonical) {
        return Ok(None);
    }
    visited.push(canonical);

    if !path.exists() {
        return Ok(None);
    }

    let file =
        File::open(&path).with_context(|| format!("Failed to open SSH config file: {:?}", path))?;
    let reader = BufReader::new(file);

    let mut current_host_matches = false;
    let target_host_lower = target_host.to_lowercase();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = trimmed
            .splitn(2, |c: char| c.is_whitespace() || c == '=')
            .collect();
        if parts.len() < 2 {
            continue;
        }
        let key = parts[0].to_lowercase();
        let val = parts[1].trim().trim_matches('"');

        if key == "host" {
            current_host_matches = false;
            for pattern in val.split_whitespace() {
                let pat_lower = pattern.to_lowercase();
                if let Ok(glob_pat) = glob::Pattern::new(&pat_lower)
                    && glob_pat.matches(&target_host_lower)
                {
                    current_host_matches = true;
                    break;
                }
            }
        } else if key == "connecttimeout" && current_host_matches {
            if let Ok(secs) = val.parse::<u64>() {
                return Ok(Some(secs));
            }
        } else if key == "include" {
            let include_path = Path::new(val);
            let target_path = if include_path.is_absolute() {
                include_path.to_path_buf()
            } else if val.starts_with("~/") {
                expand_path(include_path)
            } else {
                ssh_dir.join(include_path)
            };

            if let Some(entries) = target_path.to_str().and_then(|s| glob(s).ok()) {
                for entry in entries.flatten() {
                    if let Ok(Some(secs)) =
                        find_connect_timeout_in_file(&entry, ssh_dir, target_host, visited)
                    {
                        return Ok(Some(secs));
                    }
                }
            }
        }
    }

    Ok(None)
}

/// Finds the configured `ConnectTimeout` in seconds for the given target host from SSH config files.
pub fn find_connect_timeout(target_host: &str) -> Option<u64> {
    let home = home::home_dir()?;
    let ssh_dir = home.join(".ssh");
    let main_config = ssh_dir.join("config");
    let mut visited = Vec::new();
    find_connect_timeout_in_file(&main_config, &ssh_dir, target_host, &mut visited)
        .ok()
        .flatten()
}

/// Lists all explicit hosts found in the user's SSH config files.
pub fn list_ssh_hosts() -> Result<Vec<String>> {
    let home = home::home_dir().context("Could not determine home directory")?;
    let ssh_dir = home.join(".ssh");
    let main_config = ssh_dir.join("config");

    let mut visited = Vec::new();
    let mut hosts = find_hosts_in_file(&main_config, &ssh_dir, &mut visited)?;

    hosts.sort();
    hosts.dedup();
    Ok(hosts)
}

/// Loads the SshConfig object using ssh2-config-rs.
pub fn load_ssh_config() -> Result<SshConfig> {
    let home = home::home_dir().context("Could not determine home directory")?;
    let main_config = home.join(".ssh").join("config");

    if !main_config.exists() {
        return Ok(SshConfig::default());
    }

    let file = File::open(&main_config)
        .with_context(|| format!("Failed to open SSH config file: {:?}", main_config))?;
    let mut reader = BufReader::new(file);

    let config = SshConfig::default()
        .parse(&mut reader, ssh2_config_rs::ParseRule::STRICT)
        .context("Failed to parse SSH configuration")?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;

    #[test]
    fn test_expand_path() {
        let path = Path::new("~/foo/bar");
        let expanded = expand_path(path);
        if let Some(home) = home::home_dir() {
            assert_eq!(expanded, home.join("foo/bar"));
        }
    }

    #[test]
    fn test_find_hosts_in_file() -> Result<()> {
        let temp_dir = std::env::temp_dir().join("agentic_ssh_test_hosts");
        let _ = fs::remove_dir_all(&temp_dir); // Ensure clean slate
        fs::create_dir_all(&temp_dir)?;

        let main_config_path = temp_dir.join("config");
        let include_config_path = temp_dir.join("sub_config");

        let mut main_file = File::create(&main_config_path)?;
        writeln!(
            main_file,
            "Host server-a\n  HostName 10.0.0.1\n\nHost server-b server-c\n  User admin\n\nInclude {:?}",
            include_config_path
        )?;

        let mut include_file = File::create(&include_config_path)?;
        writeln!(include_file, "Host server-d\n  Port 2222")?;

        let mut visited = Vec::new();
        let hosts = find_hosts_in_file(&main_config_path, &temp_dir, &mut visited)?;

        assert!(hosts.contains(&"server-a".to_string()));
        assert!(hosts.contains(&"server-b".to_string()));
        assert!(hosts.contains(&"server-c".to_string()));
        assert!(hosts.contains(&"server-d".to_string()));
        assert_eq!(hosts.len(), 4);

        // Clean up
        let _ = fs::remove_file(main_config_path);
        let _ = fs::remove_file(include_config_path);
        let _ = fs::remove_dir(temp_dir);

        Ok(())
    }

    #[test]
    fn test_find_connect_timeout() -> Result<()> {
        let temp_dir = std::env::temp_dir().join("agentic_ssh_test_timeout");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir)?;

        let main_config_path = temp_dir.join("config");
        let mut main_file = File::create(&main_config_path)?;
        writeln!(
            main_file,
            "Host fast-box\n  HostName 10.0.0.1\n  ConnectTimeout 3\n\nHost *.prod.example.com\n  ConnectTimeout 7\n\nHost *\n  ConnectTimeout 15"
        )?;

        let mut visited = Vec::new();
        let t1 =
            find_connect_timeout_in_file(&main_config_path, &temp_dir, "fast-box", &mut visited)?;
        assert_eq!(t1, Some(3));

        let mut visited = Vec::new();
        let t2 = find_connect_timeout_in_file(
            &main_config_path,
            &temp_dir,
            "app.prod.example.com",
            &mut visited,
        )?;
        assert_eq!(t2, Some(7));

        let mut visited = Vec::new();
        let t3 = find_connect_timeout_in_file(
            &main_config_path,
            &temp_dir,
            "unknown-node",
            &mut visited,
        )?;
        assert_eq!(t3, Some(15));

        let _ = fs::remove_file(main_config_path);
        let _ = fs::remove_dir(temp_dir);

        Ok(())
    }
}

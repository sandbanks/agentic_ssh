use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHECK_INTERVAL_SECS: u64 = 86400; // 24 hours
const FETCH_TIMEOUT_SECS: u64 = 2; // 2 seconds timeout

#[derive(Serialize, Deserialize, Debug)]
pub struct VersionCache {
    pub last_check_timestamp: u64,
    pub latest_version: String,
}

fn get_cache_path() -> PathBuf {
    match home::home_dir() {
        Some(hd) => hd.join(".agentic_ssh").join("version_cache.json"),
        None => PathBuf::from(".agentic_ssh").join("version_cache.json"),
    }
}

pub fn parse_version(v: &str) -> (u64, u64, u64) {
    let clean = v.trim_start_matches('v');
    let mut parts = clean.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

pub fn is_newer_version(current: &str, latest: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn read_cache() -> Option<VersionCache> {
    let path = get_cache_path();
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_cache(cache: &VersionCache) {
    let path = get_cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(path, json);
    }
}

async fn fetch_latest_release_from_github() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent(concat!("agentic_ssh/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;

    let url = "https://api.github.com/repos/sandbanks/agentic_ssh/releases/latest";
    let resp = client.get(url).send().await.ok()?;

    if resp.status().is_success() {
        #[derive(Deserialize)]
        struct GitHubRelease {
            tag_name: String,
        }
        let release: GitHubRelease = resp.json().await.ok()?;
        return Some(release.tag_name);
    }

    None
}

/// Checks for newer versions of agentic_ssh in the background or from cache.
/// Returns Some(latest_version) if a newer version is available.
pub async fn check_for_updates() -> Option<String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let cache = read_cache();

    if let Some(ref c) = cache {
        let elapsed = now.saturating_sub(c.last_check_timestamp);
        if elapsed < CHECK_INTERVAL_SECS {
            if is_newer_version(current_version, &c.latest_version) {
                return Some(c.latest_version.clone());
            } else {
                return None;
            }
        }
    }

    if let Some(latest_tag) = fetch_latest_release_from_github().await {
        let clean_latest = latest_tag.trim_start_matches('v').to_string();
        let new_cache = VersionCache {
            last_check_timestamp: now,
            latest_version: clean_latest.clone(),
        };
        write_cache(&new_cache);

        if is_newer_version(current_version, &clean_latest) {
            return Some(clean_latest);
        }
        return None;
    }

    if let Some(c) = cache.filter(|c| is_newer_version(current_version, &c.latest_version)) {
        return Some(c.latest_version);
    }

    None
}

/// Checks and notifies the user on stderr if a newer version of agentic_ssh is available.
pub async fn notify_if_update_available() {
    let config = crate::ssh_pool::load_config();
    if config.disable_update_check {
        return;
    }
    if crate::ssh_pool::CLI_OVERRIDE
        .get()
        .map(|c| c.no_update_check)
        .unwrap_or(false)
    {
        return;
    }

    if let Some(latest) = check_for_updates().await {
        let current = env!("CARGO_PKG_VERSION");
        eprintln!(
            "💡 A new version of agentic_ssh is available: v{} (current: v{})\n   Upgrade with `cargo install agentic_ssh` or visit https://github.com/sandbanks/agentic_ssh/releases",
            latest, current
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("0.4.3"), (0, 4, 3));
        assert_eq!(parse_version("v1.2.3"), (1, 2, 3));
        assert_eq!(parse_version("v0.5.0"), (0, 5, 0));
        assert_eq!(parse_version("invalid"), (0, 0, 0));
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.4.3", "0.5.0"));
        assert!(is_newer_version("0.4.3", "v0.4.4"));
        assert!(is_newer_version("0.4.3", "1.0.0"));
        assert!(!is_newer_version("0.4.3", "0.4.3"));
        assert!(!is_newer_version("0.5.0", "0.4.3"));
    }

    #[test]
    fn test_config_disable_update_check_toml() {
        let toml_str = r#"
            disable_update_check = true
        "#;
        let config: crate::ssh_pool::Config = toml::from_str(toml_str).unwrap();
        assert!(config.disable_update_check);

        let toml_alias = r#"
            no_update_check = true
        "#;
        let config_alias: crate::ssh_pool::Config = toml::from_str(toml_alias).unwrap();
        assert!(config_alias.disable_update_check);
    }
}

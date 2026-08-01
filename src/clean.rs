use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LogAgeSummary {
    pub count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionLogsStats {
    pub total_count: usize,
    pub total_bytes: u64,
    pub older_than_2_days: LogAgeSummary,
    pub older_than_3_days: LogAgeSummary,
    pub older_than_7_days: LogAgeSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanResult {
    pub deleted_count: usize,
    pub reclaimed_bytes: u64,
    pub target_days: Option<u32>,
}

pub fn get_sessions_dir() -> PathBuf {
    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agentic_ssh")
        .join("sessions")
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn analyze_session_logs_in_dir(dir: &Path) -> SessionLogsStats {
    let mut stats = SessionLogsStats::default();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return stats,
    };

    let now = SystemTime::now();
    let sec_2_days = 2 * 86400;
    let sec_3_days = 3 * 86400;
    let sec_7_days = 7 * 86400;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len();
        stats.total_count += 1;
        stats.total_bytes += size;

        let age_secs = meta
            .modified()
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .unwrap_or(Duration::ZERO)
            .as_secs();

        if age_secs >= sec_2_days {
            stats.older_than_2_days.count += 1;
            stats.older_than_2_days.total_bytes += size;
        }
        if age_secs >= sec_3_days {
            stats.older_than_3_days.count += 1;
            stats.older_than_3_days.total_bytes += size;
        }
        if age_secs >= sec_7_days {
            stats.older_than_7_days.count += 1;
            stats.older_than_7_days.total_bytes += size;
        }
    }

    stats
}

pub fn clean_session_logs_in_dir(
    dir: &Path,
    all: bool,
    numdays: Option<u32>,
    default_days: u32,
) -> Result<CleanResult> {
    let mut deleted_count = 0;
    let mut reclaimed_bytes = 0;

    let target_days = if all {
        None
    } else {
        Some(numdays.unwrap_or(default_days))
    };

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            return Ok(CleanResult {
                deleted_count: 0,
                reclaimed_bytes: 0,
                target_days,
            });
        }
    };

    let now = SystemTime::now();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size = meta.len();

            let should_delete = if all {
                true
            } else if let Some(days) = target_days {
                let threshold_secs = (days as u64) * 86400;
                let age_secs = meta
                    .modified()
                    .ok()
                    .and_then(|mtime| now.duration_since(mtime).ok())
                    .unwrap_or(Duration::ZERO)
                    .as_secs();
                age_secs >= threshold_secs
            } else {
                false
            };

            if should_delete && fs::remove_file(&path).is_ok() {
                deleted_count += 1;
                reclaimed_bytes += size;
            }
        }
    }

    Ok(CleanResult {
        deleted_count,
        reclaimed_bytes,
        target_days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(1_500_000), "1.4 MB");
        assert_eq!(format_bytes(2_000_000_000), "1.86 GB");
    }

    #[test]
    fn test_analyze_and_clean_session_logs() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        // Initially empty
        let stats = analyze_session_logs_in_dir(path);
        assert_eq!(stats.total_count, 0);

        // Create a dummy file
        let file1 = path.join("job_1.log");
        fs::write(&file1, "hello world").unwrap();

        let stats2 = analyze_session_logs_in_dir(path);
        assert_eq!(stats2.total_count, 1);
        assert_eq!(stats2.total_bytes, 11);

        // Clean all
        let res = clean_session_logs_in_dir(path, true, None, 3).unwrap();
        assert_eq!(res.deleted_count, 1);
        assert_eq!(res.reclaimed_bytes, 11);
        assert_eq!(res.target_days, None);

        let stats3 = analyze_session_logs_in_dir(path);
        assert_eq!(stats3.total_count, 0);
    }
}

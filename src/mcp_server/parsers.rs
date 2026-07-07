use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SystemStats {
    pub load_averages: Vec<f64>,
    pub memory: MemoryStats,
    pub disks: Vec<DiskStats>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MemoryStats {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: Option<u64>,
    pub used_kb: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DiskStats {
    pub filesystem: String,
    pub size_kb: u64,
    pub used_kb: u64,
    pub available_kb: u64,
    pub use_percent: u32,
    pub mount_point: String,
}

pub fn parse_system_stats(raw_output: &str) -> SystemStats {
    let mut load_averages = Vec::new();
    let mut memory = MemoryStats {
        total_kb: 0,
        free_kb: 0,
        available_kb: None,
        used_kb: 0,
    };
    let mut disks = Vec::new();

    let parts: Vec<&str> = raw_output.split("=== ").collect();
    for part in parts {
        if part.starts_with("LOAD ===\n") {
            let content = part.trim_start_matches("LOAD ===\n");
            if let Some(first_line) = content.lines().next() {
                let tokens: Vec<&str> = first_line.split_whitespace().collect();
                if tokens.len() >= 3 && tokens[0].parse::<f64>().is_ok() {
                    for t in &tokens[..3] {
                        if let Ok(val) = t.parse::<f64>() {
                            load_averages.push(val);
                        }
                    }
                } else if let Some(pos) = first_line.rfind("load average:") {
                    let avg_str = &first_line[pos + 13..];
                    for t in avg_str.split(',') {
                        if let Ok(val) = t.trim().parse::<f64>() {
                            load_averages.push(val);
                        }
                    }
                } else if let Some(pos) = first_line.rfind("load averages:") {
                    let avg_str = &first_line[pos + 14..];
                    for t in avg_str.split_whitespace() {
                        if let Ok(val) = t.trim_matches(',').parse::<f64>() {
                            load_averages.push(val);
                        }
                    }
                }
            }
        } else if part.starts_with("MEM ===\n") {
            let content = part.trim_start_matches("MEM ===\n");
            let mut total = None;
            let mut free = None;
            let mut avail = None;

            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("MemTotal:") {
                    total = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok());
                } else if trimmed.starts_with("MemFree:") {
                    free = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok());
                } else if trimmed.starts_with("MemAvailable:") {
                    avail = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok());
                }
            }

            if let (Some(t), Some(f)) = (total, free) {
                memory.total_kb = t;
                memory.free_kb = f;
                memory.available_kb = avail;
                memory.used_kb = t.saturating_sub(avail.unwrap_or(f));
            } else {
                for line in content.lines() {
                    let parts_mem: Vec<&str> = line.split_whitespace().collect();
                    if parts_mem.len() >= 4 && parts_mem[0].starts_with("Mem:") {
                        let parsed = (
                            parts_mem[1].parse::<u64>(),
                            parts_mem[2].parse::<u64>(),
                            parts_mem[3].parse::<u64>(),
                        );
                        if let (Ok(t), Ok(u), Ok(f)) = parsed {
                            memory.total_kb = t;
                            memory.free_kb = f;
                            memory.used_kb = u;
                            if parts_mem.len() >= 7 {
                                memory.available_kb = parts_mem[6].parse::<u64>().ok();
                            }
                        }
                    }
                }
            }
        } else if part.starts_with("DISK ===\n") {
            let content = part.trim_start_matches("DISK ===\n");
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with("Filesystem") {
                    continue;
                }
                let parts_disk: Vec<&str> = line.split_whitespace().collect();
                if parts_disk.len() >= 6 {
                    let fs = parts_disk[0].to_string();
                    if fs == "tmpfs"
                        || fs == "devtmpfs"
                        || fs == "udev"
                        || fs.starts_with("/dev/loop")
                    {
                        continue;
                    }
                    if let (Ok(size), Ok(used), Ok(avail)) = (
                        parts_disk[1].parse::<u64>(),
                        parts_disk[2].parse::<u64>(),
                        parts_disk[3].parse::<u64>(),
                    ) {
                        let pct = parts_disk[4]
                            .trim_end_matches('%')
                            .parse::<u32>()
                            .unwrap_or(0);
                        let mount = parts_disk[5].to_string();
                        disks.push(DiskStats {
                            filesystem: fs,
                            size_kb: size,
                            used_kb: used,
                            available_kb: avail,
                            use_percent: pct,
                            mount_point: mount,
                        });
                    }
                }
            }
        }
    }

    SystemStats {
        load_averages,
        memory,
        disks,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ListeningPort {
    pub proto: String,
    pub local_address: String,
    pub port: u32,
    pub process: Option<String>,
    pub pid: Option<u32>,
}

pub fn parse_listening_ports(raw_output: &str, filter_port: Option<u32>) -> Vec<ListeningPort> {
    let mut results = Vec::new();
    for line in raw_output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Active")
            || line.starts_with("Proto")
            || line.starts_with("Netid")
        {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }

        let proto = parts[0].to_lowercase();
        if !proto.contains("tcp") && !proto.contains("udp") {
            continue;
        }
        let clean_proto = if proto.contains("tcp") {
            "tcp".to_string()
        } else {
            "udp".to_string()
        };

        let local_addr_str = if parts.len() >= 5 && parts[4].contains(':') {
            parts[4]
        } else if parts[3].contains(':') {
            parts[3]
        } else {
            let mut found = None;
            for p in &parts[3..] {
                if p.contains(':') {
                    found = Some(*p);
                    break;
                }
            }
            match found {
                Some(f) => f,
                None => continue,
            }
        };

        let last_colon = match local_addr_str.rfind(':') {
            Some(idx) => idx,
            None => continue,
        };

        let local_address = local_addr_str[..last_colon].to_string();
        let port_str = &local_addr_str[last_colon + 1..];
        let port = match port_str.parse::<u32>() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if filter_port.is_some_and(|fp| port != fp) {
            continue;
        }

        let mut process = None;
        let mut pid = None;

        let remaining_line = line;
        if let Some(pos) = remaining_line.find('/') {
            let parts_slash: Vec<&str> = remaining_line[..pos].split_whitespace().collect();
            if let Some(pid_val) = parts_slash.last().and_then(|t| t.parse::<u32>().ok()) {
                pid = Some(pid_val);
                let after_slash = &remaining_line[pos + 1..];
                if let Some(space_pos) = after_slash.find(char::is_whitespace) {
                    process = Some(after_slash[..space_pos].to_string());
                } else {
                    process = Some(after_slash.to_string());
                }
            }
        } else if let Some(pid_idx) = remaining_line.find("pid=") {
            let pid_str = &remaining_line[pid_idx + 4..];
            if let Some(pid_val) = pid_str
                .split(',')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
            {
                pid = Some(pid_val);
            }
            if let Some(users_idx) = remaining_line.find("users:((\"") {
                let proc_str = &remaining_line[users_idx + 9..];
                if let Some(quote_pos) = proc_str.find('"') {
                    process = Some(proc_str[..quote_pos].to_string());
                }
            }
        }

        results.push(ListeningPort {
            proto: clean_proto,
            local_address,
            port,
            process,
            pid,
        });
    }

    let mut seen = std::collections::HashSet::new();
    results.retain(|p| seen.insert((p.proto.clone(), p.port)));

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ps_line() {
        let line = " 1234 richard   0.5  1.2 /usr/local/bin/localmail serve --port 80";
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts[0].parse::<u32>().unwrap(), 1234);
        assert_eq!(parts[1], "richard");
        assert_eq!(parts[2], "0.5");
        assert_eq!(parts[3], "1.2");
        assert_eq!(
            parts[4..].join(" "),
            "/usr/local/bin/localmail serve --port 80"
        );

        let header = "  PID USER      %CPU %MEM COMMAND";
        let parts_header: Vec<&str> = header.split_whitespace().collect();
        assert!(parts_header[0].parse::<u32>().is_err());
    }

    #[test]
    fn test_parse_system_stats() {
        let raw = "\
=== LOAD ===
0.15 0.08 0.05 1/450 12345
=== MEM ===
MemTotal:       16278272 kB
MemFree:         4829104 kB
MemAvailable:   11000200 kB
=== DISK ===
Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/sda1        105291040  45192040  60099000      43% /
tmpfs              8139136         0   8139136       0% /dev/shm
";
        let stats = parse_system_stats(raw);
        assert_eq!(stats.load_averages, vec![0.15, 0.08, 0.05]);
        assert_eq!(stats.memory.total_kb, 16278272);
        assert_eq!(stats.memory.free_kb, 4829104);
        assert_eq!(stats.memory.available_kb, Some(11000200));
        assert_eq!(stats.memory.used_kb, 16278272 - 11000200);

        assert_eq!(stats.disks.len(), 1);
        assert_eq!(stats.disks[0].filesystem, "/dev/sda1");
        assert_eq!(stats.disks[0].size_kb, 105291040);
        assert_eq!(stats.disks[0].used_kb, 45192040);
        assert_eq!(stats.disks[0].available_kb, 60099000);
        assert_eq!(stats.disks[0].use_percent, 43);
        assert_eq!(stats.disks[0].mount_point, "/");
    }

    #[test]
    fn test_parse_listening_ports() {
        let raw_ss = "\
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port Process
tcp   LISTEN 0      4096         0.0.0.0:80          0.0.0.0:*     users:((\"nginx\",pid=123,fd=6))
tcp   LISTEN 0      4096            [::]:80             [::]:*     users:((\"nginx\",pid=123,fd=6))
udp   UNCONN 0      0            0.0.0.0:53          0.0.0.0:*     users:((\"named\",pid=456,fd=7))
";
        let ports = parse_listening_ports(raw_ss, None);
        assert_eq!(ports.len(), 2);

        assert_eq!(ports[0].proto, "tcp");
        assert_eq!(ports[0].local_address, "0.0.0.0");
        assert_eq!(ports[0].port, 80);
        assert_eq!(ports[0].process, Some("nginx".to_string()));
        assert_eq!(ports[0].pid, Some(123));

        assert_eq!(ports[1].proto, "udp");
        assert_eq!(ports[1].local_address, "0.0.0.0");
        assert_eq!(ports[1].port, 53);
        assert_eq!(ports[1].process, Some("named".to_string()));
        assert_eq!(ports[1].pid, Some(456));

        let filtered = parse_listening_ports(raw_ss, Some(53));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].port, 53);
    }
}

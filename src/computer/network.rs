//! Cross-platform network status inspection.
//!
//! Provides port usage, connectivity testing, and firewall rule reading
//! without external dependencies beyond the system shell.
//!
//! # Usage
//!
//! ```rust,no_run
//! use syscity::computer::network::NetworkInspector;
//!
//! let inspector = NetworkInspector::new();
//! let ports = inspector.list_ports(None, None).unwrap();
//! for p in &ports {
//!     println!("{}:{} -> {} ({})", p.local_addr, p.local_port, p.state, p.process_name);
//! }
//! ```

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// A single listening or established socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortEntry {
    pub protocol: String,   // "tcp", "udp", "tcp6", "udp6"
    pub local_addr: String, // e.g. "0.0.0.0" or "::"
    pub local_port: u16,
    pub remote_addr: String, // e.g. "0.0.0.0" for listeners
    pub remote_port: u16,
    pub state: String, // "LISTEN", "ESTABLISHED", "TIME_WAIT", ...
    pub process_name: String,
    pub pid: Option<u32>,
}

/// Result of a ping probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingResult {
    pub target: String,
    pub packets_sent: u32,
    pub packets_received: u32,
    pub packet_loss_percent: f32,
    pub min_latency_ms: f64,
    pub avg_latency_ms: f64,
    pub max_latency_ms: f64,
    pub success: bool,
    pub message: String,
}

/// Result of a TCP connectivity test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TcpConnectResult {
    pub target: String,
    pub port: u16,
    pub success: bool,
    pub latency_ms: f64,
    pub message: String,
}

/// A single firewall rule (best-effort parsing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirewallRule {
    pub chain: String,  // e.g. "INPUT", "OUTPUT"
    pub action: String, // e.g. "ACCEPT", "DROP", "REJECT"
    pub protocol: String,
    pub source: String,
    pub destination: String,
    pub dport: Option<String>,
    pub extra: String,
}

/// Cross-platform network inspector.
#[derive(Debug, Clone, Default)]
pub struct NetworkInspector;

impl NetworkInspector {
    /// Create a new inspector.
    pub fn new() -> Self {
        Self
    }

    /// List all network sockets with process information.
    ///
    /// `filter_protocol` — "tcp", "udp", "tcp6", "udp6", or `None` for all.
    /// `filter_state`    — "LISTEN", "ESTABLISHED", etc., or `None` for all.
    pub fn list_ports(
        &self,
        filter_protocol: Option<&str>,
        filter_state: Option<&str>,
    ) -> crate::Result<Vec<PortEntry>> {
        #[cfg(target_os = "linux")]
        {
            self.list_ports_linux(filter_protocol, filter_state)
        }
        #[cfg(target_os = "macos")]
        {
            self.list_ports_macos(filter_protocol, filter_state)
        }
        #[cfg(target_os = "windows")]
        {
            self.list_ports_windows(filter_protocol, filter_state)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(crate::error::SyscityError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ))
        }
    }

    /// Test connectivity to a host via ICMP ping.
    ///
    /// `count` — number of packets to send (default 4).
    pub async fn test_ping(&self, target: &str, count: Option<u32>) -> PingResult {
        let count = count.unwrap_or(4);
        let start = Instant::now();

        #[cfg(target_os = "windows")]
        let output = tokio::process::Command::new("ping")
            .arg("-n")
            .arg(count.to_string())
            .arg(target)
            .output()
            .await;
        #[cfg(not(target_os = "windows"))]
        let output = tokio::process::Command::new("ping")
            .arg("-c")
            .arg(count.to_string())
            .arg(target)
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let text = if stdout.is_empty() { stderr } else { stdout };
                Self::parse_ping_output(target, count, &text, start.elapsed())
            }
            Err(e) => PingResult {
                target: target.to_string(),
                packets_sent: count,
                packets_received: 0,
                packet_loss_percent: 100.0,
                min_latency_ms: 0.0,
                avg_latency_ms: 0.0,
                max_latency_ms: 0.0,
                success: false,
                message: format!("Failed to run ping: {}", e),
            },
        }
    }

    /// Test TCP connectivity to a specific host:port.
    pub async fn test_tcp_connect(
        &self,
        target: &str,
        port: u16,
        timeout: Option<Duration>,
    ) -> TcpConnectResult {
        let timeout = timeout.unwrap_or(Duration::from_secs(5));
        let addr = format!("{}:{}", target, port);
        let start = Instant::now();

        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(_stream)) => {
                let latency = start.elapsed().as_secs_f64() * 1000.0;
                TcpConnectResult {
                    target: target.to_string(),
                    port,
                    success: true,
                    latency_ms: latency,
                    message: format!("Connected to {} in {:.2}ms", addr, latency),
                }
            }
            Ok(Err(e)) => TcpConnectResult {
                target: target.to_string(),
                port,
                success: false,
                latency_ms: start.elapsed().as_secs_f64() * 1000.0,
                message: format!("Connection refused/failed: {}", e),
            },
            Err(_) => TcpConnectResult {
                target: target.to_string(),
                port,
                success: false,
                latency_ms: timeout.as_secs_f64() * 1000.0,
                message: format!("Connection to {} timed out after {:?}", addr, timeout),
            },
        }
    }

    /// Read firewall rules (best-effort, platform-specific).
    pub async fn list_firewall_rules(&self) -> crate::Result<Vec<FirewallRule>> {
        #[cfg(target_os = "linux")]
        {
            self.list_firewall_rules_linux().await
        }
        #[cfg(target_os = "macos")]
        {
            self.list_firewall_rules_macos().await
        }
        #[cfg(target_os = "windows")]
        {
            self.list_firewall_rules_windows().await
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(crate::error::SyscityError::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
            ))
        }
    }

    // ── Linux implementation ────────────────────────────────────────────────

    #[cfg(target_os = "linux")]
    fn list_ports_linux(
        &self,
        filter_protocol: Option<&str>,
        filter_state: Option<&str>,
    ) -> crate::Result<Vec<PortEntry>> {
        // Try `ss` first (modern), fall back to `netstat`.
        let output = std::process::Command::new("ss").args(["-tunap"]).output();

        let text = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => {
                let fallback = std::process::Command::new("netstat")
                    .args(["-tunap"])
                    .output();
                match fallback {
                    Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
                    _ => {
                        return Err(crate::error::SyscityError::Internal(
                            "Neither `ss` nor `netstat` is available".to_string(),
                        ))
                    }
                }
            }
        };

        let mut entries = Vec::new();
        for line in text.lines().skip(1) {
            // ss format:  Netid  State   Recv-Q  Send-Q  Local Address:Port  Peer
            // Address:Port  Process netstat:    proto  recv-q  send-q  local
            // address       foreign address    state   pid/program
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 5 {
                continue;
            }

            let (protocol, state, local, remote, process_col) =
                if cols[0].starts_with("tcp") || cols[0].starts_with("udp") {
                    // ss format
                    let proto = cols[0].to_lowercase();
                    let state_str = cols.get(1).unwrap_or(&"").to_string();
                    let local = cols.get(4).unwrap_or(&"").to_string();
                    let remote = cols.get(5).unwrap_or(&"").to_string();
                    let proc_col = cols.get(6).map(|s| s.to_string()).unwrap_or_default();
                    (proto, state_str, local, remote, proc_col)
                } else {
                    // netstat format (rough)
                    let proto = cols[0].to_lowercase();
                    let local = cols[3].to_string();
                    let remote = cols[4].to_string();
                    let state_str = cols.get(5).unwrap_or(&"").to_string();
                    let proc_col = cols.get(6).map(|s| s.to_string()).unwrap_or_default();
                    (proto, state_str, local, remote, proc_col)
                };

            if let Some(f) = filter_protocol {
                if !protocol.contains(f) {
                    continue;
                }
            }
            if let Some(f) = filter_state {
                if !state.eq_ignore_ascii_case(f) {
                    continue;
                }
            }

            let (local_addr, local_port) = Self::parse_addr_port(&local);
            let (remote_addr, remote_port) = Self::parse_addr_port(&remote);
            let (pid, process_name) = Self::parse_process(&process_col);

            entries.push(PortEntry {
                protocol,
                local_addr,
                local_port,
                remote_addr,
                remote_port,
                state,
                process_name,
                pid,
            });
        }

        Ok(entries)
    }

    #[cfg(target_os = "linux")]
    async fn list_firewall_rules_linux(&self) -> crate::Result<Vec<FirewallRule>> {
        let mut rules = Vec::new();

        // Try nftables first
        let nft = tokio::process::Command::new("nft")
            .args(["list", "ruleset"])
            .output()
            .await;

        if let Ok(o) = nft {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("type filter")
                        || trimmed.starts_with("type nat")
                        || trimmed.is_empty()
                        || trimmed.starts_with('#')
                    {
                        continue;
                    }
                    if trimmed.contains("accept")
                        || trimmed.contains("drop")
                        || trimmed.contains("reject")
                    {
                        rules.push(FirewallRule {
                            chain: "nftables".to_string(),
                            action: if trimmed.contains("accept") {
                                "ACCEPT"
                            } else if trimmed.contains("drop") {
                                "DROP"
                            } else {
                                "REJECT"
                            }
                            .to_string(),
                            protocol: "any".to_string(),
                            source: "any".to_string(),
                            destination: "any".to_string(),
                            dport: None,
                            extra: trimmed.to_string(),
                        });
                    }
                }
            }
        }

        // Fall back to iptables
        let ipt = tokio::process::Command::new("iptables")
            .args(["-L", "-n", "-v"])
            .output()
            .await;

        if let Ok(o) = ipt {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                let mut current_chain = String::new();
                for line in text.lines() {
                    if line.starts_with("Chain ") {
                        current_chain = line.split_whitespace().nth(1).unwrap_or("").to_string();
                        continue;
                    }
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    if cols.len() >= 9 {
                        rules.push(FirewallRule {
                            chain: current_chain.clone(),
                            action: cols[2].to_string(),
                            protocol: cols[3].to_string(),
                            source: cols[7].to_string(),
                            destination: cols[8].to_string(),
                            dport: cols.get(11).map(|s| s.to_string()),
                            extra: line.to_string(),
                        });
                    }
                }
            }
        }

        Ok(rules)
    }

    // ── macOS implementation ────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    fn list_ports_macos(
        &self,
        filter_protocol: Option<&str>,
        filter_state: Option<&str>,
    ) -> crate::Result<Vec<PortEntry>> {
        let output = std::process::Command::new("netstat")
            .args(["-anv"])
            .output();

        let text = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => {
                return Err(crate::error::SyscityError::Internal(
                    "`netstat` not available".to_string(),
                ))
            }
        };

        let mut entries = Vec::new();
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 6 {
                continue;
            }
            let proto = cols[0].to_lowercase();
            if !proto.starts_with("tcp") && !proto.starts_with("udp") {
                continue;
            }
            let local = cols[3].to_string();
            let remote = cols[4].to_string();
            let state = cols[5].to_string();

            if let Some(f) = filter_protocol {
                if !proto.contains(f) {
                    continue;
                }
            }
            if let Some(f) = filter_state {
                if !state.eq_ignore_ascii_case(f) {
                    continue;
                }
            }

            let (local_addr, local_port) = Self::parse_addr_port(&local);
            let (remote_addr, remote_port) = Self::parse_addr_port(&remote);

            entries.push(PortEntry {
                protocol: proto,
                local_addr,
                local_port,
                remote_addr,
                remote_port,
                state,
                process_name: String::new(),
                pid: None,
            });
        }

        Ok(entries)
    }

    #[cfg(target_os = "macos")]
    async fn list_firewall_rules_macos(&self) -> crate::Result<Vec<FirewallRule>> {
        // Try with sudo first, then fall back to regular pfctl.
        let text = match tokio::process::Command::new("sudo")
            .arg("-n")
            .arg("pfctl")
            .arg("-sr")
            .output()
            .await
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => match tokio::process::Command::new("pfctl").arg("-sr").output().await {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
                _ => String::new(),
            },
        };

        let mut rules = Vec::new();

        if text.is_empty() || text.contains("Permission denied") {
            rules.push(FirewallRule {
                chain: "pf".to_string(),
                action: "INFO".to_string(),
                protocol: "any".to_string(),
                source: "any".to_string(),
                destination: "any".to_string(),
                dport: None,
                extra: "pfctl requires root or explicit permission".to_string(),
            });
        } else {
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let action = if trimmed.contains(" pass ") {
                    "PASS"
                } else if trimmed.contains(" block ") {
                    "BLOCK"
                } else if trimmed.contains(" match ") {
                    "MATCH"
                } else {
                    "RULE"
                };
                rules.push(FirewallRule {
                    chain: "pf".to_string(),
                    action: action.to_string(),
                    protocol: "any".to_string(),
                    source: "any".to_string(),
                    destination: "any".to_string(),
                    dport: None,
                    extra: trimmed.to_string(),
                });
            }
        }

        Ok(rules)
    }

    // ── Windows implementation ──────────────────────────────────────────────

    #[cfg(target_os = "windows")]
    fn list_ports_windows(
        &self,
        filter_protocol: Option<&str>,
        filter_state: Option<&str>,
    ) -> crate::Result<Vec<PortEntry>> {
        let output = std::process::Command::new("netstat")
            .args(["-ano"])
            .output();

        let text = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => {
                return Err(crate::error::SyscityError::Internal(
                    "`netstat` not available".to_string(),
                ))
            }
        };

        let mut entries = Vec::new();
        for line in text.lines().skip(3) {
            // Format:  Proto  Local Address          Foreign Address        State
            // PID
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 {
                continue;
            }
            let proto = cols[0].to_lowercase();
            let local = cols[1].to_string();
            let remote = cols[2].to_string();
            let (state, pid_str) = if cols.len() >= 5 {
                (cols[3].to_string(), cols[4])
            } else {
                ("UNKNOWN".to_string(), cols[3])
            };

            if let Some(f) = filter_protocol {
                if !proto.contains(f) {
                    continue;
                }
            }
            if let Some(f) = filter_state {
                if !state.eq_ignore_ascii_case(f) {
                    continue;
                }
            }

            let (local_addr, local_port) = Self::parse_addr_port(&local);
            let (remote_addr, remote_port) = Self::parse_addr_port(&remote);
            let pid = pid_str.parse::<u32>().ok();
            let process_name = pid.map_or(String::new(), |p| format!("pid:{}", p));

            entries.push(PortEntry {
                protocol: proto,
                local_addr,
                local_port,
                remote_addr,
                remote_port,
                state,
                process_name,
                pid,
            });
        }

        Ok(entries)
    }

    #[cfg(target_os = "windows")]
    async fn list_firewall_rules_windows(&self) -> crate::Result<Vec<FirewallRule>> {
        let output = tokio::process::Command::new("powershell")
            .args([
                "-Command",
                "Get-NetFirewallRule | Select-Object DisplayName, Direction, Action, Enabled | \
                 ConvertTo-Json -Compress",
            ])
            .output()
            .await;

        let mut rules = Vec::new();

        match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                // PowerShell may return a single object or an array
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                    let items = if val.is_array() {
                        val.as_array().unwrap().clone()
                    } else {
                        vec![val]
                    };
                    for item in items {
                        rules.push(FirewallRule {
                            chain: item["Direction"].as_str().unwrap_or("").to_string(),
                            action: item["Action"].as_str().unwrap_or("").to_string(),
                            protocol: "any".to_string(),
                            source: "any".to_string(),
                            destination: "any".to_string(),
                            dport: None,
                            extra: item["DisplayName"].as_str().unwrap_or("").to_string(),
                        });
                    }
                }
            }
            _ => {
                // Fallback to netsh
                let fallback = tokio::process::Command::new("netsh")
                    .args(["advfirewall", "firewall", "show", "rule", "name=all"])
                    .output()
                    .await;
                if let Ok(o) = fallback {
                    let text = String::from_utf8_lossy(&o.stdout);
                    let mut current_name = String::new();
                    for line in text.lines() {
                        if line.starts_with("Rule Name:") {
                            current_name =
                                line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
                        } else if line.starts_with("Action:") {
                            let action =
                                line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
                            rules.push(FirewallRule {
                                chain: "advfirewall".to_string(),
                                action,
                                protocol: "any".to_string(),
                                source: "any".to_string(),
                                destination: "any".to_string(),
                                dport: None,
                                extra: current_name.clone(),
                            });
                        }
                    }
                }
            }
        }

        Ok(rules)
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn parse_addr_port(addr_port: &str) -> (String, u16) {
        if let Some(pos) = addr_port.rfind(':') {
            let addr = addr_port[..pos].to_string();
            let port = addr_port[pos + 1..].parse::<u16>().unwrap_or(0);
            (addr, port)
        } else if let Some(_pos) = addr_port.rfind('.') {
            // ss may use "0.0.0.0:22" format already handled above
            (addr_port.to_string(), 0)
        } else {
            (addr_port.to_string(), 0)
        }
    }

    #[allow(dead_code)]
    fn parse_process(proc_col: &str) -> (Option<u32>, String) {
        // ss format:  users:(("nginx",pid=1234,fd=3))
        // netstat:    1234/nginx
        if let Some(pid_start) = proc_col.find("pid=") {
            let pid_part = &proc_col[pid_start + 4..];
            let pid_end = pid_part.find(',').unwrap_or(pid_part.len());
            let pid = pid_part[..pid_end].parse::<u32>().ok();
            let name = if let Some(q1) = proc_col.find('"') {
                let rest = &proc_col[q1 + 1..];
                if let Some(q2) = rest.find('"') {
                    rest[..q2].to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            return (pid, name);
        }
        if let Some(slash) = proc_col.find('/') {
            let pid = proc_col[..slash].parse::<u32>().ok();
            let name = proc_col[slash + 1..].to_string();
            return (pid, name);
        }
        (None, proc_col.to_string())
    }

    fn parse_ping_output(target: &str, count: u32, text: &str, _elapsed: Duration) -> PingResult {
        let mut received = 0u32;
        let mut min_ms = f64::MAX;
        let mut max_ms = 0f64;
        let mut sum_ms = 0f64;
        let mut valid = 0u32;

        for line in text.lines() {
            // Linux: 64 bytes from 1.1.1.1: icmp_seq=1 ttl=58 time=12.3 ms
            // macOS: 64 bytes from 1.1.1.1: icmp_seq=0 ttl=58 time=12.345 ms
            // Windows: Reply from 1.1.1.1: bytes=32 time=12ms TTL=58
            if line.contains("time=") {
                received += 1;
            }
            if let Some(pos) = line.find("time=") {
                let rest = &line[pos + 5..];
                let end = rest
                    .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
                    .unwrap_or(rest.len());
                let num_str = &rest[..end].replace(',', ".");
                if let Ok(ms) = num_str.parse::<f64>() {
                    min_ms = min_ms.min(ms);
                    max_ms = max_ms.max(ms);
                    sum_ms += ms;
                    valid += 1;
                }
            }
        }

        // Summary line parsing for packet loss
        let loss = if text.contains("100% packet loss") {
            100.0
        } else if text.contains("0% packet loss") {
            0.0
        } else {
            let sent_f = count as f32;
            let recv_f = received as f32;
            if sent_f > 0.0 {
                ((sent_f - recv_f) / sent_f * 100.0).max(0.0)
            } else {
                0.0
            }
        };

        let avg = if valid > 0 {
            sum_ms / valid as f64
        } else {
            0.0
        };
        let min_val = if min_ms != f64::MAX { min_ms } else { 0.0 };

        let success = received > 0;
        let message = if success {
            format!(
                "Ping {}: {}/{} packets received, {:.1}% loss, avg {:.2}ms",
                target, received, count, loss, avg
            )
        } else {
            format!("Ping {} failed: no response", target)
        };

        PingResult {
            target: target.to_string(),
            packets_sent: count,
            packets_received: received,
            packet_loss_percent: loss,
            min_latency_ms: min_val,
            avg_latency_ms: avg,
            max_latency_ms: max_ms,
            success,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_addr_port_ipv4() {
        assert_eq!(NetworkInspector::parse_addr_port("0.0.0.0:22"), ("0.0.0.0".to_string(), 22));
        assert_eq!(
            NetworkInspector::parse_addr_port("127.0.0.1:8080"),
            ("127.0.0.1".to_string(), 8080)
        );
    }

    #[test]
    fn test_parse_addr_port_ipv6() {
        assert_eq!(NetworkInspector::parse_addr_port("[::]:22"), ("[::]".to_string(), 22));
        assert_eq!(NetworkInspector::parse_addr_port("[::1]:5432"), ("[::1]".to_string(), 5432));
    }

    #[test]
    fn test_parse_process_ss_format() {
        let (pid, name) = NetworkInspector::parse_process("users:((\"nginx\",pid=1234,fd=3))");
        assert_eq!(pid, Some(1234));
        assert_eq!(name, "nginx");
    }

    #[test]
    fn test_parse_process_netstat_format() {
        let (pid, name) = NetworkInspector::parse_process("1234/nginx");
        assert_eq!(pid, Some(1234));
        assert_eq!(name, "nginx");
    }

    #[test]
    fn test_ping_result_parsing_linux() {
        let text = r#"PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.
64 bytes from 1.1.1.1: icmp_seq=1 ttl=58 time=12.3 ms
64 bytes from 1.1.1.1: icmp_seq=2 ttl=58 time=11.8 ms
64 bytes from 1.1.1.1: icmp_seq=3 ttl=58 time=13.1 ms
64 bytes from 1.1.1.1: icmp_seq=4 ttl=58 time=12.0 ms

--- 1.1.1.1 ping statistics ---
4 packets transmitted, 4 received, 0% packet loss, time 3004ms
rtt min/avg/max/mdev = 11.800/12.300/13.100/0.450 ms"#;

        let result =
            NetworkInspector::parse_ping_output("1.1.1.1", 4, text, Duration::from_secs(3));
        assert!(result.success);
        assert_eq!(result.packets_received, 4);
        assert_eq!(result.packet_loss_percent, 0.0);
        assert!(result.avg_latency_ms > 0.0);
    }

    #[test]
    fn test_ping_result_parsing_timeout() {
        let text = r#"PING 192.0.2.1 (192.0.2.1) 56(84) bytes of data.

--- 192.0.2.1 ping statistics ---
4 packets transmitted, 0 received, 100% packet loss, time 3000ms"#;

        let result =
            NetworkInspector::parse_ping_output("192.0.2.1", 4, text, Duration::from_secs(3));
        assert!(!result.success);
        assert_eq!(result.packets_received, 0);
        assert_eq!(result.packet_loss_percent, 100.0);
    }

    #[test]
    fn test_port_entry_serde() {
        let entry = PortEntry {
            protocol: "tcp".to_string(),
            local_addr: "0.0.0.0".to_string(),
            local_port: 22,
            remote_addr: "0.0.0.0".to_string(),
            remote_port: 0,
            state: "LISTEN".to_string(),
            process_name: "sshd".to_string(),
            pid: Some(1234),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("sshd"));
        assert!(json.contains("LISTEN"));
    }
}

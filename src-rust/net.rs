use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetInterface {
    pub name: String,
    pub address: String,
    pub netmask: String,
    pub broadcast: String,
    pub is_preferred: bool,
}

/// Calcula o endereco de broadcast dirigido da sub-rede (`ip | (~mask)`).
pub fn compute_broadcast(ip: Ipv4Addr, mask: Ipv4Addr) -> Ipv4Addr {
    let ip_u32 = u32::from(ip);
    let mask_u32 = u32::from(mask);
    if mask_u32 == 0 {
        return Ipv4Addr::BROADCAST;
    }
    Ipv4Addr::from(ip_u32 | !mask_u32)
}

/// Descobre qual IPv4 local o sistema operacional usa para a rota padrao.
pub fn preferred_local_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

/// Lista interfaces IPv4 ativas no Windows usando PowerShell/Get-NetIPAddress ou fallback UDP.
pub fn list_interfaces() -> Vec<NetInterface> {
    let preferred = preferred_local_ipv4().map(|ip| ip.to_string());
    let mut out = Vec::new();

    if cfg!(target_os = "windows") {
        if let Ok(output) = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -ne '127.0.0.1' -and $_.PrefixOrigin -ne 'WellKnown' } | Select-Object InterfaceAlias, IPAddress, PrefixLength | ConvertTo-Json -Compress",
            ])
            .output()
        {
            if let Ok(text) = String::from_utf8(output.stdout) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
                    if let Ok(val) = parsed {
                        let items = match val {
                            serde_json::Value::Array(arr) => arr,
                            obj @ serde_json::Value::Object(_) => vec![obj],
                            _ => vec![],
                        };
                        for item in items {
                            let name = item
                                .get("InterfaceAlias")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Ethernet")
                                .to_string();
                            let addr_str = item
                                .get("IPAddress")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let prefix = item
                                .get("PrefixLength")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(24) as u32;

                            if addr_str.starts_with("169.254.") {
                                continue;
                            }
                            if let Ok(ipv4) = addr_str.parse::<Ipv4Addr>() {
                                let mask_u32 = if prefix == 0 {
                                    0
                                } else {
                                    !0u32 << (32 - prefix.min(32))
                                };
                                let mask = Ipv4Addr::from(mask_u32);
                                let broadcast = compute_broadcast(ipv4, mask);
                                let is_preferred = preferred.as_deref() == Some(addr_str.as_str());
                                out.push(NetInterface {
                                    name,
                                    address: addr_str,
                                    netmask: mask.to_string(),
                                    broadcast: broadcast.to_string(),
                                    is_preferred,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    if out.is_empty() {
        if let Some(ip) = preferred_local_ipv4() {
            let mask = Ipv4Addr::new(255, 255, 255, 0);
            out.push(NetInterface {
                name: "Rede Local".to_string(),
                address: ip.to_string(),
                netmask: mask.to_string(),
                broadcast: compute_broadcast(ip, mask).to_string(),
                is_preferred: true,
            });
        }
    } else if !out.iter().any(|i| i.is_preferred) {
        if let Some(first) = out.first_mut() {
            first.is_preferred = true;
        }
    }

    out.sort_by(|a, b| b.is_preferred.cmp(&a.is_preferred).then(a.name.cmp(&b.name)));
    out
}

pub fn resolve_local_address(preferred_interface: Option<&str>) -> String {
    let list = list_interfaces();
    if let Some(pref) = preferred_interface {
        if let Some(found) = list.iter().find(|i| i.address == pref) {
            return found.address.clone();
        }
    }
    if let Some(found) = list.iter().find(|i| i.is_preferred) {
        return found.address.clone();
    }
    list.first()
        .map(|i| i.address.clone())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

pub fn broadcast_targets(preferred_interface: Option<&str>) -> Vec<String> {
    let list = list_interfaces();
    let chosen: Vec<&NetInterface> = if let Some(pref) = preferred_interface {
        let filtered: Vec<&NetInterface> = list.iter().filter(|i| i.address == pref).collect();
        if filtered.is_empty() {
            list.iter().collect()
        } else {
            filtered
        }
    } else {
        list.iter().collect()
    };

    let mut targets = Vec::new();
    for item in chosen {
        if !targets.contains(&item.broadcast) {
            targets.push(item.broadcast.clone());
        }
    }
    let global = "255.255.255.255".to_string();
    if !targets.contains(&global) {
        targets.push(global);
    }
    targets
}

pub fn find_free_port(start_port: u16) -> u16 {
    for p in start_port..(start_port + 30) {
        if TcpListener::bind(("0.0.0.0", p)).is_ok() {
            return p;
        }
    }
    TcpListener::bind(("0.0.0.0", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(start_port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_broadcast() {
        let ip = Ipv4Addr::new(192, 168, 1, 42);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        assert_eq!(compute_broadcast(ip, mask), Ipv4Addr::new(192, 168, 1, 255));
    }
}

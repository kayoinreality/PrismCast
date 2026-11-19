use if_addrs::IfAddr;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetInterface {
    pub name: String,
    pub address: String,
    pub netmask: String,
    pub broadcast: String,
    pub is_preferred: bool,
}

const CACHE_TTL: Duration = Duration::from_secs(30);

static INTERFACE_CACHE: LazyLock<Mutex<Option<(Instant, Vec<NetInterface>)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Calcula o endereco de broadcast dirigido da sub-rede (`ip | (~mask)`).
pub fn compute_broadcast(ip: Ipv4Addr, mask: Ipv4Addr) -> Ipv4Addr {
    let ip_u32 = u32::from(ip);
    let mask_u32 = u32::from(mask);
    if mask_u32 == 0 {
        return Ipv4Addr::BROADCAST;
    }
    Ipv4Addr::from(ip_u32 | !mask_u32)
}

/// Descobre qual IPv4 local o sistema operacional usa para a rota padrao sem enviar pacotes.
pub fn preferred_local_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

/// Invalida o cache de interfaces de rede (ex: quando o usuario abre o seletor de placas).
pub fn invalidate_cache() {
    if let Ok(mut guard) = INTERFACE_CACHE.lock() {
        *guard = None;
    }
}

/// Lista interfaces IPv4 ativas usando `if_addrs` (Win32 `GetAdaptersAddresses` / `getifaddrs`)
/// com cache de 30 segundos para evitar picos de CPU em polling/broadcasts frequentes.
pub fn list_interfaces() -> Vec<NetInterface> {
    if let Ok(guard) = INTERFACE_CACHE.lock() {
        if let Some((fetched_at, cached)) = guard.as_ref() {
            if fetched_at.elapsed() < CACHE_TTL {
                return cached.clone();
            }
        }
    }

    let list = collect_interfaces();
    if let Ok(mut guard) = INTERFACE_CACHE.lock() {
        *guard = Some((Instant::now(), list.clone()));
    }
    list
}

fn collect_interfaces() -> Vec<NetInterface> {
    let preferred = preferred_local_ipv4();
    let mut out = Vec::new();

    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for iface in addrs {
            if iface.is_loopback() {
                continue;
            }
            if let IfAddr::V4(v4) = iface.addr {
                let ip = v4.ip;
                if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
                    continue;
                }
                let mask = v4.netmask;
                let broadcast = v4.broadcast.unwrap_or_else(|| compute_broadcast(ip, mask));
                let is_preferred = preferred == Some(ip);
                out.push(NetInterface {
                    name: iface.name,
                    address: ip.to_string(),
                    netmask: mask.to_string(),
                    broadcast: broadcast.to_string(),
                    is_preferred,
                });
            }
        }
    }

    if out.is_empty() {
        if let Some(ip) = preferred {
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
    let end_port = start_port.saturating_add(30);
    for p in start_port..=end_port {
        if TcpListener::bind(("0.0.0.0", p)).is_ok() {
            return p;
        }
        if p == u16::MAX {
            break;
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
        assert_eq!(
            compute_broadcast(ip, Ipv4Addr::new(0, 0, 0, 0)),
            Ipv4Addr::BROADCAST
        );
    }

    #[test]
    fn test_find_free_port_no_overflow() {
        let port = find_free_port(u16::MAX - 5);
        assert!(port > 0);
    }

    #[test]
    fn test_list_interfaces_and_cache() {
        invalidate_cache();
        let first = list_interfaces();
        let second = list_interfaces();
        assert_eq!(first.len(), second.len());
        let targets = broadcast_targets(None);
        assert!(targets.contains(&"255.255.255.255".to_string()));
    }
}

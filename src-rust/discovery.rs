use crate::net::broadcast_targets;
use crate::protocol::{
    DiscoveredHost, DiscoveryMessage, SourceKind, ANNOUNCE_INTERVAL_MS, DISCOVERY_PORT,
    HOST_TTL_MS, PROTOCOL_VERSION,
};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

const MAX_DISCOVERED_HOSTS: usize = 64;
const MAX_TEXT_LEN: usize = 128;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn sanitize_text(raw: String, max_chars: usize) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

#[derive(Debug, Clone)]
pub struct AnnounceState {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub requires_pin: bool,
    pub source_name: String,
    pub source_kind: SourceKind,
    pub viewers: usize,
    pub max_viewers: usize,
    pub started_at: u64,
}

impl AnnounceState {
    pub fn to_message(&self) -> DiscoveryMessage {
        DiscoveryMessage::Announce {
            v: PROTOCOL_VERSION,
            id: self.id.clone(),
            name: self.name.clone(),
            ip: self.ip.clone(),
            port: self.port,
            requires_pin: self.requires_pin,
            source_name: self.source_name.clone(),
            source_kind: self.source_kind,
            viewers: self.viewers,
            max_viewers: self.max_viewers,
            started_at: self.started_at,
        }
    }
}

type HostsCallback = Arc<dyn Fn(Vec<DiscoveredHost>) + Send + Sync>;

pub struct DiscoveryEngine {
    socket: Option<Arc<UdpSocket>>,
    announcing: Arc<Mutex<Option<AnnounceState>>>,
    browsing: Arc<Mutex<bool>>,
    preferred_interface: Arc<Mutex<Option<String>>>,
    hosts: Arc<Mutex<HashMap<String, DiscoveredHost>>>,
    on_hosts_cb: Arc<Mutex<Option<HostsCallback>>>,
    announce_task: Option<JoinHandle<()>>,
    recv_task: Option<JoinHandle<()>>,
    sweep_task: Option<JoinHandle<()>>,
    query_task: Option<JoinHandle<()>>,
}

impl Default for DiscoveryEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryEngine {
    pub fn new() -> Self {
        Self {
            socket: None,
            announcing: Arc::new(Mutex::new(None)),
            browsing: Arc::new(Mutex::new(false)),
            preferred_interface: Arc::new(Mutex::new(None)),
            hosts: Arc::new(Mutex::new(HashMap::new())),
            on_hosts_cb: Arc::new(Mutex::new(None)),
            announce_task: None,
            recv_task: None,
            sweep_task: None,
            query_task: None,
        }
    }

    pub fn set_preferred_interface(&self, pref: Option<String>) {
        *self.preferred_interface.lock().unwrap() = pref;
    }

    pub fn get_hosts(&self) -> Vec<DiscoveredHost> {
        snapshot_sorted(&self.hosts.lock().unwrap())
    }

    async fn ensure_socket<F>(&mut self, on_hosts_changed: F) -> Result<Arc<UdpSocket>, String>
    where
        F: Fn(Vec<DiscoveredHost>) + Send + Sync + 'static,
    {
        *self.on_hosts_cb.lock().unwrap() = Some(Arc::new(on_hosts_changed));

        if let Some(sock) = &self.socket {
            return Ok(sock.clone());
        }

        let raw_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| e.to_string())?;
        let _ = raw_sock.set_reuse_address(true);
        let _ = raw_sock.set_broadcast(true);
        let _ = raw_sock.set_nonblocking(true);
        disable_udp_connreset(&raw_sock);

        let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT);
        raw_sock.bind(&bind_addr.into()).map_err(|e| e.to_string())?;

        let std_sock: std::net::UdpSocket = raw_sock.into();
        let udp = Arc::new(UdpSocket::from_std(std_sock).map_err(|e| e.to_string())?);
        self.socket = Some(udp.clone());

        let udp_recv = udp.clone();
        let announcing_ref = self.announcing.clone();
        let browsing_ref = self.browsing.clone();
        let hosts_ref = self.hosts.clone();
        let cb_ref = self.on_hosts_cb.clone();

        self.recv_task = Some(tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match udp_recv.recv_from(&mut buf).await {
                    Ok((len, rinfo)) => {
                        let Ok(msg) = serde_json::from_slice::<DiscoveryMessage>(&buf[..len])
                        else {
                            continue;
                        };
                        match msg {
                            DiscoveryMessage::Query { v } if v == PROTOCOL_VERSION => {
                                let reply =
                                    announcing_ref.lock().unwrap().as_ref().map(|s| s.to_message());
                                if let Some(announce_msg) = reply {
                                    if let Ok(bytes) = serde_json::to_vec(&announce_msg) {
                                        let _ = udp_recv.send_to(&bytes, rinfo).await;
                                    }
                                }
                            }
                            other => {
                                if let Some(snapshot) = process_incoming_message(
                                    other,
                                    rinfo.ip(),
                                    now_ms(),
                                    &announcing_ref,
                                    &browsing_ref,
                                    &hosts_ref,
                                ) {
                                    emit_hosts(&cb_ref, snapshot);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        // Nunca abortar o loop de descoberta por erros transientes de UDP
                        // (ex: WSAECONNRESET 10054, WSAEMSGSIZE 10040, Interrupted).
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::Interrupted
                                | std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        ) || matches!(err.raw_os_error(), Some(10054 | 10040 | 10052))
                        {
                            continue;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }));

        let hosts_sweep = self.hosts.clone();
        let cb_sweep = self.on_hosts_cb.clone();
        self.sweep_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1500));
            loop {
                interval.tick().await;
                let cutoff = now_ms().saturating_sub(HOST_TTL_MS);
                let snapshot = {
                    let mut map = hosts_sweep.lock().unwrap();
                    let before = map.len();
                    map.retain(|_, h| h.last_seen >= cutoff);
                    if map.len() != before {
                        Some(snapshot_sorted(&map))
                    } else {
                        None
                    }
                };
                if let Some(v) = snapshot {
                    emit_hosts(&cb_sweep, v);
                }
            }
        }));

        Ok(udp)
    }

    pub async fn start_announcing<F>(
        &mut self,
        state: AnnounceState,
        on_hosts_changed: F,
    ) -> Result<(), String>
    where
        F: Fn(Vec<DiscoveredHost>) + Send + Sync + 'static,
    {
        let sock = self.ensure_socket(on_hosts_changed).await?;
        let initial_msg = state.to_message();
        *self.announcing.lock().unwrap() = Some(state);

        if let Some(h) = self.announce_task.take() {
            h.abort();
        }

        let pref_now = self.preferred_interface.lock().unwrap().clone();
        send_broadcast(&sock, &initial_msg, pref_now.as_deref()).await;

        let announcing_ref = self.announcing.clone();
        let pref_ref = self.preferred_interface.clone();
        self.announce_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(ANNOUNCE_INTERVAL_MS));
            // O primeiro tick dispara imediatamente; pulamos para nao duplicar o envio inicial.
            interval.tick().await;
            loop {
                interval.tick().await;
                let msg_opt = announcing_ref.lock().unwrap().as_ref().map(|s| s.to_message());
                let Some(msg) = msg_opt else { break };
                let pref = pref_ref.lock().unwrap().clone();
                send_broadcast(&sock, &msg, pref.as_deref()).await;
            }
        }));

        Ok(())
    }

    pub fn patch_announce(
        &self,
        viewers: Option<usize>,
        source_name: Option<String>,
        source_kind: Option<SourceKind>,
    ) {
        if let Some(state) = self.announcing.lock().unwrap().as_mut() {
            if let Some(v) = viewers {
                state.viewers = v;
            }
            if let Some(name) = source_name {
                state.source_name = name;
            }
            if let Some(kind) = source_kind {
                state.source_kind = kind;
            }
        }
    }

    pub async fn stop_announcing(&mut self) {
        if let Some(h) = self.announce_task.take() {
            h.abort();
        }
        let prev = self.announcing.lock().unwrap().take();
        if let (Some(state), Some(sock)) = (prev, &self.socket) {
            let bye = DiscoveryMessage::Bye {
                v: PROTOCOL_VERSION,
                id: state.id,
            };
            let pref = self.preferred_interface.lock().unwrap().clone();
            send_broadcast(sock, &bye, pref.as_deref()).await;
        }
        self.close_socket_if_idle();
    }

    pub async fn start_browsing<F>(&mut self, on_hosts_changed: F) -> Result<(), String>
    where
        F: Fn(Vec<DiscoveredHost>) + Send + Sync + 'static,
    {
        self.hosts.lock().unwrap().clear();
        *self.browsing.lock().unwrap() = true;
        let sock = self.ensure_socket(on_hosts_changed).await?;
        emit_hosts(&self.on_hosts_cb, Vec::new());

        let query = DiscoveryMessage::Query {
            v: PROTOCOL_VERSION,
        };
        let pref = self.preferred_interface.lock().unwrap().clone();
        send_broadcast(&sock, &query, pref.as_deref()).await;

        if let Some(h) = self.query_task.take() {
            h.abort();
        }
        let browsing_ref = self.browsing.clone();
        let pref_ref = self.preferred_interface.clone();
        self.query_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(3000));
            interval.tick().await;
            loop {
                interval.tick().await;
                if !*browsing_ref.lock().unwrap() {
                    break;
                }
                let msg = DiscoveryMessage::Query {
                    v: PROTOCOL_VERSION,
                };
                let pref = pref_ref.lock().unwrap().clone();
                send_broadcast(&sock, &msg, pref.as_deref()).await;
            }
        }));

        Ok(())
    }

    pub async fn stop_browsing(&mut self) {
        *self.browsing.lock().unwrap() = false;
        if let Some(h) = self.query_task.take() {
            h.abort();
        }
        self.hosts.lock().unwrap().clear();
        self.close_socket_if_idle();
    }

    pub async fn refresh(&self) {
        if !*self.browsing.lock().unwrap() {
            return;
        }
        if let Some(sock) = &self.socket {
            let query = DiscoveryMessage::Query {
                v: PROTOCOL_VERSION,
            };
            let pref = self.preferred_interface.lock().unwrap().clone();
            send_broadcast(sock, &query, pref.as_deref()).await;
        }
    }

    pub async fn dispose(&mut self) {
        *self.browsing.lock().unwrap() = false;
        self.stop_announcing().await;
        self.stop_browsing().await;
    }

    fn close_socket_if_idle(&mut self) {
        let has_announce = self.announcing.lock().unwrap().is_some();
        let is_browsing = *self.browsing.lock().unwrap();
        if has_announce || is_browsing {
            return;
        }
        if let Some(h) = self.recv_task.take() {
            h.abort();
        }
        if let Some(h) = self.sweep_task.take() {
            h.abort();
        }
        if let Some(h) = self.query_task.take() {
            h.abort();
        }
        self.socket = None;
    }
}

fn snapshot_sorted(map: &HashMap<String, DiscoveredHost>) -> Vec<DiscoveredHost> {
    let mut v: Vec<DiscoveredHost> = map.values().cloned().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn emit_hosts(cb_ref: &Arc<Mutex<Option<HostsCallback>>>, snapshot: Vec<DiscoveredHost>) {
    let cb = cb_ref.lock().unwrap().clone();
    if let Some(cb) = cb {
        cb(snapshot);
    }
}

fn process_incoming_message(
    msg: DiscoveryMessage,
    sender_ip: IpAddr,
    now: u64,
    announcing_ref: &Arc<Mutex<Option<AnnounceState>>>,
    browsing_ref: &Arc<Mutex<bool>>,
    hosts_ref: &Arc<Mutex<HashMap<String, DiscoveredHost>>>,
) -> Option<Vec<DiscoveredHost>> {
    match msg {
        DiscoveryMessage::Announce {
            v,
            id,
            name,
            ip,
            port,
            requires_pin,
            source_name,
            source_kind,
            viewers,
            max_viewers,
            started_at,
        } if v == PROTOCOL_VERSION => {
            if !*browsing_ref.lock().unwrap() {
                return None;
            }
            if id.trim().is_empty() || id.len() > 64 || port == 0 {
                return None;
            }
            let is_self = announcing_ref
                .lock()
                .unwrap()
                .as_ref()
                .map(|a| a.id == id)
                .unwrap_or(false);
            if is_self {
                return None;
            }

            // Anti-spoofing: so aceita `ip` anunciado se bater com a origem real do pacote;
            // caso contrario (ou se multi-homed), usa o IP real de origem (`sender_ip`).
            let sender_ip_str = sender_ip.to_string();
            let verified_ip = match ip.parse::<IpAddr>() {
                Ok(parsed) if parsed == sender_ip => ip,
                _ => sender_ip_str,
            };

            let host = DiscoveredHost {
                id: id.clone(),
                name: sanitize_text(name, MAX_TEXT_LEN),
                ip: verified_ip,
                port,
                requires_pin,
                source_name: sanitize_text(source_name, MAX_TEXT_LEN),
                source_kind,
                viewers,
                max_viewers,
                started_at,
                last_seen: now,
            };

            let mut map = hosts_ref.lock().unwrap();
            if !map.contains_key(&id) && map.len() >= MAX_DISCOVERED_HOSTS {
                // Remove a entrada mais antiga se atingir o teto, evitando crescimento ilimitado.
                if let Some(oldest_id) = map
                    .iter()
                    .min_by_key(|(_, h)| h.last_seen)
                    .map(|(k, _)| k.clone())
                {
                    map.remove(&oldest_id);
                } else {
                    return None;
                }
            }
            map.insert(id, host);
            Some(snapshot_sorted(&map))
        }
        DiscoveryMessage::Bye { v, id } if v == PROTOCOL_VERSION => {
            let mut map = hosts_ref.lock().unwrap();
            if map.remove(&id).is_some() {
                Some(snapshot_sorted(&map))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn disable_udp_connreset(sock: &Socket) {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{WSAIoctl, SIO_UDP_CONNRESET, SOCKET};

    let mut new_behavior: u32 = 0; // FALSE
    let mut bytes_returned: u32 = 0;
    let raw = sock.as_raw_socket() as usize;
    unsafe {
        let _ = WSAIoctl(
            SOCKET(raw),
            SIO_UDP_CONNRESET,
            Some(&mut new_behavior as *mut u32 as *const _),
            std::mem::size_of::<u32>() as u32,
            None,
            0,
            &mut bytes_returned,
            None,
            None,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn disable_udp_connreset(_sock: &Socket) {}

async fn send_broadcast(
    sock: &UdpSocket,
    msg: &DiscoveryMessage,
    preferred_interface: Option<&str>,
) {
    let Ok(bytes) = serde_json::to_vec(msg) else {
        return;
    };
    for target in broadcast_targets(preferred_interface) {
        if let Ok(addr) = format!("{}:{}", target, DISCOVERY_PORT).parse::<SocketAddr>() {
            let _ = sock.send_to(&bytes, addr).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_announce(id: &str, ip: &str, port: u16) -> DiscoveryMessage {
        DiscoveryMessage::Announce {
            v: PROTOCOL_VERSION,
            id: id.to_string(),
            name: format!("Host-{id}"),
            ip: ip.to_string(),
            port,
            requires_pin: false,
            source_name: "Tela 1".to_string(),
            source_kind: SourceKind::Screen,
            viewers: 0,
            max_viewers: 4,
            started_at: 1000,
        }
    }

    #[test]
    fn test_anti_spoofing_overrides_mismatched_ip() {
        let announcing = Arc::new(Mutex::new(None));
        let browsing = Arc::new(Mutex::new(true));
        let hosts = Arc::new(Mutex::new(HashMap::new()));
        let real_sender = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 55));

        let msg = make_announce("host-1", "10.0.0.99", 41235);
        let list = process_incoming_message(msg, real_sender, 5000, &announcing, &browsing, &hosts)
            .expect("host deve ser aceito");

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ip, "192.168.1.55");
    }

    #[test]
    fn test_rejects_invalid_port_and_self_id() {
        let announcing = Arc::new(Mutex::new(Some(AnnounceState {
            id: "self-id".to_string(),
            name: "Eu".to_string(),
            ip: "192.168.1.10".to_string(),
            port: 41235,
            requires_pin: false,
            source_name: "Tela".to_string(),
            source_kind: SourceKind::Screen,
            viewers: 0,
            max_viewers: 4,
            started_at: 1000,
        })));
        let browsing = Arc::new(Mutex::new(true));
        let hosts = Arc::new(Mutex::new(HashMap::new()));
        let sender = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));

        assert!(process_incoming_message(
            make_announce("self-id", "192.168.1.10", 41235),
            sender,
            5000,
            &announcing,
            &browsing,
            &hosts
        )
        .is_none());

        assert!(process_incoming_message(
            make_announce("other", "192.168.1.10", 0),
            sender,
            5000,
            &announcing,
            &browsing,
            &hosts
        )
        .is_none());
    }

    #[test]
    fn test_hosts_cap_evicts_oldest() {
        let announcing = Arc::new(Mutex::new(None));
        let browsing = Arc::new(Mutex::new(true));
        let hosts = Arc::new(Mutex::new(HashMap::new()));
        let sender = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));

        for i in 0..MAX_DISCOVERED_HOSTS {
            let msg = make_announce(&format!("h-{i}"), "192.168.1.2", 41235);
            process_incoming_message(msg, sender, 1000 + i as u64, &announcing, &browsing, &hosts);
        }
        assert_eq!(hosts.lock().unwrap().len(), MAX_DISCOVERED_HOSTS);

        let extra = make_announce("h-new", "192.168.1.2", 41235);
        process_incoming_message(extra, sender, 9999, &announcing, &browsing, &hosts);
        let map = hosts.lock().unwrap();
        assert_eq!(map.len(), MAX_DISCOVERED_HOSTS);
        assert!(!map.contains_key("h-0"));
        assert!(map.contains_key("h-new"));
    }
}

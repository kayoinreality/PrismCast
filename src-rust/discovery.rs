use crate::net::broadcast_targets;
use crate::protocol::{
    DiscoveredHost, DiscoveryMessage, SourceKind, ANNOUNCE_INTERVAL_MS, DISCOVERY_PORT,
    HOST_TTL_MS, PROTOCOL_VERSION,
};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

pub struct DiscoveryEngine {
    socket: Option<Arc<UdpSocket>>,
    announcing: Arc<Mutex<Option<AnnounceState>>>,
    preferred_interface: Arc<Mutex<Option<String>>>,
    hosts: Arc<Mutex<HashMap<String, DiscoveredHost>>>,
    announce_task: Option<JoinHandle<()>>,
    recv_task: Option<JoinHandle<()>>,
    sweep_task: Option<JoinHandle<()>>,
}

impl DiscoveryEngine {
    pub fn new() -> Self {
        Self {
            socket: None,
            announcing: Arc::new(Mutex::new(None)),
            preferred_interface: Arc::new(Mutex::new(None)),
            hosts: Arc::new(Mutex::new(HashMap::new())),
            announce_task: None,
            recv_task: None,
            sweep_task: None,
        }
    }

    pub fn set_preferred_interface(&self, pref: Option<String>) {
        *self.preferred_interface.lock().unwrap() = pref;
    }

    pub fn get_hosts(&self) -> Vec<DiscoveredHost> {
        let mut list: Vec<DiscoveredHost> = self.hosts.lock().unwrap().values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    async fn ensure_socket<F>(&mut self, on_hosts_changed: F) -> Result<Arc<UdpSocket>, String>
    where
        F: Fn(Vec<DiscoveredHost>) + Send + Sync + 'static,
    {
        if let Some(sock) = &self.socket {
            return Ok(sock.clone());
        }

        let raw_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| e.to_string())?;
        let _ = raw_sock.set_reuse_address(true);
        let _ = raw_sock.set_broadcast(true);
        let _ = raw_sock.set_nonblocking(true);

        let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT);
        raw_sock.bind(&bind_addr.into()).map_err(|e| e.to_string())?;

        let std_sock: std::net::UdpSocket = raw_sock.into();
        let udp = Arc::new(UdpSocket::from_std(std_sock).map_err(|e| e.to_string())?);
        self.socket = Some(udp.clone());

        let udp_recv = udp.clone();
        let announcing_ref = self.announcing.clone();
        let hosts_ref = self.hosts.clone();
        let cb = Arc::new(on_hosts_changed);
        let cb_recv = cb.clone();

        self.recv_task = Some(tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((len, rinfo)) = udp_recv.recv_from(&mut buf).await {
                let Ok(msg) = serde_json::from_slice::<DiscoveryMessage>(&buf[..len]) else {
                    continue;
                };
                match msg {
                    DiscoveryMessage::Query { v } if v == PROTOCOL_VERSION => {
                        let reply = announcing_ref.lock().unwrap().as_ref().map(|s| s.to_message());
                        if let Some(announce_msg) = reply {
                            if let Ok(bytes) = serde_json::to_vec(&announce_msg) {
                                let _ = udp_recv.send_to(&bytes, rinfo).await;
                            }
                        }
                    }
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
                        let is_self = announcing_ref
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|a| a.id == id)
                            .unwrap_or(false);
                        if is_self {
                            continue;
                        }
                        let host = DiscoveredHost {
                            id: id.clone(),
                            name,
                            ip: if ip.is_empty() {
                                rinfo.ip().to_string()
                            } else {
                                ip
                            },
                            port,
                            requires_pin,
                            source_name,
                            source_kind,
                            viewers,
                            max_viewers,
                            started_at,
                            last_seen: now_ms(),
                        };
                        let snapshot = {
                            let mut map = hosts_ref.lock().unwrap();
                            map.insert(id, host);
                            let mut v: Vec<DiscoveredHost> = map.values().cloned().collect();
                            v.sort_by(|a, b| a.name.cmp(&b.name));
                            v
                        };
                        cb_recv(snapshot);
                    }
                    DiscoveryMessage::Bye { v, id } if v == PROTOCOL_VERSION => {
                        let snapshot = {
                            let mut map = hosts_ref.lock().unwrap();
                            if map.remove(&id).is_some() {
                                let mut v: Vec<DiscoveredHost> = map.values().cloned().collect();
                                v.sort_by(|a, b| a.name.cmp(&b.name));
                                Some(v)
                            } else {
                                None
                            }
                        };
                        if let Some(v) = snapshot {
                            cb_recv(v);
                        }
                    }
                    _ => {}
                }
            }
        }));

        let hosts_sweep = self.hosts.clone();
        let cb_sweep = cb;
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
                        let mut v: Vec<DiscoveredHost> = map.values().cloned().collect();
                        v.sort_by(|a, b| a.name.cmp(&b.name));
                        Some(v)
                    } else {
                        None
                    }
                };
                if let Some(v) = snapshot {
                    cb_sweep(v);
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
        *self.announcing.lock().unwrap() = Some(state);

        if let Some(h) = self.announce_task.take() {
            h.abort();
        }

        let announcing_ref = self.announcing.clone();
        let pref_ref = self.preferred_interface.clone();
        self.announce_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(ANNOUNCE_INTERVAL_MS));
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

    pub fn patch_announce(&self, viewers: Option<usize>, source_name: Option<String>, source_kind: Option<SourceKind>) {
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
    }

    pub async fn start_browsing<F>(&mut self, on_hosts_changed: F) -> Result<(), String>
    where
        F: Fn(Vec<DiscoveredHost>) + Send + Sync + 'static,
    {
        self.hosts.lock().unwrap().clear();
        let sock = self.ensure_socket(on_hosts_changed).await?;
        let query = DiscoveryMessage::Query {
            v: PROTOCOL_VERSION,
        };
        let pref = self.preferred_interface.lock().unwrap().clone();
        send_broadcast(&sock, &query, pref.as_deref()).await;
        Ok(())
    }

    pub async fn refresh(&self) {
        if let Some(sock) = &self.socket {
            let query = DiscoveryMessage::Query {
                v: PROTOCOL_VERSION,
            };
            let pref = self.preferred_interface.lock().unwrap().clone();
            send_broadcast(sock, &query, pref.as_deref()).await;
        }
    }
}

async fn send_broadcast(sock: &UdpSocket, msg: &DiscoveryMessage, preferred_interface: Option<&str>) {
    let Ok(bytes) = serde_json::to_vec(msg) else {
        return;
    };
    for target in broadcast_targets(preferred_interface) {
        if let Ok(addr) = format!("{}:{}", target, DISCOVERY_PORT).parse::<SocketAddr>() {
            let _ = sock.send_to(&bytes, addr).await;
        }
    }
}

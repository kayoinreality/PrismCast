use crate::protocol::{
    ChatMessage, ConnectedViewer, HostToViewer, ReactionEvent, RejectReason, SourceKind,
    ViewerToHost, PIN_BLOCK_MS, PIN_MAX_ATTEMPTS, PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

const HEARTBEAT_MS: u64 = 2000;
const HEARTBEAT_TIMEOUT_MS: u64 = 8000;
const HELLO_TIMEOUT_MS: u64 = 5000;
const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024; // 1 MB
const MAX_ATTEMPT_ENTRIES: usize = 1024;
const ATTEMPT_STALE_MS: u64 = 10 * 60 * 1000;
const CHAT_RATE_WINDOW_MS: u64 = 5000;
const CHAT_RATE_MAX: u32 = 8;
const REACTION_RATE_WINDOW_MS: u64 = 3000;
const REACTION_RATE_MAX: u32 = 10;
const FLUSH_TIMEOUT: Duration = Duration::from_millis(250);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn normalize_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(|v4| v4.to_string())
            .unwrap_or_else(|| v6.to_string()),
        IpAddr::V4(v4) => v4.to_string(),
    }
}

/// Compara PINs em tempo constante (evita timing side-channel).
pub fn verify_pin_constant_time(expected: &str, provided: Option<&str>) -> bool {
    let Some(got) = provided else {
        return false;
    };
    let a = expected.as_bytes();
    let b = got.as_bytes();
    let len_eq = (a.len() as u64).ct_eq(&(b.len() as u64));
    let mut diff: u8 = if bool::from(len_eq) { 0 } else { 1 };
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let ba = a.get(i).copied().unwrap_or(0);
        let bb = b.get(i).copied().unwrap_or(0);
        diff |= ba ^ bb;
    }
    bool::from(diff.ct_eq(&0) & len_eq)
}

#[derive(Debug, Clone)]
pub struct HostSessionConfig {
    pub host_name: String,
    pub source_name: String,
    pub source_kind: SourceKind,
    pub pin: Option<String>,
    pub max_viewers: usize,
}

#[derive(Default)]
struct RateBucket {
    window_start: u64,
    count: u32,
}

impl RateBucket {
    fn allow(&mut self, now: u64, window_ms: u64, max_count: u32) -> bool {
        if now.saturating_sub(self.window_start) >= window_ms {
            self.window_start = now;
            self.count = 1;
            return true;
        }
        if self.count >= max_count {
            return false;
        }
        self.count += 1;
        true
    }
}

struct ViewerSlot {
    tx: mpsc::UnboundedSender<Message>,
    cancel: CancellationToken,
    info: ConnectedViewer,
    last_pong: u64,
    chat_rate: RateBucket,
    reaction_rate: RateBucket,
}

#[derive(Default)]
struct AttemptEntry {
    count: u32,
    blocked_until: u64,
    last_seen: u64,
}

pub struct SignalingServer {
    viewers: Arc<Mutex<HashMap<String, ViewerSlot>>>,
    attempts: Arc<Mutex<HashMap<String, AttemptEntry>>>,
    session: Arc<Mutex<Option<HostSessionConfig>>>,
    server_cancel: Option<CancellationToken>,
    accept_task: Option<JoinHandle<()>>,
    heartbeat_task: Option<JoinHandle<()>>,
}

impl Default for SignalingServer {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalingServer {
    pub fn new() -> Self {
        Self {
            viewers: Arc::new(Mutex::new(HashMap::new())),
            attempts: Arc::new(Mutex::new(HashMap::new())),
            session: Arc::new(Mutex::new(None)),
            server_cancel: None,
            accept_task: None,
            heartbeat_task: None,
        }
    }

    pub async fn start<FV, FM>(
        &mut self,
        port: u16,
        config: HostSessionConfig,
        on_viewers: FV,
        on_message: FM,
    ) -> Result<(), String>
    where
        FV: Fn(Vec<ConnectedViewer>) + Send + Sync + 'static,
        FM: Fn(String, ViewerToHost) + Send + Sync + 'static,
    {
        self.stop().await;

        let listener = match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => l,
            Err(e) => {
                *self.session.lock().unwrap() = None;
                return Err(e.to_string());
            }
        };

        *self.session.lock().unwrap() = Some(config);
        let server_cancel = CancellationToken::new();
        self.server_cancel = Some(server_cancel.clone());

        let viewers_ref = self.viewers.clone();
        let attempts_ref = self.attempts.clone();
        let session_ref = self.session.clone();
        let on_viewers = Arc::new(on_viewers);
        let on_message = Arc::new(on_message);

        let viewers_hb = viewers_ref.clone();
        let attempts_hb = attempts_ref.clone();
        let on_viewers_hb = on_viewers.clone();
        let cancel_hb = server_cancel.clone();
        self.heartbeat_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(HEARTBEAT_MS));
            loop {
                tokio::select! {
                    _ = cancel_hb.cancelled() => break,
                    _ = interval.tick() => {}
                }
                let now = now_ms();
                {
                    let mut att = attempts_hb.lock().unwrap();
                    att.retain(|_, e| {
                        e.blocked_until > now || now.saturating_sub(e.last_seen) < ATTEMPT_STALE_MS
                    });
                }

                let mut stale_slots = Vec::new();
                let snapshot = {
                    let mut map = viewers_hb.lock().unwrap();
                    let stale: Vec<String> = map
                        .iter()
                        .filter(|(_, slot)| now.saturating_sub(slot.last_pong) > HEARTBEAT_TIMEOUT_MS)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in stale {
                        if let Some(slot) = map.remove(&id) {
                            stale_slots.push(slot);
                        }
                    }
                    let ping_bytes = now.to_be_bytes().to_vec();
                    for slot in map.values() {
                        let _ = slot.tx.send(Message::Ping(ping_bytes.clone().into()));
                    }
                    if stale_slots.is_empty() {
                        None
                    } else {
                        Some(map.values().map(|s| s.info.clone()).collect::<Vec<_>>())
                    }
                };
                for slot in stale_slots {
                    slot.cancel.cancel();
                }
                if let Some(list) = snapshot {
                    on_viewers_hb(list);
                }
            }
        }));

        let cancel_accept = server_cancel.clone();
        self.accept_task = Some(tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = cancel_accept.cancelled() => break,
                    res = listener.accept() => res,
                };
                let (stream, peer_addr) = match accepted {
                    Ok(pair) => pair,
                    Err(_) => {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        continue;
                    }
                };

                let viewers_conn = viewers_ref.clone();
                let attempts_conn = attempts_ref.clone();
                let session_conn = session_ref.clone();
                let on_viewers_conn = on_viewers.clone();
                let on_message_conn = on_message.clone();
                let conn_parent_cancel = cancel_accept.clone();

                tokio::spawn(async move {
                    let mut ws_cfg = WebSocketConfig::default();
                    ws_cfg.max_message_size = Some(MAX_WS_MESSAGE_BYTES);
                    ws_cfg.max_frame_size = Some(MAX_WS_MESSAGE_BYTES);

                    let Ok(ws_stream) =
                        tokio_tungstenite::accept_async_with_config(stream, Some(ws_cfg)).await
                    else {
                        return;
                    };

                    let (mut ws_write, mut ws_read) = ws_stream.split();
                    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
                    let conn_cancel = conn_parent_cancel.child_token();
                    let write_cancel = conn_cancel.clone();

                    let write_task = tokio::spawn(async move {
                        loop {
                            let msg = tokio::select! {
                                _ = write_cancel.cancelled() => {
                                    // Drena mensagens pendentes (ex: Rejected/Kicked/Ended) antes de encerrar.
                                    while let Ok(pending) = rx.try_recv() {
                                        if ws_write.send(pending).await.is_err() {
                                            break;
                                        }
                                    }
                                    let _ = ws_write.flush().await;
                                    let _ = ws_write.close().await;
                                    break;
                                }
                                item = rx.recv() => match item {
                                    Some(m) => m,
                                    None => break,
                                }
                            };
                            if ws_write.send(msg).await.is_err() {
                                break;
                            }
                        }
                    });

                    let ip_str = normalize_ip(peer_addr.ip());
                    let mut current_viewer_id: Option<String> = None;
                    let hello_deadline =
                        tokio::time::sleep(Duration::from_millis(HELLO_TIMEOUT_MS));
                    tokio::pin!(hello_deadline);

                    loop {
                        let next_item = if current_viewer_id.is_none() {
                            tokio::select! {
                                _ = conn_cancel.cancelled() => break,
                                _ = &mut hello_deadline => break,
                                item = ws_read.next() => item,
                            }
                        } else {
                            tokio::select! {
                                _ = conn_cancel.cancelled() => break,
                                item = ws_read.next() => item,
                            }
                        };

                        let Some(Ok(msg)) = next_item else {
                            break;
                        };

                        match msg {
                            Message::Pong(payload) => {
                                if let Some(ref vid) = current_viewer_id {
                                    let now = now_ms();
                                    let updated_list = {
                                        let mut map = viewers_conn.lock().unwrap();
                                        if let Some(slot) = map.get_mut(vid) {
                                            slot.last_pong = now;
                                            if payload.len() == 8 {
                                                let mut arr = [0u8; 8];
                                                arr.copy_from_slice(&payload);
                                                let sent_at = u64::from_be_bytes(arr);
                                                slot.info.ping_ms =
                                                    Some(now.saturating_sub(sent_at));
                                            }
                                            Some(
                                                map.values()
                                                    .map(|s| s.info.clone())
                                                    .collect::<Vec<_>>(),
                                            )
                                        } else {
                                            None
                                        }
                                    };
                                    if let Some(list) = updated_list {
                                        on_viewers_conn(list);
                                    }
                                }
                            }
                            Message::Text(text) => {
                                let Ok(parsed) = serde_json::from_str::<ViewerToHost>(&text) else {
                                    continue;
                                };

                                if current_viewer_id.is_none() {
                                    let ViewerToHost::Hello { v, name, pin } = parsed else {
                                        continue;
                                    };
                                    let sess_opt = session_conn.lock().unwrap().clone();
                                    let Some(sess) = sess_opt else {
                                        send_json(
                                            &tx,
                                            &HostToViewer::Rejected {
                                                reason: RejectReason::SemTransmissao,
                                                retry_after_ms: None,
                                            },
                                        );
                                        break;
                                    };
                                    if v != PROTOCOL_VERSION {
                                        send_json(
                                            &tx,
                                            &HostToViewer::Rejected {
                                                reason: RejectReason::VersaoIncompativel,
                                                retry_after_ms: None,
                                            },
                                        );
                                        break;
                                    }

                                    // Verifica bloqueio e valida PIN em tempo constante
                                    let reject_reason = check_pin_attempt(
                                        &attempts_conn,
                                        &ip_str,
                                        sess.pin.as_deref(),
                                        pin.as_deref(),
                                        now_ms(),
                                    );
                                    if let Some((reason, retry_after_ms)) = reject_reason {
                                        send_json(
                                            &tx,
                                            &HostToViewer::Rejected {
                                                reason,
                                                retry_after_ms,
                                            },
                                        );
                                        break;
                                    }

                                    let vid = Uuid::new_v4().to_string();
                                    let clean_name: String = name
                                        .chars()
                                        .filter(|c| !c.is_control())
                                        .take(40)
                                        .collect();
                                    let viewer_name = if clean_name.trim().is_empty() {
                                        "Espectador".to_string()
                                    } else {
                                        clean_name
                                    };

                                    let list_opt = {
                                        let mut map = viewers_conn.lock().unwrap();
                                        if map.len() >= sess.max_viewers {
                                            None
                                        } else {
                                            let now = now_ms();
                                            let info = ConnectedViewer {
                                                id: vid.clone(),
                                                name: viewer_name,
                                                address: ip_str.clone(),
                                                joined_at: now,
                                                ping_ms: None,
                                            };
                                            map.insert(
                                                vid.clone(),
                                                ViewerSlot {
                                                    tx: tx.clone(),
                                                    cancel: conn_cancel.clone(),
                                                    info,
                                                    last_pong: now,
                                                    chat_rate: RateBucket::default(),
                                                    reaction_rate: RateBucket::default(),
                                                },
                                            );
                                            Some(
                                                map.values()
                                                    .map(|s| s.info.clone())
                                                    .collect::<Vec<_>>(),
                                            )
                                        }
                                    };

                                    let Some(list) = list_opt else {
                                        send_json(
                                            &tx,
                                            &HostToViewer::Rejected {
                                                reason: RejectReason::SalaCheia,
                                                retry_after_ms: None,
                                            },
                                        );
                                        break;
                                    };

                                    current_viewer_id = Some(vid.clone());
                                    send_json(
                                        &tx,
                                        &HostToViewer::Welcome {
                                            viewer_id: vid,
                                            host_name: sess.host_name,
                                            source_name: sess.source_name,
                                            source_kind: sess.source_kind,
                                        },
                                    );
                                    on_viewers_conn(list);
                                    continue;
                                }

                                let vid = current_viewer_id.clone().unwrap();
                                if !viewers_conn.lock().unwrap().contains_key(&vid) {
                                    break;
                                }

                                match &parsed {
                                    ViewerToHost::Bye => break,
                                    ViewerToHost::Chat { text } => {
                                        let now = now_ms();
                                        let sender_opt = {
                                            let mut map = viewers_conn.lock().unwrap();
                                            match map.get_mut(&vid) {
                                                Some(slot) => {
                                                    if slot.chat_rate.allow(
                                                        now,
                                                        CHAT_RATE_WINDOW_MS,
                                                        CHAT_RATE_MAX,
                                                    ) {
                                                        Some(slot.info.name.clone())
                                                    } else {
                                                        None
                                                    }
                                                }
                                                None => None,
                                            }
                                        };
                                        let Some(sender_name) = sender_opt else {
                                            continue;
                                        };
                                        let clean: String = text
                                            .trim()
                                            .chars()
                                            .filter(|c| !c.is_control())
                                            .take(400)
                                            .collect();
                                        if clean.is_empty() {
                                            continue;
                                        }
                                        let chat_msg = ChatMessage {
                                            id: Uuid::new_v4().to_string(),
                                            senderId: vid.clone(),
                                            senderName: sender_name,
                                            role: "viewer".to_string(),
                                            text: clean,
                                            timestamp: now,
                                        };
                                        broadcast_to_all(
                                            &viewers_conn,
                                            &HostToViewer::Chat { message: chat_msg },
                                        );
                                    }
                                    ViewerToHost::Reaction { emoji } => {
                                        let now = now_ms();
                                        let sender_opt = {
                                            let mut map = viewers_conn.lock().unwrap();
                                            match map.get_mut(&vid) {
                                                Some(slot) => {
                                                    if slot.reaction_rate.allow(
                                                        now,
                                                        REACTION_RATE_WINDOW_MS,
                                                        REACTION_RATE_MAX,
                                                    ) {
                                                        Some(slot.info.name.clone())
                                                    } else {
                                                        None
                                                    }
                                                }
                                                None => None,
                                            }
                                        };
                                        let Some(sender_name) = sender_opt else {
                                            continue;
                                        };
                                        let clean_emoji: String = emoji
                                            .chars()
                                            .filter(|c| !c.is_control())
                                            .take(8)
                                            .collect();
                                        if clean_emoji.is_empty() {
                                            continue;
                                        }
                                        let reaction = ReactionEvent {
                                            id: Uuid::new_v4().to_string(),
                                            senderName: sender_name,
                                            emoji: clean_emoji,
                                            timestamp: now,
                                        };
                                        broadcast_to_all(
                                            &viewers_conn,
                                            &HostToViewer::Reaction { reaction },
                                        );
                                    }
                                    _ => {}
                                }
                                on_message_conn(vid, parsed);
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }

                    // Garante flush de mensagens finais (ex: Rejected/Kicked) e aguarda o close
                    // handshake do peer brevemente para evitar TCP RST no Winsock (Windows) antes
                    // do cliente ler o frame final.
                    conn_cancel.cancel();
                    if tokio::time::timeout(FLUSH_TIMEOUT, write_task).await.is_ok() {
                        let _ = tokio::time::timeout(Duration::from_millis(150), async {
                            while let Some(Ok(frame)) = ws_read.next().await {
                                if matches!(frame, Message::Close(_)) {
                                    break;
                                }
                            }
                        })
                        .await;
                    }

                    if let Some(vid) = current_viewer_id {
                        let removed_list = {
                            let mut map = viewers_conn.lock().unwrap();
                            if map.remove(&vid).is_some() {
                                Some(map.values().map(|s| s.info.clone()).collect::<Vec<_>>())
                            } else {
                                None
                            }
                        };
                        if let Some(list) = removed_list {
                            on_viewers_conn(list);
                        }
                    }
                });
            }
        }));

        Ok(())
    }

    pub fn update_source(&self, source_name: String, source_kind: SourceKind) {
        if let Some(sess) = self.session.lock().unwrap().as_mut() {
            sess.source_name = source_name.clone();
            sess.source_kind = source_kind;
        }
        self.broadcast(&HostToViewer::SourceChanged {
            source_name,
            source_kind,
        });
    }

    pub fn send(&self, viewer_id: &str, msg: &HostToViewer) {
        if let Some(slot) = self.viewers.lock().unwrap().get(viewer_id) {
            send_json(&slot.tx, msg);
        }
    }

    pub fn broadcast(&self, msg: &HostToViewer) {
        broadcast_to_all(&self.viewers, msg);
    }

    /// Expulsa um viewer imediatamente, envia `Kicked`, cancela sua conexao e retorna a lista atualizada.
    pub fn kick(&self, viewer_id: &str) -> Option<Vec<ConnectedViewer>> {
        let (removed, remaining) = {
            let mut map = self.viewers.lock().unwrap();
            let slot = map.remove(viewer_id)?;
            let remaining = map.values().map(|s| s.info.clone()).collect::<Vec<_>>();
            (slot, remaining)
        };
        send_json(&removed.tx, &HostToViewer::Kicked);
        removed.cancel.cancel();
        Some(remaining)
    }

    pub async fn stop(&mut self) {
        if let Some(h) = self.heartbeat_task.take() {
            h.abort();
        }
        if let Some(h) = self.accept_task.take() {
            h.abort();
        }
        self.broadcast(&HostToViewer::Ended);

        let slots: Vec<ViewerSlot> = {
            let mut map = self.viewers.lock().unwrap();
            map.drain().map(|(_, s)| s).collect()
        };
        if !slots.is_empty() {
            tokio::time::sleep(FLUSH_TIMEOUT).await;
            for slot in slots {
                slot.cancel.cancel();
            }
        }
        if let Some(cancel) = self.server_cancel.take() {
            cancel.cancel();
        }
        self.attempts.lock().unwrap().clear();
        *self.session.lock().unwrap() = None;
    }
}

fn check_pin_attempt(
    attempts_ref: &Arc<Mutex<HashMap<String, AttemptEntry>>>,
    ip_str: &str,
    expected_pin: Option<&str>,
    provided_pin: Option<&str>,
    now: u64,
) -> Option<(RejectReason, Option<u64>)> {
    let mut att = attempts_ref.lock().unwrap();
    if att.len() >= MAX_ATTEMPT_ENTRIES && !att.contains_key(ip_str) {
        att.retain(|_, e| {
            e.blocked_until > now || now.saturating_sub(e.last_seen) < ATTEMPT_STALE_MS
        });
        if att.len() >= MAX_ATTEMPT_ENTRIES {
            if let Some(oldest) = att
                .iter()
                .min_by_key(|(_, e)| e.last_seen)
                .map(|(k, _)| k.clone())
            {
                att.remove(&oldest);
            }
        }
    }

    let entry = att.entry(ip_str.to_string()).or_default();
    entry.last_seen = now;
    if entry.blocked_until > now {
        return Some((RejectReason::Bloqueado, Some(entry.blocked_until - now)));
    }

    if let Some(expected) = expected_pin {
        if !verify_pin_constant_time(expected, provided_pin) {
            entry.count += 1;
            if entry.count >= PIN_MAX_ATTEMPTS {
                entry.blocked_until = now + PIN_BLOCK_MS;
                entry.count = 0;
                return Some((RejectReason::Bloqueado, Some(PIN_BLOCK_MS)));
            }
            return Some((RejectReason::PinInvalido, None));
        }
    }

    att.remove(ip_str);
    None
}

fn send_json(tx: &mpsc::UnboundedSender<Message>, msg: &HostToViewer) {
    if let Ok(raw) = serde_json::to_string(msg) {
        let _ = tx.send(Message::Text(raw.into()));
    }
}

fn broadcast_to_all(viewers: &Arc<Mutex<HashMap<String, ViewerSlot>>>, msg: &HostToViewer) {
    let Ok(raw) = serde_json::to_string(msg) else {
        return;
    };
    let map = viewers.lock().unwrap();
    for slot in map.values() {
        let _ = slot.tx.send(Message::Text(raw.clone().into()));
    }
}

// ------------------------------------------------------------------ Cliente WebSocket LAN (Viewer)

#[derive(Debug, Clone)]
pub enum ViewerClosedReason {
    Kicked,
    Ended,
    Rede,
    Local,
}

#[derive(Debug, Clone)]
pub struct ViewerClosedInfo {
    pub reason: ViewerClosedReason,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ViewerWelcome {
    pub viewer_id: String,
    pub host_name: String,
    pub source_name: String,
    pub source_kind: SourceKind,
}

pub enum ViewerConnectOutcome {
    Connected(ViewerWelcome),
    Rejected {
        reason: RejectReason,
        message: String,
        retry_after_ms: Option<u64>,
    },
    NetworkError(String),
}

pub fn validate_signaling_url(ip: &str, port: u16) -> Result<Url, String> {
    if port == 0 {
        return Err("Porta inválida.".to_string());
    }
    let parsed_ip: IpAddr = ip
        .trim()
        .parse()
        .map_err(|_| "Endereço IP inválido.".to_string())?;
    let sock = SocketAddr::new(parsed_ip, port);
    Url::parse(&format!("ws://{sock}")).map_err(|e| e.to_string())
}

pub struct SignalingClient {
    tx: Option<mpsc::UnboundedSender<Message>>,
    cancel: Option<CancellationToken>,
}

impl Default for SignalingClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalingClient {
    pub fn new() -> Self {
        Self {
            tx: None,
            cancel: None,
        }
    }

    pub async fn connect<FM, FC>(
        &mut self,
        ip: &str,
        port: u16,
        name: String,
        pin: Option<String>,
        on_message: FM,
        on_closed: FC,
    ) -> ViewerConnectOutcome
    where
        FM: Fn(HostToViewer) + Send + Sync + 'static,
        FC: Fn(ViewerClosedInfo) + Send + Sync + 'static,
    {
        self.disconnect();

        let url = match validate_signaling_url(ip, port) {
            Ok(u) => u,
            Err(err) => return ViewerConnectOutcome::NetworkError(err),
        };

        let mut ws_cfg = WebSocketConfig::default();
        ws_cfg.max_message_size = Some(MAX_WS_MESSAGE_BYTES);
        ws_cfg.max_frame_size = Some(MAX_WS_MESSAGE_BYTES);

        let connect_res = tokio::time::timeout(
            Duration::from_secs(6),
            tokio_tungstenite::connect_async_with_config(url.as_str(), Some(ws_cfg), false),
        )
        .await;

        let ws_stream = match connect_res {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(_)) | Err(_) => {
                return ViewerConnectOutcome::NetworkError(
                    "Não foi possível conectar. O host ainda está transmitindo?".to_string(),
                )
            }
        };

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        let cancel = CancellationToken::new();
        let write_cancel = cancel.clone();

        let write_task = tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    _ = write_cancel.cancelled() => break,
                    item = rx.recv() => match item {
                        Some(m) => m,
                        None => break,
                    }
                };
                if ws_write.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let hello = ViewerToHost::Hello {
            v: PROTOCOL_VERSION,
            name,
            pin,
        };
        if let Ok(raw) = serde_json::to_string(&hello) {
            let _ = tx.send(Message::Text(raw.into()));
        }

        // Aguarda Welcome ou Rejected dentro de 8 segundos
        let handshake = tokio::time::timeout(Duration::from_secs(8), async {
            while let Some(Ok(msg)) = ws_read.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(parsed) = serde_json::from_str::<HostToViewer>(&text) {
                        return Some(parsed);
                    }
                }
            }
            None
        })
        .await;

        let welcome = match handshake {
            Ok(Some(HostToViewer::Welcome {
                viewer_id,
                host_name,
                source_name,
                source_kind,
            })) => ViewerWelcome {
                viewer_id,
                host_name,
                source_name,
                source_kind,
            },
            Ok(Some(HostToViewer::Rejected {
                reason,
                retry_after_ms,
            })) => {
                cancel.cancel();
                write_task.abort();
                return ViewerConnectOutcome::Rejected {
                    reason,
                    message: reason.message().to_string(),
                    retry_after_ms,
                };
            }
            _ => {
                cancel.cancel();
                write_task.abort();
                return ViewerConnectOutcome::NetworkError(
                    "O host não respondeu a tempo.".to_string(),
                );
            }
        };

        self.tx = Some(tx);
        self.cancel = Some(cancel.clone());

        let read_cancel = cancel.clone();
        tokio::spawn(async move {
            let mut closed_info: Option<ViewerClosedInfo> = None;
            loop {
                let item = tokio::select! {
                    _ = read_cancel.cancelled() => {
                        closed_info = Some(ViewerClosedInfo {
                            reason: ViewerClosedReason::Local,
                            message: "Desconectado.".to_string(),
                        });
                        break;
                    }
                    msg = ws_read.next() => msg,
                };

                let Some(Ok(msg)) = item else {
                    break;
                };
                match msg {
                    Message::Text(text) => {
                        if let Ok(parsed) = serde_json::from_str::<HostToViewer>(&text) {
                            match &parsed {
                                HostToViewer::Kicked => {
                                    closed_info = Some(ViewerClosedInfo {
                                        reason: ViewerClosedReason::Kicked,
                                        message: "O host removeu você da transmissão.".to_string(),
                                    });
                                }
                                HostToViewer::Ended => {
                                    closed_info = Some(ViewerClosedInfo {
                                        reason: ViewerClosedReason::Ended,
                                        message: "O host encerrou a transmissão.".to_string(),
                                    });
                                }
                                _ => {}
                            }
                            on_message(parsed);
                            if closed_info.is_some() {
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }

            let was_local = matches!(
                closed_info.as_ref().map(|c| &c.reason),
                Some(ViewerClosedReason::Local)
            );
            read_cancel.cancel();
            write_task.abort();
            if !was_local {
                on_closed(closed_info.unwrap_or(ViewerClosedInfo {
                    reason: ViewerClosedReason::Rede,
                    message: "A conexão com o host caiu.".to_string(),
                }));
            }
        });

        ViewerConnectOutcome::Connected(welcome)
    }

    pub fn send(&self, msg: &ViewerToHost) {
        if let Some(tx) = &self.tx {
            if let Ok(raw) = serde_json::to_string(msg) {
                let _ = tx.send(Message::Text(raw.into()));
            }
        }
    }

    pub fn disconnect(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Ok(raw) = serde_json::to_string(&ViewerToHost::Bye) {
                let _ = tx.send(Message::Text(raw.into()));
            }
        }
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_constant_time() {
        assert!(verify_pin_constant_time("123456", Some("123456")));
        assert!(!verify_pin_constant_time("123456", Some("123457")));
        assert!(!verify_pin_constant_time("123456", Some("12345")));
        assert!(!verify_pin_constant_time("123456", None));
    }

    #[test]
    fn test_pin_rate_limit_blocks_after_max_attempts() {
        let attempts = Arc::new(Mutex::new(HashMap::new()));
        let mut t = 1000u64;
        for _ in 0..(PIN_MAX_ATTEMPTS - 1) {
            let res = check_pin_attempt(&attempts, "192.168.1.9", Some("654321"), Some("000000"), t);
            assert_eq!(res, Some((RejectReason::PinInvalido, None)));
            t += 10;
        }
        let blocked = check_pin_attempt(&attempts, "192.168.1.9", Some("654321"), Some("000000"), t);
        assert_eq!(blocked, Some((RejectReason::Bloqueado, Some(PIN_BLOCK_MS))));

        // Mesmo acertando o PIN durante bloqueio, continua bloqueado
        let still_blocked =
            check_pin_attempt(&attempts, "192.168.1.9", Some("654321"), Some("654321"), t + 1000);
        assert!(matches!(still_blocked, Some((RejectReason::Bloqueado, Some(_)))));

        // Apos expirar o bloqueio, PIN correto limpa entrada
        let ok = check_pin_attempt(
            &attempts,
            "192.168.1.9",
            Some("654321"),
            Some("654321"),
            t + PIN_BLOCK_MS + 1,
        );
        assert_eq!(ok, None);
    }

    #[test]
    fn test_rate_bucket_limits_spam() {
        let mut bucket = RateBucket::default();
        let now = 5000;
        for _ in 0..CHAT_RATE_MAX {
            assert!(bucket.allow(now, CHAT_RATE_WINDOW_MS, CHAT_RATE_MAX));
        }
        assert!(!bucket.allow(now + 100, CHAT_RATE_WINDOW_MS, CHAT_RATE_MAX));
        assert!(bucket.allow(now + CHAT_RATE_WINDOW_MS + 1, CHAT_RATE_WINDOW_MS, CHAT_RATE_MAX));
    }

    #[test]
    fn test_validate_signaling_url() {
        assert!(validate_signaling_url("192.168.1.20", 41235).is_ok());
        assert!(validate_signaling_url("::1", 41235).is_ok());
        assert!(validate_signaling_url("not-an-ip", 41235).is_err());
        assert!(validate_signaling_url("192.168.1.20", 0).is_err());
    }

    #[tokio::test]
    async fn test_rtt_kick_and_real_server_handshake() {
        let mut server = SignalingServer::new();
        let port = crate::net::find_free_port(45100);
        let events = Arc::new(Mutex::new(Vec::<usize>::new()));
        let events_cb = events.clone();

        server
            .start(
                port,
                HostSessionConfig {
                    host_name: "Host".to_string(),
                    source_name: "Jogo".to_string(),
                    source_kind: SourceKind::Window,
                    pin: Some("998877".to_string()),
                    max_viewers: 2,
                },
                move |viewers| {
                    events_cb.lock().unwrap().push(viewers.len());
                },
                |_vid, _msg| {},
            )
            .await
            .expect("server start");

        let mut client = SignalingClient::new();
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<ViewerClosedReason>();
        let closed_tx = Arc::new(Mutex::new(Some(closed_tx)));

        let outcome = client
            .connect(
                "127.0.0.1",
                port,
                "Amigo".to_string(),
                Some("998877".to_string()),
                |_msg| {},
                move |info| {
                    if let Some(tx) = closed_tx.lock().unwrap().take() {
                        let _ = tx.send(info.reason);
                    }
                },
            )
            .await;

        let viewer_id = match outcome {
            ViewerConnectOutcome::Connected(w) => w.viewer_id,
            _ => panic!("esperava conectar com PIN correto"),
        };

        let remaining = server.kick(&viewer_id).expect("viewer deveria existir");
        assert!(remaining.is_empty());

        let reason = tokio::time::timeout(Duration::from_secs(5), closed_rx)
            .await
            .expect("timeout aguardando callback on_closed após kick")
            .expect("canal on_closed encerrado sem motivo");
        assert!(
            matches!(reason, ViewerClosedReason::Kicked),
            "esperava ViewerClosedReason::Kicked, recebeu {reason:?}"
        );

        server.stop().await;
    }
}

use crate::protocol::{
    ChatMessage, ConnectedViewer, HostToViewer, ReactionEvent, RejectReason, SourceKind,
    ViewerToHost, PIN_BLOCK_MS, PIN_MAX_ATTEMPTS, PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone)]
pub struct HostSessionConfig {
    pub host_name: String,
    pub source_name: String,
    pub source_kind: SourceKind,
    pub pin: Option<String>,
    pub max_viewers: usize,
}

struct ViewerSlot {
    tx: mpsc::UnboundedSender<Message>,
    info: ConnectedViewer,
    last_pong: u64,
}

#[derive(Default)]
struct AttemptEntry {
    count: u32,
    blocked_until: u64,
}

pub struct SignalingServer {
    viewers: Arc<Mutex<HashMap<String, ViewerSlot>>>,
    attempts: Arc<Mutex<HashMap<String, AttemptEntry>>>,
    session: Arc<Mutex<Option<HostSessionConfig>>>,
    accept_task: Option<JoinHandle<()>>,
    heartbeat_task: Option<JoinHandle<()>>,
}

impl SignalingServer {
    pub fn new() -> Self {
        Self {
            viewers: Arc::new(Mutex::new(HashMap::new())),
            attempts: Arc::new(Mutex::new(HashMap::new())),
            session: Arc::new(Mutex::new(None)),
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
        *self.session.lock().unwrap() = Some(config);

        let listener = TcpListener::bind(("0.0.0.0", port))
            .await
            .map_err(|e| e.to_string())?;

        let viewers_ref = self.viewers.clone();
        let attempts_ref = self.attempts.clone();
        let session_ref = self.session.clone();
        let on_viewers = Arc::new(on_viewers);
        let on_message = Arc::new(on_message);

        let viewers_hb = viewers_ref.clone();
        let on_viewers_hb = on_viewers.clone();
        self.heartbeat_task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(2000));
            loop {
                interval.tick().await;
                let now = now_ms();
                let mut changed = false;
                let snapshot = {
                    let mut map = viewers_hb.lock().unwrap();
                    let stale: Vec<String> = map
                        .iter()
                        .filter(|(_, slot)| now.saturating_sub(slot.last_pong) > 8000)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in stale {
                        map.remove(&id);
                        changed = true;
                    }
                    for slot in map.values() {
                        let _ = slot.tx.send(Message::Ping(vec![].into()));
                    }
                    if changed {
                        Some(map.values().map(|s| s.info.clone()).collect::<Vec<_>>())
                    } else {
                        None
                    }
                };
                if let Some(list) = snapshot {
                    on_viewers_hb(list);
                }
            }
        }));

        self.accept_task = Some(tokio::spawn(async move {
            while let Ok((stream, peer_addr)) = listener.accept().await {
                let viewers_conn = viewers_ref.clone();
                let attempts_conn = attempts_ref.clone();
                let session_conn = session_ref.clone();
                let on_viewers_conn = on_viewers.clone();
                let on_message_conn = on_message.clone();

                tokio::spawn(async move {
                    let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    let (mut ws_write, mut ws_read) = ws_stream.split();
                    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

                    let write_task = tokio::spawn(async move {
                        while let Some(msg) = rx.recv().await {
                            if ws_write.send(msg).await.is_err() {
                                break;
                            }
                        }
                    });

                    let ip_str = peer_addr.ip().to_string();
                    let mut current_viewer_id: Option<String> = None;

                    while let Some(Ok(msg)) = ws_read.next().await {
                        match msg {
                            Message::Pong(_) => {
                                if let Some(ref vid) = current_viewer_id {
                                    if let Some(slot) = viewers_conn.lock().unwrap().get_mut(vid) {
                                        let rtt = now_ms().saturating_sub(slot.last_pong).min(500);
                                        slot.last_pong = now_ms();
                                        slot.info.ping_ms = Some(rtt);
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
                                        send_json(&tx, &HostToViewer::Rejected {
                                            reason: RejectReason::SemTransmissao,
                                            retry_after_ms: None,
                                        });
                                        break;
                                    };
                                    if v != PROTOCOL_VERSION {
                                        send_json(&tx, &HostToViewer::Rejected {
                                            reason: RejectReason::VersaoIncompativel,
                                            retry_after_ms: None,
                                        });
                                        break;
                                    }

                                    // Verifica bloqueio por tentativas de PIN
                                    {
                                        let mut att = attempts_conn.lock().unwrap();
                                        let entry = att.entry(ip_str.clone()).or_default();
                                        let now = now_ms();
                                        if entry.blocked_until > now {
                                            send_json(&tx, &HostToViewer::Rejected {
                                                reason: RejectReason::Bloqueado,
                                                retry_after_ms: Some(entry.blocked_until - now),
                                            });
                                            break;
                                        }
                                        if let Some(ref expected_pin) = sess.pin {
                                            if pin.as_deref() != Some(expected_pin.as_str()) {
                                                entry.count += 1;
                                                if entry.count >= PIN_MAX_ATTEMPTS {
                                                    entry.blocked_until = now + PIN_BLOCK_MS;
                                                    entry.count = 0;
                                                    send_json(&tx, &HostToViewer::Rejected {
                                                        reason: RejectReason::Bloqueado,
                                                        retry_after_ms: Some(PIN_BLOCK_MS),
                                                    });
                                                } else {
                                                    send_json(&tx, &HostToViewer::Rejected {
                                                        reason: RejectReason::PinInvalido,
                                                        retry_after_ms: None,
                                                    });
                                                }
                                                break;
                                            }
                                        }
                                        att.remove(&ip_str);
                                    }

                                    let vid = Uuid::new_v4().to_string();
                                    let clean_name: String = name.chars().take(40).collect();
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
                                            let info = ConnectedViewer {
                                                id: vid.clone(),
                                                name: viewer_name,
                                                address: ip_str.clone(),
                                                joined_at: now_ms(),
                                                ping_ms: None,
                                            };
                                            map.insert(
                                                vid.clone(),
                                                ViewerSlot {
                                                    tx: tx.clone(),
                                                    info,
                                                    last_pong: now_ms(),
                                                },
                                            );
                                            Some(map.values().map(|s| s.info.clone()).collect::<Vec<_>>())
                                        }
                                    };

                                    let Some(list) = list_opt else {
                                        send_json(&tx, &HostToViewer::Rejected {
                                            reason: RejectReason::SalaCheia,
                                            retry_after_ms: None,
                                        });
                                        break;
                                    };

                                    current_viewer_id = Some(vid.clone());
                                    send_json(&tx, &HostToViewer::Welcome {
                                        viewer_id: vid,
                                        host_name: sess.host_name,
                                        source_name: sess.source_name,
                                        source_kind: sess.source_kind,
                                    });
                                    on_viewers_conn(list);
                                    continue;
                                }

                                let vid = current_viewer_id.clone().unwrap();
                                match &parsed {
                                    ViewerToHost::Bye => break,
                                    ViewerToHost::Chat { text } => {
                                        let sender_name = viewers_conn
                                            .lock()
                                            .unwrap()
                                            .get(&vid)
                                            .map(|s| s.info.name.clone())
                                            .unwrap_or_else(|| "Espectador".to_string());
                                        let chat_msg = ChatMessage {
                                            id: Uuid::new_v4().to_string(),
                                            senderId: vid.clone(),
                                            senderName: sender_name,
                                            role: "viewer".to_string(),
                                            text: text.chars().take(400).collect(),
                                            timestamp: now_ms(),
                                        };
                                        broadcast_to_all(&viewers_conn, &HostToViewer::Chat { message: chat_msg });
                                    }
                                    ViewerToHost::Reaction { emoji } => {
                                        let sender_name = viewers_conn
                                            .lock()
                                            .unwrap()
                                            .get(&vid)
                                            .map(|s| s.info.name.clone())
                                            .unwrap_or_else(|| "Espectador".to_string());
                                        let reaction = ReactionEvent {
                                            id: Uuid::new_v4().to_string(),
                                            senderName: sender_name,
                                            emoji: emoji.chars().take(8).collect(),
                                            timestamp: now_ms(),
                                        };
                                        broadcast_to_all(&viewers_conn, &HostToViewer::Reaction { reaction });
                                    }
                                    _ => {}
                                }
                                on_message_conn(vid, parsed);
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }

                    write_task.abort();
                    if let Some(vid) = current_viewer_id {
                        let list = {
                            let mut map = viewers_conn.lock().unwrap();
                            map.remove(&vid);
                            map.values().map(|s| s.info.clone()).collect::<Vec<_>>()
                        };
                        on_viewers_conn(list);
                    }
                });
            }
        }));

        Ok(())
    }

    pub fn send(&self, viewer_id: &str, msg: &HostToViewer) {
        if let Some(slot) = self.viewers.lock().unwrap().get(viewer_id) {
            send_json(&slot.tx, msg);
        }
    }

    pub fn broadcast(&self, msg: &HostToViewer) {
        broadcast_to_all(&self.viewers, msg);
    }

    pub fn kick(&self, viewer_id: &str) {
        if let Some(slot) = self.viewers.lock().unwrap().remove(viewer_id) {
            send_json(&slot.tx, &HostToViewer::Kicked);
        }
    }

    pub async fn stop(&mut self) {
        if let Some(h) = self.heartbeat_task.take() {
            h.abort();
        }
        if let Some(h) = self.accept_task.take() {
            h.abort();
        }
        self.broadcast(&HostToViewer::Ended);
        self.viewers.lock().unwrap().clear();
        self.attempts.lock().unwrap().clear();
        *self.session.lock().unwrap() = None;
    }
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

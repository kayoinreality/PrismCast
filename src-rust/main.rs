use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use prismcast_core::{
    discovery::{AnnounceState, DiscoveryEngine},
    firewall,
    hotkeys::spawn_global_hotkey_listener,
    multistream::MultistreamEngine,
    net,
    protocol::{
        CanvasOrientation, HostToViewer, MultistreamTarget, SourceKind, MAX_VIEWERS, RUST_RPC_PORT,
        SIGNALING_PORT,
    },
    recorder::RecorderEngine,
    signaling::{HostSessionConfig, SignalingServer},
    store::SettingsStore,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcEvent {
    event: String,
    payload: Value,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn generate_pin() -> String {
    let n: u32 = rand::thread_rng().gen_range(0..1_000_000);
    format!("{:06}", n)
}

#[tokio::main]
async fn main() {
    env_logger::init();
    println!("====================================================");
    println!(" PrismCast Studio v2.0.0 — Rust Native Engine");
    println!(" RPC / Event Bridge listening on 127.0.0.1:{}", RUST_RPC_PORT);
    println!("====================================================");

    let store = Arc::new(SettingsStore::new());
    let discovery = Arc::new(AsyncMutex::new(DiscoveryEngine::new()));
    let signaling = Arc::new(AsyncMutex::new(SignalingServer::new()));
    let multistream = Arc::new(Mutex::new(MultistreamEngine::new()));
    let recorder = Arc::new(Mutex::new(RecorderEngine::new()));

    let (evt_tx, _) = broadcast::channel::<String>(256);

    // Atalhos globais nativos (Ctrl+Shift+M / D / P / S / V)
    let evt_hotkeys = evt_tx.clone();
    let _hotkey_handle = spawn_global_hotkey_listener(move |action| {
        let msg = serde_json::to_string(&RpcEvent {
            event: "hotkey:triggered".to_string(),
            payload: json!({ "action": action.as_str() }),
        });
        if let Ok(raw) = msg {
            let _ = evt_hotkeys.send(raw);
        }
    });

    let addr = format!("127.0.0.1:{}", RUST_RPC_PORT);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[prismcast-rust] Porta {} ja em uso ou indisponivel: {}", addr, e);
            return;
        }
    };

    while let Ok((stream, _)) = listener.accept().await {
        let store = store.clone();
        let discovery = discovery.clone();
        let signaling = signaling.clone();
        let multistream = multistream.clone();
        let recorder = recorder.clone();
        let evt_tx = evt_tx.clone();
        let mut evt_rx = evt_tx.subscribe();

        tokio::spawn(async move {
            let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let (ws_write, mut ws_read) = ws_stream.split();
            let ws_write = Arc::new(AsyncMutex::new(ws_write));

            let ws_write_evt = ws_write.clone();
            let evt_forward = tokio::spawn(async move {
                while let Ok(raw) = evt_rx.recv().await {
                    if ws_write_evt
                        .lock()
                        .await
                        .send(Message::Text(raw.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            while let Some(Ok(Message::Text(text))) = ws_read.next().await {
                let Ok(req) = serde_json::from_str::<RpcRequest>(&text) else {
                    continue;
                };
                let req_id = req.id.clone();
                let response = handle_rpc(
                    req,
                    &store,
                    &discovery,
                    &signaling,
                    &multistream,
                    &recorder,
                    &evt_tx,
                )
                .await;

                if let Some(id) = req_id {
                    let reply = json!({
                        "id": id,
                        "result": response,
                    });
                    if let Ok(raw) = serde_json::to_string(&reply) {
                        let _ = ws_write.lock().await.send(Message::Text(raw.into())).await;
                    }
                }
            }

            evt_forward.abort();
        });
    }
}

async fn handle_rpc(
    req: RpcRequest,
    store: &Arc<SettingsStore>,
    discovery: &Arc<AsyncMutex<DiscoveryEngine>>,
    signaling: &Arc<AsyncMutex<SignalingServer>>,
    multistream: &Arc<Mutex<MultistreamEngine>>,
    recorder: &Arc<Mutex<RecorderEngine>>,
    evt_tx: &broadcast::Sender<String>,
) -> Value {
    match req.method.as_str() {
        "app.info" => {
            let host = hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "PrismCast-PC".to_string());
            let encoder = multistream.lock().unwrap().detected_encoder().to_string();
            json!({
                "name": "PrismCast Studio",
                "version": env!("CARGO_PKG_VERSION"),
                "hostname": host,
                "engine": "rust-native",
                "hardwareEncoder": encoder,
            })
        }
        "settings.get" => serde_json::to_value(store.get()).unwrap_or(Value::Null),
        "settings.set" => {
            let next = store.update(req.params);
            discovery
                .lock()
                .await
                .set_preferred_interface(next.preferred_interface.clone());
            serde_json::to_value(next).unwrap_or(Value::Null)
        }
        "net.interfaces" => serde_json::to_value(net::list_interfaces()).unwrap_or(json!([])),
        "firewall.status" => serde_json::to_value(firewall::check_status()).unwrap_or(Value::Null),
        "firewall.authorize" => match firewall::authorize() {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "ok": false, "error": e }),
        },
        "host.start" => {
            let settings = store.get();
            let source_name = req
                .params
                .get("sourceName")
                .and_then(|v| v.as_str())
                .unwrap_or("Tela Principal")
                .to_string();
            let source_kind = match req.params.get("sourceKind").and_then(|v| v.as_str()) {
                Some("window") => SourceKind::Window,
                _ => SourceKind::Screen,
            };
            let host_name = req
                .params
                .get("hostName")
                .and_then(|v| v.as_str())
                .unwrap_or(&settings.host_name)
                .to_string();
            let require_pin = req
                .params
                .get("requirePin")
                .and_then(|v| v.as_bool())
                .unwrap_or(settings.require_pin);
            let max_viewers = req
                .params
                .get("maxViewers")
                .and_then(|v| v.as_u64())
                .unwrap_or(MAX_VIEWERS as u64) as usize;

            let pin = if require_pin { Some(generate_pin()) } else { None };
            let port = net::find_free_port(SIGNALING_PORT);
            let ip = net::resolve_local_address(settings.preferred_interface.as_deref());
            let session_id = Uuid::new_v4().to_string();

            let evt_viewers = evt_tx.clone();
            let evt_messages = evt_tx.clone();
            let disc_ref = discovery.clone();

            let sig_res = signaling
                .lock()
                .await
                .start(
                    port,
                    HostSessionConfig {
                        host_name: host_name.clone(),
                        source_name: source_name.clone(),
                        source_kind,
                        pin: pin.clone(),
                        max_viewers: max_viewers.clamp(1, MAX_VIEWERS),
                    },
                    move |viewers| {
                        let count = viewers.len();
                        let disc_clone = disc_ref.clone();
                        tokio::spawn(async move {
                            disc_clone.lock().await.patch_announce(Some(count), None, None);
                        });
                        if let Ok(raw) = serde_json::to_string(&RpcEvent {
                            event: "host:viewers".to_string(),
                            payload: serde_json::to_value(viewers).unwrap_or(json!([])),
                        }) {
                            let _ = evt_viewers.send(raw);
                        }
                    },
                    move |viewer_id, msg| {
                        if let Ok(raw) = serde_json::to_string(&RpcEvent {
                            event: "host:message".to_string(),
                            payload: json!({ "viewerId": viewer_id, "msg": msg }),
                        }) {
                            let _ = evt_messages.send(raw);
                        }
                    },
                )
                .await;

            if let Err(err) = sig_res {
                return json!({ "ok": false, "error": err });
            }

            let evt_hosts = evt_tx.clone();
            let mut disc = discovery.lock().await;
            disc.set_preferred_interface(settings.preferred_interface);
            let _ = disc
                .start_announcing(
                    AnnounceState {
                        id: session_id.clone(),
                        name: host_name,
                        ip: ip.clone(),
                        port,
                        requires_pin: pin.is_some(),
                        source_name,
                        source_kind,
                        viewers: 0,
                        max_viewers,
                        started_at: now_ms(),
                    },
                    move |hosts| {
                        if let Ok(raw) = serde_json::to_string(&RpcEvent {
                            event: "discovery:hosts".to_string(),
                            payload: serde_json::to_value(hosts).unwrap_or(json!([])),
                        }) {
                            let _ = evt_hosts.send(raw);
                        }
                    },
                )
                .await;

            json!({
                "ok": true,
                "sessionId": session_id,
                "ip": ip,
                "port": port,
                "pin": pin,
            })
        }
        "host.stop" => {
            discovery.lock().await.stop_announcing().await;
            signaling.lock().await.stop().await;
            json!({ "ok": true })
        }
        "host.kick" => {
            if let Some(vid) = req.params.get("viewerId").and_then(|v| v.as_str()) {
                signaling.lock().await.kick(vid);
            }
            json!({ "ok": true })
        }
        "host.send" => {
            if let (Some(vid), Some(msg_val)) = (
                req.params.get("viewerId").and_then(|v| v.as_str()),
                req.params.get("msg"),
            ) {
                if let Ok(msg) = serde_json::from_value::<HostToViewer>(msg_val.clone()) {
                    signaling.lock().await.send(vid, &msg);
                }
            }
            json!({ "ok": true })
        }
        "host.broadcast" => {
            if let Some(msg_val) = req.params.get("msg") {
                if let Ok(msg) = serde_json::from_value::<HostToViewer>(msg_val.clone()) {
                    signaling.lock().await.broadcast(&msg);
                }
            }
            json!({ "ok": true })
        }
        "discovery.start" => {
            let evt_hosts = evt_tx.clone();
            let pref = store.get().preferred_interface;
            let mut disc = discovery.lock().await;
            disc.set_preferred_interface(pref);
            let _ = disc
                .start_browsing(move |hosts| {
                    if let Ok(raw) = serde_json::to_string(&RpcEvent {
                        event: "discovery:hosts".to_string(),
                        payload: serde_json::to_value(hosts).unwrap_or(json!([])),
                    }) {
                        let _ = evt_hosts.send(raw);
                    }
                })
                .await;
            json!({ "ok": true })
        }
        "discovery.refresh" => {
            discovery.lock().await.refresh().await;
            json!({ "ok": true })
        }
        "multistream.status" => {
            serde_json::to_value(multistream.lock().unwrap().status()).unwrap_or(Value::Null)
        }
        "multistream.start" => {
            let targets = req
                .params
                .get("targets")
                .and_then(|v| serde_json::from_value::<Vec<MultistreamTarget>>(v.clone()).ok())
                .unwrap_or_else(|| store.get().multistream_targets);
            let fps = req
                .params
                .get("fps")
                .and_then(|v| v.as_u64())
                .unwrap_or(60) as u32;
            match multistream.lock().unwrap().start(targets, fps) {
                Ok(status) => json!({ "ok": true, "status": status }),
                Err(error) => json!({ "ok": false, "error": error }),
            }
        }
        "multistream.pushChunk" => {
            let orientation = match req.params.get("orientation").and_then(|v| v.as_str()) {
                Some("vertical") => CanvasOrientation::Vertical,
                _ => CanvasOrientation::Horizontal,
            };
            if let Some(b64) = req.params.get("chunkBase64").and_then(|v| v.as_str()) {
                if let Ok(bytes) = BASE64.decode(b64) {
                    multistream.lock().unwrap().push_chunk(orientation, &bytes);
                }
            }
            json!({ "ok": true })
        }
        "multistream.stop" => {
            let status = multistream.lock().unwrap().stop();
            json!({ "ok": true, "status": status })
        }
        "recorder.status" => {
            serde_json::to_value(recorder.lock().unwrap().status()).unwrap_or(Value::Null)
        }
        "recorder.start" => match recorder.lock().unwrap().start_recording() {
            Ok(status) => json!({ "ok": true, "status": status }),
            Err(error) => json!({ "ok": false, "error": error }),
        },
        "recorder.stop" => {
            let status = recorder.lock().unwrap().stop_recording();
            json!({ "ok": true, "status": status })
        }
        "recorder.setReplayBuffer" => {
            let enabled = req
                .params
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let seconds = req
                .params
                .get("seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(30) as u32;
            let status = recorder.lock().unwrap().set_replay_buffer(enabled, seconds);
            json!({ "ok": true, "status": status })
        }
        "recorder.pushChunk" => {
            if let Some(b64) = req.params.get("chunkBase64").and_then(|v| v.as_str()) {
                if let Ok(bytes) = BASE64.decode(b64) {
                    recorder.lock().unwrap().push_media_chunk(&bytes);
                }
            }
            json!({ "ok": true })
        }
        "recorder.saveReplay" => match recorder.lock().unwrap().save_replay() {
            Ok(path) => json!({ "ok": true, "path": path, "status": recorder.lock().unwrap().status() }),
            Err(error) => json!({ "ok": false, "error": error }),
        },
        _ => json!({ "ok": false, "error": format!("Metodo desconhecido: {}", req.method) }),
    }
}

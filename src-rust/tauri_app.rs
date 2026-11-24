//! Integração Tauri v2 (`tauri_app.rs`) — substitui `src/main/index.ts`,
//! `src/main/window.ts`, `src/main/chat-window.ts`, `src/main/ipc-handlers.ts` e `src/preload/*`.
//!
//! - Expõe todos os `#[tauri::command]` consumidos por `src/renderer/bridge.ts`
//!   delegando diretamente ao `Arc<AppEngine>` compartilhado em `tauri::State`.
//! - Gerencia a janela principal sem moldura (`main`) e a janela popout de chat (`chat`).
//! - Intercepta `WindowEvent::CloseRequested` quando `engine.is_busy()` (`live || recording`),
//!   impedindo fechamento acidental durante transmissão/gravação e emitindo `app:confirm-quit`.
//! - Encaminha todos os eventos de `AppEngine::subscribe()` (`settings:changed`, `audio:levels`,
//!   `telemetry:stats`, `chat:event`, `multichat:event`, `hotkey:triggered`, `host:viewers`,
//!   `discovery:hosts`, `viewer:message`, `viewer:closed`, `dest:twitch-state`, `recorder:status`)
//!   para as janelas WebView via `AppHandle::emit`.

use crate::rpc::AppEngine;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use url::Url;

/// Valida estritamente URLs externas antes de abrir no navegador do sistema (apenas `https://`).
pub fn validate_external_https_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let parsed = Url::parse(trimmed).map_err(|e| format!("URL inválida: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("Apenas URLs https:// são permitidas.".to_string());
    }
    if parsed.host_str().unwrap_or("").is_empty() {
        return Err("URL sem host válido.".to_string());
    }
    Ok(parsed.to_string())
}

/// Estado compartilhado gerenciado pelo Tauri (`app.manage(TauriEngineState)`).
pub struct TauriEngineState {
    pub engine: Arc<AppEngine>,
    pub force_close: AtomicBool,
}

impl TauriEngineState {
    pub fn new(engine: Arc<AppEngine>) -> Self {
        Self {
            engine,
            force_close: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "tauri-app")]
mod commands {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::Ordering;
    use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};
    use tauri_plugin_dialog::DialogExt;
    use tauri_plugin_opener::OpenerExt;

    #[tauri::command]
    pub async fn rpc_call(
        state: State<'_, TauriEngineState>,
        method: String,
        params: Option<Value>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(&method, params.unwrap_or(Value::Null))
            .await)
    }

    #[tauri::command]
    pub async fn app_info(state: State<'_, TauriEngineState>) -> Result<Value, String> {
        Ok(state.engine.handle_command("app_info", Value::Null).await)
    }

    #[tauri::command]
    pub fn app_minimize(window: tauri::Window) -> Result<Value, String> {
        let _ = window.minimize();
        Ok(json!({ "ok": true }))
    }

    #[tauri::command]
    pub fn app_toggle_maximize(window: tauri::Window, app: AppHandle) -> Result<Value, String> {
        let is_max = window.is_maximized().unwrap_or(false);
        if is_max {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
        let now_max = window.is_maximized().unwrap_or(!is_max);
        if window.label() == "main" {
            let _ = app.emit("app:maximize-changed", now_max);
        }
        Ok(json!({ "ok": true, "maximized": now_max }))
    }

    #[tauri::command]
    pub fn app_close(
        window: tauri::Window,
        app: AppHandle,
        state: State<'_, TauriEngineState>,
        force: Option<bool>,
    ) -> Result<Value, String> {
        if window.label() == "chat" {
            let _ = window.close();
            let _ = app.emit("chat:popout-changed", false);
            return Ok(json!({ "ok": true }));
        }
        if force.unwrap_or(false) {
            state.force_close.store(true, Ordering::SeqCst);
        }
        if !state.force_close.load(Ordering::SeqCst) && state.engine.is_busy() {
            let _ = app.emit("app:confirm-quit", json!({ "busy": true }));
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.eval("window.dispatchEvent(new CustomEvent('app:confirm-quit'))");
            }
            return Ok(json!({ "ok": false, "busy": true }));
        }
        if let Some(chat) = app.get_webview_window("chat") {
            let _ = chat.destroy();
        }
        let _ = window.close();
        Ok(json!({ "ok": true }))
    }

    #[tauri::command]
    pub fn app_always_on_top(
        app: AppHandle,
        value: Option<bool>,
        flag: Option<bool>,
    ) -> Result<bool, String> {
        let enabled = value.or(flag).unwrap_or(false);
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.set_always_on_top(enabled);
        }
        Ok(enabled)
    }

    #[tauri::command]
    pub fn app_open_external(app: AppHandle, url: String) -> Result<Value, String> {
        let safe_url = validate_external_https_url(&url)?;
        app.opener()
            .open_url(&safe_url, None::<&str>)
            .map_err(|e| format!("Falha ao abrir URL externa: {e}"))?;
        Ok(json!({ "ok": true }))
    }

    #[tauri::command]
    pub fn app_show_in_folder(app: AppHandle, path: String) -> Result<Value, String> {
        if !path.trim().is_empty() {
            let _ = app.opener().reveal_item_in_dir(&path);
        }
        Ok(json!({ "ok": true }))
    }

    #[tauri::command]
    pub fn chat_open_popout(app: AppHandle) -> Result<Value, String> {
        if let Some(existing) = app.get_webview_window("chat") {
            let _ = existing.show();
            let _ = existing.set_focus();
            let _ = app.emit("chat:popout-changed", true);
            return Ok(json!({ "ok": true, "open": true }));
        }

        let builder = WebviewWindowBuilder::new(
            &app,
            "chat",
            WebviewUrl::App("index.html?view=chat".into()),
        )
        .title("PrismCast Studio — Chat Popout")
        .inner_size(420.0, 720.0)
        .min_inner_size(320.0, 460.0)
        .decorations(false)
        .resizable(true);

        match builder.build() {
            Ok(win) => {
                let app_ev = app.clone();
                win.on_window_event(move |ev| {
                    if matches!(ev, WindowEvent::Destroyed) {
                        let _ = app_ev.emit("chat:popout-changed", false);
                    }
                });
                let _ = app.emit("chat:popout-changed", true);
                Ok(json!({ "ok": true, "open": true }))
            }
            Err(e) => Ok(json!({ "ok": false, "error": e.to_string() })),
        }
    }

    #[tauri::command]
    pub fn chat_close_popout(app: AppHandle) -> Result<Value, String> {
        if let Some(win) = app.get_webview_window("chat") {
            let _ = win.close();
        }
        let _ = app.emit("chat:popout-changed", false);
        Ok(json!({ "ok": true, "open": false }))
    }

    #[tauri::command]
    pub fn chat_is_popout_open(app: AppHandle) -> Result<bool, String> {
        Ok(app.get_webview_window("chat").is_some())
    }

    #[tauri::command]
    pub async fn kit_pick_folder(app: AppHandle) -> Result<Value, String> {
        let folder = tokio::task::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .ok()
            .flatten();
        match folder {
            Some(f) => Ok(Value::String(f.to_string())),
            None => Ok(Value::Null),
        }
    }

    #[tauri::command]
    pub async fn files_pick_image(app: AppHandle) -> Result<Value, String> {
        let picked = tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .add_filter("Imagens", &["png", "jpg", "jpeg", "webp", "gif", "svg"])
                .blocking_pick_file()
        })
        .await
        .ok()
        .flatten();
        match picked {
            Some(f) => Ok(Value::String(f.to_string())),
            None => Ok(Value::Null),
        }
    }

    #[tauri::command]
    pub async fn files_pick_html(app: AppHandle) -> Result<Value, String> {
        let picked = tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .add_filter("HTML / Widget", &["html", "htm"])
                .blocking_pick_file()
        })
        .await
        .ok()
        .flatten();
        match picked {
            Some(f) => Ok(Value::String(f.to_string())),
            None => Ok(Value::Null),
        }
    }

    #[tauri::command]
    pub async fn files_pick_media(app: AppHandle) -> Result<Value, String> {
        let picked = tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .add_filter(
                    "Mídia de Transição (Stinger / Luma Matte)",
                    &["webm", "mp4", "mov", "png", "jpg", "jpeg", "webp"],
                )
                .blocking_pick_file()
        })
        .await
        .ok()
        .flatten();
        match picked {
            Some(f) => Ok(Value::String(f.to_string())),
            None => Ok(Value::Null),
        }
    }

    #[tauri::command]
    pub async fn files_read_image(
        state: State<'_, TauriEngineState>,
        path: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("files_read_image", json!({ "path": path }))
            .await)
    }

    // Macro para gerar os comandos Tauri 1:1 com src/renderer/bridge.ts
    macro_rules! delegate_cmd {
        ($fn_name:ident, $method:literal) => {
            #[tauri::command]
            pub async fn $fn_name(
                state: State<'_, TauriEngineState>,
                params: Option<Value>,
            ) -> Result<Value, String> {
                Ok(state
                    .engine
                    .handle_command($method, params.unwrap_or(Value::Null))
                    .await)
            }
        };
    }

    delegate_cmd!(sources_list, "sources_list");
    delegate_cmd!(audio_list_apps, "audio_list_apps");
    delegate_cmd!(audio_list_devices, "audio_list_devices");
    delegate_cmd!(plugins_scan, "plugins_scan");
    delegate_cmd!(gpu_status, "gpu_status");
    delegate_cmd!(abr_status, "abr_status");
    delegate_cmd!(virtual_cam_status, "virtual_cam_status");
    delegate_cmd!(virtual_cam_devices, "virtual_cam_devices");
    delegate_cmd!(virtual_cam_stop, "virtual_cam_stop");
    delegate_cmd!(external_control_status, "external_control_status");
    delegate_cmd!(external_control_restart, "external_control_restart");
    delegate_cmd!(telemetry_get, "telemetry_get");
    delegate_cmd!(twitch_capabilities, "twitch_capabilities");
    delegate_cmd!(twitch_get_info, "twitch_get_info");
    delegate_cmd!(multichat_status, "multichat_status");
    delegate_cmd!(multichat_reconnect, "multichat_reconnect");
    delegate_cmd!(host_stop, "host_stop");
    delegate_cmd!(discovery_start, "discovery_start");
    delegate_cmd!(discovery_stop, "discovery_stop");
    delegate_cmd!(discovery_refresh, "discovery_refresh");
    delegate_cmd!(viewer_disconnect, "viewer_disconnect");
    delegate_cmd!(multistream_status, "multistream_status");
    delegate_cmd!(multistream_stop, "multistream_stop");
    delegate_cmd!(recorder_status, "recorder_status");
    delegate_cmd!(recorder_start, "recorder_start");
    delegate_cmd!(recorder_stop, "recorder_stop");
    delegate_cmd!(recorder_reset, "recorder_reset");
    delegate_cmd!(recorder_save_replay, "recorder_save_replay");
    delegate_cmd!(net_interfaces, "net_interfaces");
    delegate_cmd!(firewall_status, "firewall_status");
    delegate_cmd!(firewall_authorize, "firewall_authorize");
    delegate_cmd!(settings_get, "settings_get");
    delegate_cmd!(dest_twitch_cancel, "dest_twitch_cancel");

    #[tauri::command]
    pub async fn plugins_open_gui(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] channelId: String,
        #[allow(non_snake_case)] slotId: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "plugins_open_gui",
                json!({ "channelId": channelId, "slotId": slotId }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn plugins_set_param(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] channelId: String,
        #[allow(non_snake_case)] slotId: String,
        #[allow(non_snake_case)] paramId: u32,
        value: f64,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "plugins_set_param",
                json!({
                    "channelId": channelId,
                    "slotId": slotId,
                    "paramId": paramId,
                    "value": value,
                }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn virtual_cam_start(
        state: State<'_, TauriEngineState>,
        orientation: Option<String>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("virtual_cam_start", json!({ "orientation": orientation }))
            .await)
    }

    #[tauri::command]
    pub async fn virtual_cam_push_frame(
        state: State<'_, TauriEngineState>,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    ) -> Result<Value, String> {
        let res = state
            .engine
            .virtual_cam
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_frame_bgra(width, height, &bgra);
        match res {
            Ok(()) => Ok(json!({ "ok": true })),
            Err(error) => Ok(json!({ "ok": false, "error": error })),
        }
    }

    #[tauri::command]
    pub async fn external_control_set_obs_password(
        state: State<'_, TauriEngineState>,
        password: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "external_control_set_obs_password",
                json!({ "password": password }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn sources_select(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] sourceId: Option<String>,
        #[allow(non_snake_case)] withAudio: Option<bool>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "sources_select",
                json!({ "sourceId": sourceId, "withAudio": withAudio }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn bs_sync(
        state: State<'_, TauriEngineState>,
        specs: Option<Value>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("bs_sync", json!({ "specs": specs }))
            .await)
    }

    #[tauri::command]
    pub async fn bs_reload(
        state: State<'_, TauriEngineState>,
        id: Option<String>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("bs_reload", json!({ "id": id }))
            .await)
    }

    #[tauri::command]
    pub async fn bs_ack(
        state: State<'_, TauriEngineState>,
        id: Option<String>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("bs_ack", json!({ "id": id }))
            .await)
    }

    #[tauri::command]
    pub async fn audio_sync(
        state: State<'_, TauriEngineState>,
        channels: Option<Value>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("audio_sync", json!({ "channels": channels }))
            .await)
    }

    #[tauri::command]
    pub async fn hotkeys_conflicts(
        state: State<'_, TauriEngineState>,
        bindings: Option<Value>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("hotkeys_conflicts", json!({ "bindings": bindings }))
            .await)
    }

    #[tauri::command]
    pub async fn settings_set(
        state: State<'_, TauriEngineState>,
        patch: Value,
    ) -> Result<Value, String> {
        Ok(state.engine.handle_command("settings_set", patch).await)
    }

    #[tauri::command]
    pub async fn dest_set_key(
        state: State<'_, TauriEngineState>,
        id: String,
        key: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("dest_set_key", json!({ "id": id, "key": key }))
            .await)
    }

    #[tauri::command]
    pub async fn dest_twitch_login(
        state: State<'_, TauriEngineState>,
        id: Option<String>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("dest_twitch_login", json!({ "id": id }))
            .await)
    }

    #[tauri::command]
    pub async fn dest_unlink(
        state: State<'_, TauriEngineState>,
        id: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("dest_unlink", json!({ "id": id }))
            .await)
    }

    #[tauri::command]
    pub async fn kit_load(
        state: State<'_, TauriEngineState>,
        path: Option<String>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("kit_load", json!({ "path": path }))
            .await)
    }

    #[tauri::command]
    pub async fn kit_read_config(
        state: State<'_, TauriEngineState>,
        path: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("kit_read_config", json!({ "path": path }))
            .await)
    }

    #[tauri::command]
    pub async fn kit_write_config(
        state: State<'_, TauriEngineState>,
        path: String,
        config: Value,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("kit_write_config", json!({ "path": path, "config": config }))
            .await)
    }

    #[tauri::command]
    pub async fn audio_app_for_window(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] captureId: Option<String>,
        source_id: Option<String>,
        #[allow(non_snake_case)] sourceId: Option<String>,
    ) -> Result<Value, String> {
        let sid = captureId.or(source_id).or(sourceId).unwrap_or_default();
        Ok(state
            .engine
            .handle_command("audio_app_for_window", json!({ "captureId": sid }))
            .await)
    }

    #[tauri::command]
    pub async fn twitch_update_info(
        state: State<'_, TauriEngineState>,
        info: Value,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("twitch_update_info", info)
            .await)
    }

    #[tauri::command]
    pub async fn twitch_search_categories(
        state: State<'_, TauriEngineState>,
        query: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("twitch_search_categories", json!({ "query": query }))
            .await)
    }

    #[tauri::command]
    pub async fn twitch_send_chat(
        state: State<'_, TauriEngineState>,
        text: String,
        channel: Option<String>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "twitch_send_chat",
                json!({ "text": text, "channel": channel }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn host_start(
        state: State<'_, TauriEngineState>,
        opts: Value,
    ) -> Result<Value, String> {
        Ok(state.engine.handle_command("host_start", opts).await)
    }

    #[tauri::command]
    pub async fn host_kick(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] viewerId: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("host_kick", json!({ "viewerId": viewerId }))
            .await)
    }

    #[tauri::command]
    pub async fn host_send(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] viewerId: String,
        msg: Value,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("host_send", json!({ "viewerId": viewerId, "msg": msg }))
            .await)
    }

    #[tauri::command]
    pub async fn host_broadcast(
        state: State<'_, TauriEngineState>,
        msg: Value,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("host_broadcast", json!({ "msg": msg }))
            .await)
    }

    #[tauri::command]
    pub async fn host_update_source(
        state: State<'_, TauriEngineState>,
        #[allow(non_snake_case)] sourceName: String,
        #[allow(non_snake_case)] sourceKind: String,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "host_update_source",
                json!({ "sourceName": sourceName, "sourceKind": sourceKind }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn host_set_viewer_count(
        state: State<'_, TauriEngineState>,
        n: Option<usize>,
        count: Option<usize>,
    ) -> Result<Value, String> {
        let val = n.or(count).unwrap_or(0);
        Ok(state
            .engine
            .handle_command("host_set_viewer_count", json!({ "n": val }))
            .await)
    }

    #[tauri::command]
    pub async fn viewer_connect(
        state: State<'_, TauriEngineState>,
        opts: Value,
    ) -> Result<Value, String> {
        Ok(state.engine.handle_command("viewer_connect", opts).await)
    }

    #[tauri::command]
    pub async fn viewer_send(
        state: State<'_, TauriEngineState>,
        msg: Value,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("viewer_send", json!({ "msg": msg }))
            .await)
    }

    #[tauri::command]
    pub async fn multistream_start(
        state: State<'_, TauriEngineState>,
        fps: Option<u32>,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command("multistream_start", json!({ "fps": fps }))
            .await)
    }

    #[tauri::command]
    pub async fn multistream_push_chunk(
        state: State<'_, TauriEngineState>,
        orientation: String,
        chunk: Vec<u8>,
    ) -> Result<Value, String> {
        let orient = if orientation == "vertical" {
            crate::protocol::CanvasOrientation::Vertical
        } else {
            crate::protocol::CanvasOrientation::Horizontal
        };
        state
            .engine
            .multistream
            .lock()
            .await
            .push_chunk(orient, &chunk);
        Ok(json!({ "ok": true }))
    }

    #[tauri::command]
    pub async fn recorder_set_replay(
        state: State<'_, TauriEngineState>,
        enabled: bool,
        seconds: u32,
    ) -> Result<Value, String> {
        Ok(state
            .engine
            .handle_command(
                "recorder_set_replay",
                json!({ "enabled": enabled, "seconds": seconds }),
            )
            .await)
    }

    #[tauri::command]
    pub async fn recorder_push_chunk(
        state: State<'_, TauriEngineState>,
        chunk: Vec<u8>,
    ) -> Result<Value, String> {
        state
            .engine
            .recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_media_chunk(&chunk);
        Ok(json!({ "ok": true }))
    }

    /// Inicializa e executa a aplicação desktop Tauri v2 com o `AppEngine` nativo.
    pub fn run_tauri_app(engine: Arc<AppEngine>) -> Result<(), String> {
        let eng_setup = engine.clone();

        tauri::Builder::default()
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_opener::init())
            .manage(TauriEngineState::new(engine))
            .setup(move |app| {
                let handle = app.handle().clone();
                let mut rx = eng_setup.subscribe();
                tauri::async_runtime::spawn(async move {
                    while let Ok(ev) = rx.recv().await {
                        let _ = handle.emit(&ev.event, ev.payload);
                    }
                });

                if let Some(main_win) = app.get_webview_window("main") {
                    let app_win = app.handle().clone();
                    let win_ref = main_win.clone();
                    main_win.on_window_event(move |event| match event {
                        WindowEvent::CloseRequested { api, .. } => {
                            let st = app_win.state::<TauriEngineState>();
                            if !st.force_close.load(Ordering::SeqCst) && st.engine.is_busy() {
                                api.prevent_close();
                                let _ = app_win.emit("app:confirm-quit", json!({ "busy": true }));
                                let _ = win_ref.eval(
                                    "window.dispatchEvent(new CustomEvent('app:confirm-quit'))",
                                );
                            } else if let Some(chat) = app_win.get_webview_window("chat") {
                                let _ = chat.destroy();
                            }
                        }
                        WindowEvent::Resized(_) => {
                            if let Ok(is_max) = win_ref.is_maximized() {
                                let _ = app_win.emit("app:maximize-changed", is_max);
                            }
                        }
                        _ => {}
                    });
                }
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                rpc_call,
                app_info,
                app_minimize,
                app_toggle_maximize,
                app_close,
                app_always_on_top,
                app_open_external,
                app_show_in_folder,
                chat_open_popout,
                chat_close_popout,
                chat_is_popout_open,
                kit_pick_folder,
                files_pick_image,
                files_pick_html,
                files_pick_media,
                files_read_image,
                sources_list,
                sources_select,
                bs_sync,
                bs_reload,
                bs_ack,
                audio_list_apps,
                audio_list_devices,
                audio_app_for_window,
                audio_sync,
                plugins_scan,
                plugins_open_gui,
                plugins_set_param,
                gpu_status,
                abr_status,
                virtual_cam_status,
                virtual_cam_devices,
                virtual_cam_start,
                virtual_cam_stop,
                virtual_cam_push_frame,
                external_control_status,
                external_control_set_obs_password,
                external_control_restart,
                telemetry_get,
                hotkeys_conflicts,
                twitch_capabilities,
                twitch_get_info,
                twitch_update_info,
                twitch_search_categories,
                twitch_send_chat,
                multichat_status,
                multichat_reconnect,
                host_start,
                host_stop,
                host_kick,
                host_send,
                host_broadcast,
                host_update_source,
                host_set_viewer_count,
                discovery_start,
                discovery_stop,
                discovery_refresh,
                viewer_connect,
                viewer_send,
                viewer_disconnect,
                multistream_status,
                multistream_start,
                multistream_push_chunk,
                multistream_stop,
                recorder_status,
                recorder_start,
                recorder_stop,
                recorder_set_replay,
                recorder_push_chunk,
                recorder_reset,
                recorder_save_replay,
                net_interfaces,
                firewall_status,
                firewall_authorize,
                settings_get,
                settings_set,
                dest_set_key,
                dest_twitch_login,
                dest_twitch_cancel,
                dest_unlink,
                kit_load,
                kit_read_config,
                kit_write_config,
            ])
            .run(tauri::generate_context!("tauri.conf.json"))
            .map_err(|e| format!("Erro fatal na execução do Tauri v2: {e}"))
    }
}

#[cfg(feature = "tauri-app")]
pub use commands::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_external_https_url_blocks_non_https() {
        assert!(validate_external_https_url("https://www.twitch.tv/activate").is_ok());
        assert!(validate_external_https_url("http://www.twitch.tv/activate").is_err());
        assert!(validate_external_https_url("file:///C:/Windows/System32/cmd.exe").is_err());
        assert!(validate_external_https_url("javascript:alert(1)").is_err());
        assert!(validate_external_https_url("ms-msdt:/id PCWDiagnostic").is_err());
    }
}

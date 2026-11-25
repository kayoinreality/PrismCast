//! Pipeline 100% nativo de captura de tela/janela + composição em tempo real.
//!
//! Substitui integralmente `navigator.mediaDevices.getDisplayMedia()` e `MediaRecorder`
//! do WebView/Tauri por captura direta no SO:
//! - Windows: DXGI Desktop Duplication + fallback GDI para monitores; `PrintWindow(PW_RENDERFULLCONTENT)`
//!   por `HWND` para janelas individuais (zero popup, zero barra `"tauri.localhost"`, zero roubo de foco).
//! - Imagens (`ImageSource`): decodificadas nativamente via `image` crate direto para `CaptureManager`.
//! - Composição: dispara `AppEngine.compositor.tick(...)` (`wgpu 24` com fallback CPU) e alimenta
//!   `VirtualCameraEngine`, `MultistreamEngine` (modo nativo) e snapshots RGBA leves para o palco `egui`.

use crate::capture::{self, CaptureStatus};
use crate::compositor::capture_key_for_item;
use crate::protocol::{CanvasOrientation, Scene, SourceKind, SourceType};
use crate::rpc::AppEngine;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Snapshot RGBA leve pronto para upload imediato como textura GPU no palco nativo.
#[derive(Debug, Clone)]
pub struct PreviewSnapshot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub seq: u64,
}

/// Estado compartilhado de prévia entre o pump nativo e a UI `egui`.
#[derive(Default)]
pub struct NativePreviewHub {
    pub program_h: Mutex<Option<PreviewSnapshot>>,
    pub program_v: Mutex<Option<PreviewSnapshot>>,
    pub studio_preview: Mutex<Option<PreviewSnapshot>>,
    pub seq: AtomicU64,
}

impl NativePreviewHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn publish_bgra(
        slot: &Mutex<Option<PreviewSnapshot>>,
        width: u32,
        height: u32,
        bgra: &[u8],
        seq: u64,
    ) {
        if width == 0 || height == 0 || bgra.len() != (width as usize) * (height as usize) * 4 {
            return;
        }
        let rgba = capture::bgra_to_rgba(bgra);
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(PreviewSnapshot {
                width,
                height,
                rgba,
                seq,
            });
        }
    }
}

/// Identificador parseado de uma fonte de captura (`screen:0:0` ou `window:12345:0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCaptureTarget {
    Screen(usize),
    Window(isize),
}

/// Interpreta `capture_id` no padrão `screen:<idx>:0` ou `window:<hwnd>:0`.
pub fn parse_capture_target(
    source_type: SourceType,
    capture_id: Option<&str>,
) -> Option<ParsedCaptureTarget> {
    let raw = capture_id.map(str::trim).filter(|s| !s.is_empty());
    if let Some(id) = raw {
        let mut parts = id.split(':');
        let prefix = parts.next().unwrap_or_default();
        let val = parts.next().unwrap_or_default();
        if prefix.eq_ignore_ascii_case("screen") {
            let idx = val.parse::<usize>().unwrap_or(0);
            return Some(ParsedCaptureTarget::Screen(idx));
        }
        if prefix.eq_ignore_ascii_case("window") {
            if let Ok(hwnd) = val.parse::<isize>() {
                if hwnd != 0 {
                    return Some(ParsedCaptureTarget::Window(hwnd));
                }
            }
            return None;
        }
    }
    match source_type {
        SourceType::ScreenCapture => Some(ParsedCaptureTarget::Screen(0)),
        _ => None,
    }
}

/// Handle do pump de captura/composição nativo em background.
pub struct NativeStudioPump {
    stop: Arc<AtomicBool>,
    pub previews: Arc<NativePreviewHub>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl NativeStudioPump {
    pub fn start(engine: Arc<AppEngine>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let previews = NativePreviewHub::new();
        let stop_bg = stop.clone();
        let previews_bg = previews.clone();

        let handle = std::thread::Builder::new()
            .name("prismcast-native-pump".to_string())
            .spawn(move || {
                run_pump_loop(engine, previews_bg, stop_bg);
            })
            .ok();

        Self {
            stop,
            previews,
            handle,
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for NativeStudioPump {
    fn drop(&mut self) {
        self.stop();
    }
}

fn collect_active_scenes(engine: &AppEngine) -> (Vec<Scene>, u32, bool, bool) {
    let settings = engine.store.snapshot();
    let fps = settings.studio.fps.clamp(15, 60);
    let dual = settings.studio.dual_output_enabled;
    let studio_mode = settings.scenes.studio_mode;

    let mut scenes = Vec::new();
    if let Some(prog) = settings
        .scenes
        .scenes
        .iter()
        .find(|s| s.id == settings.scenes.active_scene_id)
        .or_else(|| settings.scenes.scenes.first())
    {
        scenes.push(prog.clone());
    }
    if studio_mode {
        if let Some(prev) = settings
            .scenes
            .scenes
            .iter()
            .find(|s| s.id == settings.scenes.preview_scene_id)
        {
            if !scenes.iter().any(|s| s.id == prev.id) {
                scenes.push(prev.clone());
            }
        }
    }
    (scenes, fps, dual, studio_mode)
}

fn run_pump_loop(
    engine: Arc<AppEngine>,
    previews: Arc<NativePreviewHub>,
    stop: Arc<AtomicBool>,
) {
    let mut loaded_images: HashMap<String, u64> = HashMap::new();
    let mut last_recovery_check = Instant::now();
    let mut frame_seq: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        let tick_start = Instant::now();
        let ts_ms = now_ms();
        let (scenes, fps, _dual, studio_mode) = collect_active_scenes(&engine);
        let frame_interval = Duration::from_secs_f64(1.0 / (fps.max(15) as f64));

        // 1. Descobre todas as fontes necessárias nas cenas ativas/prévia
        let mut active_keys: HashSet<String> = HashSet::new();
        let mut capture_tasks: Vec<(
            String,
            SourceType,
            String,
            Option<String>,
            Option<String>,
        )> = Vec::new();

        for scene in &scenes {
            for item in &scene.items {
                if !item.visible {
                    continue;
                }
                match item.source_type {
                    SourceType::ScreenCapture | SourceType::WindowCapture => {
                        if let Some(key) = capture_key_for_item(item) {
                            if active_keys.insert(key.clone()) {
                                capture_tasks.push((
                                    key,
                                    item.source_type,
                                    item.name.clone(),
                                    item.settings.capture_id.clone(),
                                    item.settings.capture_name.clone(),
                                ));
                            }
                        }
                    }
                    SourceType::ImageSource => {
                        if let Some(path) = item
                            .settings
                            .path
                            .as_deref()
                            .map(str::trim)
                            .filter(|p| !p.is_empty())
                        {
                            if let Some(key) = capture_key_for_item(item) {
                                active_keys.insert(key.clone());
                                if !loaded_images.contains_key(&key) {
                                    if let Ok(fb) = capture::decode_image_file_bgra(path, ts_ms) {
                                        if let Ok(mut cm) = engine.capture.lock() {
                                            let _ = cm.push_full_frame(
                                                &key, fb.width, fb.height, fb.bgra, ts_ms,
                                            );
                                            loaded_images.insert(key, ts_ms);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Remove buffers antigos de fontes que saíram da cena
        if let Ok(mut cm) = engine.capture.lock() {
            cm.prune(&active_keys);
            loaded_images.retain(|k, _| active_keys.contains(k));
        }

        // 2. Captura nativa direta de cada tela / janela (sem popup e sem roubo de foco!)
        for (key, source_type, label, cap_id, cap_name) in &capture_tasks {
            let kind = if *source_type == SourceType::ScreenCapture {
                SourceKind::Screen
            } else {
                SourceKind::Window
            };
            if let Ok(mut cm) = engine.capture.lock() {
                cm.ensure_source(
                    key.clone(),
                    kind,
                    label.clone(),
                    cap_id.clone(),
                    cap_name.clone(),
                );
            }

            let resolved_id = engine
                .capture
                .lock()
                .ok()
                .and_then(|cm| cm.get_entry(key).and_then(|e| e.capture_id.clone()))
                .or_else(|| cap_id.clone());

            match parse_capture_target(*source_type, resolved_id.as_deref()) {
                Some(ParsedCaptureTarget::Screen(monitor_idx)) => {
                    // Tenta DXGI Zero-Copy no monitor 0 quando disponível, com fallback GDI limpo
                    let mut captured = None;
                    if monitor_idx == 0 {
                        if let Ok(mut gpu_cap) = engine.gpu_capture.lock() {
                            if let Ok(Some(fb)) = gpu_cap.poll_primary_cpu_frame(0, ts_ms) {
                                captured = Some(fb);
                            }
                        }
                    }
                    if captured.is_none() {
                        captured = capture::capture_screen_frame_bgra(monitor_idx, ts_ms);
                    }
                    if let Some(fb) = captured {
                        if let Ok(mut cm) = engine.capture.lock() {
                            let _ = cm.push_full_frame(key, fb.width, fb.height, fb.bgra, ts_ms);
                        }
                    }
                }
                Some(ParsedCaptureTarget::Window(hwnd)) => {
                    if !capture::is_window_handle_alive(hwnd) {
                        if let Ok(mut cm) = engine.capture.lock() {
                            cm.set_status(key, CaptureStatus::Missing);
                        }
                    } else if let Some(fb) = capture::capture_window_frame_bgra(hwnd, ts_ms) {
                        if let Ok(mut cm) = engine.capture.lock() {
                            let _ = cm.push_full_frame(key, fb.width, fb.height, fb.bgra, ts_ms);
                        }
                    }
                }
                None => {
                    if let Ok(mut cm) = engine.capture.lock() {
                        cm.set_status(key, CaptureStatus::Missing);
                    }
                }
            }
        }

        // Auto-recuperação periódica (a cada 3s) caso uma janela/jogo tenha sido reaberto
        if last_recovery_check.elapsed() >= Duration::from_secs(3) {
            last_recovery_check = Instant::now();
            let has_missing = engine
                .capture
                .lock()
                .map(|cm| {
                    capture_tasks
                        .iter()
                        .any(|(k, ..)| cm.status(k) == Some(CaptureStatus::Missing))
                })
                .unwrap_or(false);
            if has_missing {
                let avail = capture::list_capture_sources();
                if let Ok(mut cm) = engine.capture.lock() {
                    let _ = cm.poll_missing_recoveries(&avail, ts_ms);
                }
            }
        }

        // 3. Executa o compositor Rust (`wgpu` GPU ou fallback software) e publica prévias
        frame_seq = frame_seq.saturating_add(1);
        let mut vcam_frame: Option<(CanvasOrientation, u32, u32, Vec<u8>)> = None;
        let mut stream_frame_h: Option<Vec<u8>> = None;
        let mut stream_frame_v: Option<Vec<u8>> = None;

        let vcam_req = engine.virtual_cam.lock().ok().and_then(|vc| {
            if vc.is_active() {
                Some(vc.status().orientation)
            } else {
                None
            }
        });

        let stream_req = engine.multistream.try_lock().ok().and_then(|ms| {
            let st = ms.status();
            if st.active && ms.is_native_mode() {
                Some((st.horizontalLive, st.verticalLive))
            } else {
                None
            }
        });

        if let (Ok(cm), Ok(mut comp)) = (engine.capture.lock(), engine.compositor.lock()) {
            comp.tick(&cm, ts_ms, None);

            NativePreviewHub::publish_bgra(
                &previews.program_h,
                comp.preview_small_h.width,
                comp.preview_small_h.height,
                &comp.preview_small_h.bgra,
                frame_seq,
            );

            if comp.is_vertical_active() {
                NativePreviewHub::publish_bgra(
                    &previews.program_v,
                    comp.preview_small_v.width,
                    comp.preview_small_v.height,
                    &comp.preview_small_v.bgra,
                    frame_seq,
                );
            }

            if studio_mode {
                NativePreviewHub::publish_bgra(
                    &previews.studio_preview,
                    comp.preview.width,
                    comp.preview.height,
                    &comp.preview.bgra,
                    frame_seq,
                );
            }

            if let Some(orient) = vcam_req {
                let src = match orient {
                    CanvasOrientation::Horizontal => &comp.program,
                    CanvasOrientation::Vertical => &comp.vertical,
                };
                vcam_frame = Some((orient, src.width, src.height, src.bgra.clone()));
            }

            if let Some((need_h, need_v)) = stream_req {
                if need_h {
                    stream_frame_h = Some(comp.program.bgra.clone());
                }
                if need_v {
                    stream_frame_v = Some(comp.vertical.bgra.clone());
                }
            }
        }

        previews.seq.store(frame_seq, Ordering::Release);

        // 4. Alimenta Câmera Virtual e Multistream nativo sem segurar lock do compositor
        if let Some((_orient, w, h, bgra)) = vcam_frame {
            if let Ok(mut vc) = engine.virtual_cam.lock() {
                let _ = vc.push_frame_bgra(w, h, &bgra);
            }
        }
        if stream_frame_h.is_some() || stream_frame_v.is_some() {
            if let Ok(mut ms) = engine.multistream.try_lock() {
                if let Some(bgra) = stream_frame_h {
                    ms.push_native_frame(CanvasOrientation::Horizontal, &bgra);
                }
                if let Some(bgra) = stream_frame_v {
                    ms.push_native_frame(CanvasOrientation::Vertical, &bgra);
                }
            }
        }

        let spent = tick_start.elapsed();
        if spent < frame_interval {
            std::thread::sleep(frame_interval - spent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_capture_target_screen_and_window() {
        assert_eq!(
            parse_capture_target(SourceType::ScreenCapture, Some("screen:1:0")),
            Some(ParsedCaptureTarget::Screen(1))
        );
        assert_eq!(
            parse_capture_target(SourceType::ScreenCapture, None),
            Some(ParsedCaptureTarget::Screen(0))
        );
        assert_eq!(
            parse_capture_target(SourceType::WindowCapture, Some("window:424242:0")),
            Some(ParsedCaptureTarget::Window(424242))
        );
        assert_eq!(
            parse_capture_target(SourceType::WindowCapture, None),
            None
        );
    }

    #[test]
    fn test_preview_hub_publishes_rgba_from_bgra() {
        let hub = NativePreviewHub::new();
        // 2x1 pixels BGRA: (B=10, G=20, R=30, A=255), (B=40, G=50, R=60, A=255)
        let bgra = vec![10, 20, 30, 255, 40, 50, 60, 255];
        NativePreviewHub::publish_bgra(&hub.program_h, 2, 1, &bgra, 7);
        let snap = hub.program_h.lock().unwrap().clone().unwrap();
        assert_eq!(snap.seq, 7);
        assert_eq!(snap.rgba, vec![30, 20, 10, 255, 60, 50, 40, 255]);
    }
}

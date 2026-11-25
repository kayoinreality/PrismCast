use crate::protocol::{DesktopSource, SourceKind};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Estado vivo de uma fonte de captura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStatus {
    Ok,
    Loading,
    Missing,
    Error,
}

impl CaptureStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureStatus::Ok => "ok",
            CaptureStatus::Loading => "loading",
            CaptureStatus::Missing => "missing",
            CaptureStatus::Error => "error",
        }
    }
}

/// Retângulo sujo de uma fonte (usado pelos Browser Sources offscreen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Frame em memória no formato BGRA (4 bytes por pixel, pré-multiplicado ou opaco).
#[derive(Debug, Clone)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub seq: u64,
    pub updated_at_ms: u64,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        Self {
            width,
            height,
            bgra: vec![0; len],
            seq: 0,
            updated_at_ms: 0,
        }
    }

    pub fn from_bgra(width: u32, height: u32, bgra: Vec<u8>, now_ms: u64) -> Result<Self, String> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if bgra.len() != expected {
            return Err(format!(
                "Tamanho de buffer BGRA inválido: esperado {expected}, recebido {}",
                bgra.len()
            ));
        }
        Ok(Self {
            width,
            height,
            bgra,
            seq: 1,
            updated_at_ms: now_ms,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CaptureEntry {
    pub key: String,
    pub kind: SourceKind,
    pub label: String,
    pub capture_id: Option<String>,
    pub capture_name: Option<String>,
    pub status: CaptureStatus,
    pub frame: Option<FrameBuffer>,
    pub last_recovery_attempt_ms: u64,
    pub is_browser_source: bool,
}

/// Resultado da tentativa de auto-recuperação de fontes perdidas (`Missing`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAction {
    pub key: String,
    pub resolved_source_id: String,
    pub resolved_name: String,
}

/// Enumera monitores e janelas visíveis no sistema operacional.
/// - Windows: `EnumDisplayMonitors` + `EnumWindows` (sem `desktopCapturer` pesado nem strings quebradas).
/// - Outros OS: fallback limpo retornando tela primária virtual caso não haja X11/Portal ativo.
pub fn list_capture_sources() -> Vec<DesktopSource> {
    list_native_sources_with_thumbs()
        .into_iter()
        .map(|e| e.source)
        .collect()
}

/// Resolve uma fonte pelo `capture_id` exato, depois pelo `capture_name` exato e
/// por fim pelo sufixo após `" - "` (janelas mudam o prefixo quando trocam de aba/arquivo).
pub fn resolve_desktop_source<'a>(
    sources: &'a [DesktopSource],
    capture_id: Option<&str>,
    capture_name: Option<&str>,
) -> Option<&'a DesktopSource> {
    let cid = capture_id.map(str::trim).filter(|s| !s.is_empty());
    let Some(id) = cid else {
        return sources.iter().find(|s| s.kind == SourceKind::Screen);
    };

    if let Some(exact) = sources.iter().find(|s| s.id == id) {
        return Some(exact);
    }

    let raw_name = capture_name.filter(|s| !s.trim().is_empty())?;
    if let Some(by_name) = sources.iter().find(|s| s.name == raw_name) {
        return Some(by_name);
    }
    let cname = raw_name.trim();
    if let Some(by_trimmed) = sources.iter().find(|s| s.name.trim() == cname) {
        return Some(by_trimmed);
    }

    let stem = cname.rsplit(" - ").next().map(str::trim).filter(|s| !s.is_empty())?;
    sources
        .iter()
        .find(|s| s.kind == SourceKind::Window && s.name.trim().ends_with(stem))
}

/// Gerenciador de capturas (tela, janela, câmera) e canvas offscreen de Browser Sources.
///
/// Correções da auditoria implementadas:
/// - `prune(active_keys)` libera memória dos `FrameBuffer`s (ex.: Browser Sources antigos) imediatamente.
/// - Dirty-rect com validação estrita de bounds (`x + width <= full_width`), evitando panics/OOB.
/// - Auto-recuperação periódica (`poll_missing_recoveries`) para janelas/jogos que fecharam ou trocaram de modo.
pub struct CaptureManager {
    entries: HashMap<String, CaptureEntry>,
    target_fps: u32,
    recovery_cooldown_ms: u64,
}

impl CaptureManager {
    pub fn new(target_fps: u32) -> Self {
        Self {
            entries: HashMap::new(),
            target_fps: target_fps.clamp(15, 60),
            recovery_cooldown_ms: 3000,
        }
    }

    pub fn display_key(capture_id: Option<&str>, capture_name: Option<&str>) -> String {
        format!(
            "display:{}:{}",
            capture_id.unwrap_or("primary"),
            capture_name.unwrap_or("")
        )
    }

    pub fn camera_key(device_id: Option<&str>) -> String {
        format!("camera:{}", device_id.unwrap_or("default"))
    }

    pub fn set_fps(&mut self, fps: u32) {
        self.target_fps = fps.clamp(15, 60);
    }

    pub fn target_fps(&self) -> u32 {
        self.target_fps
    }

    pub fn set_recovery_cooldown_ms(&mut self, ms: u64) {
        self.recovery_cooldown_ms = ms.max(500);
    }

    /// Garante registro de uma fonte de captura ou câmera.
    pub fn ensure_source(
        &mut self,
        key: String,
        kind: SourceKind,
        label: String,
        capture_id: Option<String>,
        capture_name: Option<String>,
    ) {
        self.entries.entry(key.clone()).or_insert_with(|| CaptureEntry {
            key,
            kind,
            label,
            capture_id,
            capture_name,
            status: CaptureStatus::Loading,
            frame: None,
            last_recovery_attempt_ms: 0,
            is_browser_source: false,
        });
    }

    pub fn set_status(&mut self, key: &str, status: CaptureStatus) {
        if let Some(e) = self.entries.get_mut(key) {
            e.status = status;
            if status == CaptureStatus::Missing || status == CaptureStatus::Error {
                // Limpa último frame congelado apenas se necessário? Mantemos ou limpamos;
                // no PrismCast a UI mostra overlay de "Janela fechada".
            }
        }
    }

    pub fn status(&self, key: &str) -> Option<CaptureStatus> {
        self.entries.get(key).map(|e| e.status)
    }

    /// Atualiza um frame inteiro BGRA de captura de tela, janela ou câmera.
    pub fn push_full_frame(
        &mut self,
        key: &str,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
        now_ms: u64,
    ) -> Result<(), String> {
        let next_seq = self
            .entries
            .get(key)
            .and_then(|e| e.frame.as_ref())
            .map(|f| f.seq.saturating_add(1))
            .unwrap_or(1);

        let mut fb = FrameBuffer::from_bgra(width, height, bgra, now_ms)?;
        fb.seq = next_seq;

        let entry = self.entries.entry(key.to_string()).or_insert_with(|| CaptureEntry {
            key: key.to_string(),
            kind: SourceKind::Screen,
            label: key.to_string(),
            capture_id: None,
            capture_name: None,
            status: CaptureStatus::Ok,
            frame: None,
            last_recovery_attempt_ms: 0,
            is_browser_source: false,
        });
        entry.status = CaptureStatus::Ok;
        entry.frame = Some(fb);
        Ok(())
    }

    /// Ingere um retângulo sujo BGRA vindo de um Browser Source offscreen, aplicando-o no
    /// canvas persistente do widget sem alocar um novo buffer completo a cada pintura parcial.
    pub fn push_browser_source_dirty_rect(
        &mut self,
        id: &str,
        full_width: u32,
        full_height: u32,
        rect: DirtyRect,
        bgra: &[u8],
        now_ms: u64,
    ) -> Result<(), String> {
        if full_width == 0 || full_height == 0 {
            return Err("Dimensões do Browser Source inválidas (0).".to_string());
        }
        if rect.width == 0 || rect.height == 0 {
            return Ok(());
        }
        if rect.x.saturating_add(rect.width) > full_width
            || rect.y.saturating_add(rect.height) > full_height
        {
            return Err(format!(
                "DirtyRect ({},{} {}x{}) excede canvas {}x{}",
                rect.x, rect.y, rect.width, rect.height, full_width, full_height
            ));
        }
        let expected_rect_bytes = (rect.width as usize)
            .saturating_mul(rect.height as usize)
            .saturating_mul(4);
        if bgra.len() != expected_rect_bytes {
            return Err(format!(
                "DirtyRect BGRA inválido: esperado {expected_rect_bytes}, recebido {}",
                bgra.len()
            ));
        }

        let entry = self.entries.entry(id.to_string()).or_insert_with(|| CaptureEntry {
            key: id.to_string(),
            kind: SourceKind::Window,
            label: id.to_string(),
            capture_id: None,
            capture_name: None,
            status: CaptureStatus::Ok,
            frame: Some(FrameBuffer::new(full_width, full_height)),
            last_recovery_attempt_ms: 0,
            is_browser_source: true,
        });
        entry.is_browser_source = true;
        entry.status = CaptureStatus::Ok;

        let needs_alloc = match entry.frame {
            Some(ref f) => f.width != full_width || f.height != full_height,
            None => true,
        };
        if needs_alloc {
            entry.frame = Some(FrameBuffer::new(full_width, full_height));
        }

        let fb = entry.frame.as_mut().unwrap();
        let stride_dst = (full_width as usize) * 4;
        let stride_src = (rect.width as usize) * 4;

        for row in 0..(rect.height as usize) {
            let dst_y = (rect.y as usize) + row;
            let dst_off = dst_y * stride_dst + (rect.x as usize) * 4;
            let src_off = row * stride_src;
            fb.bgra[dst_off..dst_off + stride_src]
                .copy_from_slice(&bgra[src_off..src_off + stride_src]);
        }
        fb.seq = fb.seq.saturating_add(1);
        fb.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn get_frame(&self, key: &str) -> Option<&FrameBuffer> {
        self.entries
            .get(key)
            .filter(|e| e.status == CaptureStatus::Ok)
            .and_then(|e| e.frame.as_ref())
    }

    pub fn get_entry(&self, key: &str) -> Option<&CaptureEntry> {
        self.entries.get(key)
    }

    /// Marca todas as entradas não presentes em `active_keys` para remoção e libera os buffers imediatamente.
    pub fn prune(&mut self, active_keys: &HashSet<String>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|k, entry| {
            let keep = active_keys.contains(k);
            if !keep {
                if let Some(ref mut f) = entry.frame {
                    f.bgra.clear();
                    f.bgra.shrink_to_fit();
                }
            }
            keep
        });
        before.saturating_sub(self.entries.len())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Auto-recuperação periódica: verifica entradas `Missing` cujo cooldown venceu
    /// e tenta reencontrar o handle na lista de fontes atual.
    pub fn poll_missing_recoveries(
        &mut self,
        available_sources: &[DesktopSource],
        now_ms: u64,
    ) -> Vec<RecoveryAction> {
        let mut recovered = Vec::new();
        for entry in self.entries.values_mut() {
            if entry.is_browser_source || entry.status != CaptureStatus::Missing {
                continue;
            }
            if now_ms.saturating_sub(entry.last_recovery_attempt_ms) < self.recovery_cooldown_ms {
                continue;
            }
            entry.last_recovery_attempt_ms = now_ms;

            if let Some(found) = resolve_desktop_source(
                available_sources,
                entry.capture_id.as_deref(),
                entry.capture_name.as_deref(),
            ) {
                entry.capture_id = Some(found.id.clone());
                entry.label = found.name.clone();
                entry.status = CaptureStatus::Loading;
                recovered.push(RecoveryAction {
                    key: entry.key.clone(),
                    resolved_source_id: found.id.clone(),
                    resolved_name: found.name.clone(),
                });
            }
        }
        recovered
    }
}

pub type SharedCaptureManager = Arc<Mutex<CaptureManager>>;

/// Thumbnail nativo decodificado em RGBA pronto para texturas GPU / `egui`.
#[derive(Debug, Clone)]
pub struct NativeThumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Fonte capturável acompanhada de thumbnail nativo em memória (sem re-decodificar base64 na UI).
#[derive(Debug, Clone)]
pub struct NativeSourceEntry {
    pub source: DesktopSource,
    pub thumb: Option<NativeThumbnail>,
    pub resolution: Option<(u32, u32)>,
}

/// Converte pixels BGRA para RGBA em um novo `Vec<u8>`.
pub fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        rgba.push(px[2]);
        rgba.push(px[1]);
        rgba.push(px[0]);
        rgba.push(255);
    }
    rgba
}

/// Codifica um buffer RGBA em PNG base64 (`data:image/png;base64,...`) para compatibilidade RPC.
pub fn encode_rgba_png_data_url(width: u32, height: u32, rgba: &[u8]) -> String {
    if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
        return String::new();
    }
    let Some(img) = image::RgbaImage::from_raw(width, height, rgba.to_vec()) else {
        return String::new();
    };
    let mut buf = std::io::Cursor::new(Vec::new());
    if img.write_to(&mut buf, image::ImageFormat::Png).is_err() {
        return String::new();
    }
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    format!("data:image/png;base64,{b64}")
}

/// Decodifica um arquivo de imagem (PNG, JPEG, WebP, GIF, BMP) em `FrameBuffer` BGRA nativo.
pub fn decode_image_file_bgra(path: &str, now_ms: u64) -> Result<FrameBuffer, String> {
    let img = image::open(path).map_err(|e| format!("Falha ao abrir imagem '{path}': {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err("Imagem vazia (dimensão 0)".to_string());
    }
    let raw = rgba.into_raw();
    let mut bgra = Vec::with_capacity(raw.len());
    for px in raw.chunks_exact(4) {
        bgra.push(px[2]);
        bgra.push(px[1]);
        bgra.push(px[0]);
        bgra.push(px[3]);
    }
    FrameBuffer::from_bgra(w, h, bgra, now_ms)
}

/// Enumera telas e janelas visíveis com miniaturas reais nativas do sistema operacional.
pub fn list_native_sources_with_thumbs() -> Vec<NativeSourceEntry> {
    #[cfg(windows)]
    {
        win_enum::enumerate_sources_with_thumbs()
    }
    #[cfg(target_os = "linux")]
    {
        linux_enum::enumerate_sources_with_thumbs()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        vec![NativeSourceEntry {
            source: DesktopSource {
                id: "screen:0:0".to_string(),
                name: "Tela Principal".to_string(),
                kind: SourceKind::Screen,
                thumbnail: String::new(),
                app_icon: None,
                display_id: "0".to_string(),
            },
            thumb: None,
            resolution: Some((1920, 1080)),
        }]
    }
}

/// Captura um frame BGRA nativo de um monitor pelo índice (`0`, `1`, ...) sem popup nem foco.
pub fn capture_screen_frame_bgra(monitor_idx: usize, now_ms: u64) -> Option<FrameBuffer> {
    #[cfg(windows)]
    {
        win_enum::capture_monitor_bgra(monitor_idx, now_ms)
    }
    #[cfg(not(windows))]
    {
        let _ = (monitor_idx, now_ms);
        None
    }
}

/// Captura um frame BGRA nativo de uma janela pelo handle `HWND` (`PrintWindow(PW_RENDERFULLCONTENT)`)
/// sem popup, sem barra do navegador e sem roubar foco da janela do estúdio.
pub fn capture_window_frame_bgra(hwnd_raw: isize, now_ms: u64) -> Option<FrameBuffer> {
    #[cfg(windows)]
    {
        win_enum::capture_window_bgra(hwnd_raw, now_ms)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd_raw, now_ms);
        None
    }
}

/// Verifica se o handle de janela ainda existe e é válido no SO.
pub fn is_window_handle_alive(hwnd_raw: isize) -> bool {
    #[cfg(windows)]
    {
        win_enum::is_hwnd_alive(hwnd_raw)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd_raw;
        false
    }
}

#[cfg(target_os = "linux")]
mod linux_enum {
    use super::*;

    pub fn enumerate_sources_with_thumbs() -> Vec<NativeSourceEntry> {
        let mut out = Vec::new();
        // Tenta descobrir monitores via xrandr quando há sessão X11 ativa
        if std::env::var_os("DISPLAY").is_some() {
            if let Ok(output) = std::process::Command::new("xrandr")
                .arg("--listmonitors")
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines().skip(1) {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let idx = out.len();
                        let name = trimmed
                            .split_whitespace()
                            .last()
                            .map(|s| format!("Tela {} ({s})", idx + 1))
                            .unwrap_or_else(|| format!("Tela {}", idx + 1));
                        out.push(NativeSourceEntry {
                            source: DesktopSource {
                                id: format!("screen:{idx}:0"),
                                name,
                                kind: SourceKind::Screen,
                                thumbnail: String::new(),
                                app_icon: None,
                                display_id: idx.to_string(),
                            },
                            thumb: None,
                            resolution: Some((1920, 1080)),
                        });
                    }
                }
            }
        }
        if out.is_empty() {
            out.push(NativeSourceEntry {
                source: DesktopSource {
                    id: "screen:0:0".to_string(),
                    name: "Tela Principal".to_string(),
                    kind: SourceKind::Screen,
                    thumbnail: String::new(),
                    app_icon: None,
                    display_id: "0".to_string(),
                },
                thumb: None,
                resolution: Some((1920, 1080)),
            });
        }
        out
    }
}

#[cfg(windows)]
mod win_enum {
    use super::*;
    use std::ffi::c_void;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject,
        SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HALFTONE, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetDesktopWindow, GetShellWindow, GetWindowLongW, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindow, IsWindowVisible, GWL_EXSTYLE,
        WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };

    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
    const THUMB_MAX_W: i32 = 320;
    const THUMB_MAX_H: i32 = 180;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn PrintWindow(hwnd: HWND, hdc_blt: HDC, n_flags: u32) -> BOOL;
    }

    struct MonitorDesc {
        idx: usize,
        rect: RECT,
        is_primary: bool,
    }

    fn collect_monitors() -> Vec<MonitorDesc> {
        let mut list = Vec::new();
        unsafe extern "system" fn monitor_cb(
            hmon: HMONITOR,
            _hdc: HDC,
            rect_ptr: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            let monitors = &mut *(lparam.0 as *mut Vec<MonitorDesc>);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let (rect, is_primary) = if GetMonitorInfoW(hmon, &mut info).as_bool() {
                (info.rcMonitor, (info.dwFlags & 1) != 0)
            } else if !rect_ptr.is_null() {
                (*rect_ptr, monitors.is_empty())
            } else {
                (RECT::default(), monitors.is_empty())
            };
            let idx = monitors.len();
            monitors.push(MonitorDesc {
                idx,
                rect,
                is_primary,
            });
            BOOL(1)
        }
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_cb),
                LPARAM(&mut list as *mut Vec<MonitorDesc> as isize),
            );
        }
        list
    }

    fn is_window_cloaked(hwnd: HWND) -> bool {
        let mut cloaked: u32 = 0;
        let res = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast::<c_void>(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        res.is_ok() && cloaked != 0
    }

    fn read_bitmap_bgra(hdc: HDC, hbmp: HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = unsafe {
            GetDIBits(
                hdc,
                hbmp,
                0,
                h as u32,
                Some(bgra.as_mut_ptr().cast::<c_void>()),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        if lines == 0 {
            return None;
        }
        // Garante canal alfa opaco (GDI frequentemente deixa alpha = 0).
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Some(bgra)
    }

    fn fit_thumb_dims(src_w: i32, src_h: i32) -> (i32, i32) {
        if src_w <= 0 || src_h <= 0 {
            return (0, 0);
        }
        let scale_w = THUMB_MAX_W as f32 / src_w as f32;
        let scale_h = THUMB_MAX_H as f32 / src_h as f32;
        let scale = scale_w.min(scale_h).min(1.0);
        let tw = ((src_w as f32 * scale).round() as i32).max(16);
        let th = ((src_h as f32 * scale).round() as i32).max(16);
        (tw, th)
    }

    fn capture_rect_bgra(rect: RECT, max_w: Option<i32>, max_h: Option<i32>) -> Option<(u32, u32, Vec<u8>)> {
        let src_w = rect.right - rect.left;
        let src_h = rect.bottom - rect.top;
        if src_w <= 0 || src_h <= 0 {
            return None;
        }
        let (dst_w, dst_h) = match (max_w, max_h) {
            (Some(mw), Some(mh)) => {
                let s = (mw as f32 / src_w as f32).min(mh as f32 / src_h as f32).min(1.0);
                (((src_w as f32 * s).round() as i32).max(16), ((src_h as f32 * s).round() as i32).max(16))
            }
            _ => (src_w, src_h),
        };

        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return None;
            }
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.is_invalid() {
                ReleaseDC(None, screen_dc);
                return None;
            }
            let hbmp = CreateCompatibleBitmap(screen_dc, dst_w, dst_h);
            if hbmp.is_invalid() {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
                return None;
            }
            let old: HGDIOBJ = SelectObject(mem_dc, HGDIOBJ(hbmp.0));
            let ok = if dst_w == src_w && dst_h == src_h {
                BitBlt(mem_dc, 0, 0, dst_w, dst_h, Some(screen_dc), rect.left, rect.top, SRCCOPY).is_ok()
            } else {
                let _ = SetStretchBltMode(mem_dc, HALFTONE);
                StretchBlt(
                    mem_dc,
                    0,
                    0,
                    dst_w,
                    dst_h,
                    Some(screen_dc),
                    rect.left,
                    rect.top,
                    src_w,
                    src_h,
                    SRCCOPY,
                )
                .as_bool()
            };
            let bgra = if ok {
                read_bitmap_bgra(mem_dc, hbmp, dst_w, dst_h)
            } else {
                None
            };
            SelectObject(mem_dc, old);
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            bgra.map(|data| (dst_w as u32, dst_h as u32, data))
        }
    }

    fn capture_hwnd_raw(hwnd: HWND, downscale_thumb: bool) -> Option<(u32, u32, Vec<u8>)> {
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() || IsIconic(hwnd).as_bool() {
                return None;
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return None;
            }
            let src_w = rect.right - rect.left;
            let src_h = rect.bottom - rect.top;
            if src_w < 8 || src_h < 8 {
                return None;
            }

            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return None;
            }
            let full_dc = CreateCompatibleDC(Some(screen_dc));
            if full_dc.is_invalid() {
                ReleaseDC(None, screen_dc);
                return None;
            }
            let full_bmp = CreateCompatibleBitmap(screen_dc, src_w, src_h);
            if full_bmp.is_invalid() {
                let _ = DeleteDC(full_dc);
                ReleaseDC(None, screen_dc);
                return None;
            }
            let old_full = SelectObject(full_dc, HGDIOBJ(full_bmp.0));

            // `PrintWindow(PW_RENDERFULLCONTENT)` captura janelas DirectComposition/Chromium/WPF/UWP
            // mesmo quando estão atrás do PrismCast Studio, sem popup nem roubo de foco!
            let printed = PrintWindow(hwnd, full_dc, PW_RENDERFULLCONTENT).as_bool()
                || PrintWindow(hwnd, full_dc, 0).as_bool()
                || BitBlt(
                    full_dc,
                    0,
                    0,
                    src_w,
                    src_h,
                    Some(screen_dc),
                    rect.left,
                    rect.top,
                    SRCCOPY,
                )
                .is_ok();

            let result = if !printed {
                None
            } else if downscale_thumb {
                let (tw, th) = fit_thumb_dims(src_w, src_h);
                let thumb_dc = CreateCompatibleDC(Some(screen_dc));
                let thumb_bmp = CreateCompatibleBitmap(screen_dc, tw, th);
                let out = if !thumb_dc.is_invalid() && !thumb_bmp.is_invalid() {
                    let old_t = SelectObject(thumb_dc, HGDIOBJ(thumb_bmp.0));
                    let _ = SetStretchBltMode(thumb_dc, HALFTONE);
                    let ok = StretchBlt(
                        thumb_dc,
                        0,
                        0,
                        tw,
                        th,
                        Some(full_dc),
                        0,
                        0,
                        src_w,
                        src_h,
                        SRCCOPY,
                    )
                    .as_bool();
                    let pixels = if ok {
                        read_bitmap_bgra(thumb_dc, thumb_bmp, tw, th)
                    } else {
                        None
                    };
                    SelectObject(thumb_dc, old_t);
                    pixels.map(|d| (tw as u32, th as u32, d))
                } else {
                    None
                };
                if !thumb_bmp.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(thumb_bmp.0));
                }
                if !thumb_dc.is_invalid() {
                    let _ = DeleteDC(thumb_dc);
                }
                out
            } else {
                read_bitmap_bgra(full_dc, full_bmp, src_w, src_h)
                    .map(|d| (src_w as u32, src_h as u32, d))
            };

            SelectObject(full_dc, old_full);
            let _ = DeleteObject(HGDIOBJ(full_bmp.0));
            let _ = DeleteDC(full_dc);
            ReleaseDC(None, screen_dc);
            result
        }
    }

    pub fn is_hwnd_alive(hwnd_raw: isize) -> bool {
        if hwnd_raw == 0 {
            return false;
        }
        unsafe { IsWindow(Some(HWND(hwnd_raw as *mut c_void))).as_bool() }
    }

    pub fn capture_monitor_bgra(monitor_idx: usize, now_ms: u64) -> Option<FrameBuffer> {
        let monitors = collect_monitors();
        let mon = monitors.get(monitor_idx).or_else(|| monitors.first())?;
        let (w, h, bgra) = capture_rect_bgra(mon.rect, None, None)?;
        FrameBuffer::from_bgra(w, h, bgra, now_ms).ok()
    }

    pub fn capture_window_bgra(hwnd_raw: isize, now_ms: u64) -> Option<FrameBuffer> {
        if hwnd_raw == 0 {
            return None;
        }
        let hwnd = HWND(hwnd_raw as *mut c_void);
        let (w, h, bgra) = capture_hwnd_raw(hwnd, false)?;
        FrameBuffer::from_bgra(w, h, bgra, now_ms).ok()
    }

    pub fn enumerate_sources_with_thumbs() -> Vec<NativeSourceEntry> {
        let mut entries = Vec::new();

        // 1. Monitores reais com resolução e miniatura ao vivo
        for mon in collect_monitors() {
            let w = (mon.rect.right - mon.rect.left).max(0) as u32;
            let h = (mon.rect.bottom - mon.rect.top).max(0) as u32;
            let name = if mon.is_primary {
                format!("Tela {} · Principal ({w}×{h})", mon.idx + 1)
            } else {
                format!("Tela {} ({w}×{h})", mon.idx + 1)
            };
            let thumb = capture_rect_bgra(mon.rect, Some(THUMB_MAX_W), Some(THUMB_MAX_H)).map(
                |(tw, th, bgra)| NativeThumbnail {
                    width: tw,
                    height: th,
                    rgba: bgra_to_rgba(&bgra),
                },
            );
            let thumb_uri = thumb
                .as_ref()
                .map(|t| encode_rgba_png_data_url(t.width, t.height, &t.rgba))
                .unwrap_or_default();
            entries.push(NativeSourceEntry {
                source: DesktopSource {
                    id: format!("screen:{}:0", mon.idx),
                    name,
                    kind: SourceKind::Screen,
                    thumbnail: thumb_uri,
                    app_icon: None,
                    display_id: mon.idx.to_string(),
                },
                thumb,
                resolution: Some((w, h)),
            });
        }

        // 2. Janelas reais visíveis (filtrando tool windows, invisíveis e UWP cloaked)
        let shell_hwnd = unsafe { GetShellWindow() };
        let desktop_hwnd = unsafe { GetDesktopWindow() };
        struct WinCollect {
            entries: Vec<NativeSourceEntry>,
            shell: HWND,
            desktop: HWND,
        }
        let mut state = WinCollect {
            entries,
            shell: shell_hwnd,
            desktop: desktop_hwnd,
        };

        unsafe extern "system" fn window_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let st = &mut *(lparam.0 as *mut WinCollect);
            if hwnd == st.shell || hwnd == st.desktop || !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            if is_window_cloaked(hwnd) {
                return BOOL(1);
            }
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if (ex_style & WS_EX_TOOLWINDOW.0) != 0 && (ex_style & WS_EX_APPWINDOW.0) == 0 {
                return BOOL(1);
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return BOOL(1);
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return BOOL(1);
            }
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            if w < 32 || h < 32 {
                return BOOL(1);
            }

            let mut buf = vec![0u16; (len as usize) + 1];
            let read = GetWindowTextW(hwnd, &mut buf);
            if read > 0 {
                let title = String::from_utf16_lossy(&buf[..read as usize])
                    .trim()
                    .to_string();
                if !title.is_empty()
                    && title != "Program Manager"
                    && title != "Windows Input Experience"
                {
                    let hwnd_num = hwnd.0 as isize;
                    let thumb = capture_hwnd_raw(hwnd, true).map(|(tw, th, bgra)| NativeThumbnail {
                        width: tw,
                        height: th,
                        rgba: bgra_to_rgba(&bgra),
                    });
                    let thumb_uri = thumb
                        .as_ref()
                        .map(|t| encode_rgba_png_data_url(t.width, t.height, &t.rgba))
                        .unwrap_or_default();
                    st.entries.push(NativeSourceEntry {
                        source: DesktopSource {
                            id: format!("window:{hwnd_num}:0"),
                            name: title,
                            kind: SourceKind::Window,
                            thumbnail: thumb_uri,
                            app_icon: None,
                            display_id: hwnd_num.to_string(),
                        },
                        thumb,
                        resolution: Some((w as u32, h as u32)),
                    });
                }
            }
            BOOL(1)
        }

        unsafe {
            let _ = EnumWindows(
                Some(window_cb),
                LPARAM(&mut state as *mut WinCollect as isize),
            );
        }

        state.entries
    }

    pub fn enumerate_sources() -> Vec<DesktopSource> {
        enumerate_sources_with_thumbs()
            .into_iter()
            .map(|e| e.source)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_source_dirty_rect_blending_and_bounds() {
        let mut mgr = CaptureManager::new(60);

        // Quadro inicial 4x4 branco opaco
        let full = vec![255u8; 4 * 4 * 4];
        mgr.push_browser_source_dirty_rect(
            "bs1",
            4,
            4,
            DirtyRect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            &full,
            100,
        )
        .unwrap();

        // Patch 2x2 vermelho em (1, 1) -> BGRA = (0, 0, 255, 255)
        let mut patch = Vec::with_capacity(2 * 2 * 4);
        for _ in 0..4 {
            patch.extend_from_slice(&[0, 0, 255, 255]);
        }
        mgr.push_browser_source_dirty_rect(
            "bs1",
            4,
            4,
            DirtyRect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
            &patch,
            116,
        )
        .unwrap();

        let fb = mgr.get_frame("bs1").expect("frame exists");
        assert_eq!(fb.seq, 2);
        // Pixel (0,0) ainda branco
        assert_eq!(&fb.bgra[0..4], &[255, 255, 255, 255]);
        // Pixel (1,1) = índice (1*4 + 1)*4 = 20 -> vermelho BGRA
        assert_eq!(&fb.bgra[20..24], &[0, 0, 255, 255]);
        // Pixel (2,2) = índice (2*4 + 2)*4 = 40 -> vermelho BGRA
        assert_eq!(&fb.bgra[40..44], &[0, 0, 255, 255]);

        // Out-of-bounds deve retornar erro limpo em vez de panic
        let oob = mgr.push_browser_source_dirty_rect(
            "bs1",
            4,
            4,
            DirtyRect {
                x: 3,
                y: 0,
                width: 2,
                height: 1,
            },
            &[0; 8],
            120,
        );
        assert!(oob.is_err());
    }

    #[test]
    fn test_prune_removes_unused_buffers() {
        let mut mgr = CaptureManager::new(60);
        mgr.push_full_frame("k1", 2, 2, vec![1; 16], 10).unwrap();
        mgr.push_full_frame("k2", 2, 2, vec![2; 16], 10).unwrap();
        mgr.push_full_frame("k3", 2, 2, vec![3; 16], 10).unwrap();
        assert_eq!(mgr.len(), 3);

        let keep: HashSet<String> = ["k2".to_string()].into_iter().collect();
        let removed = mgr.prune(&keep);
        assert_eq!(removed, 2);
        assert_eq!(mgr.len(), 1);
        assert!(mgr.get_frame("k1").is_none());
        assert!(mgr.get_frame("k2").is_some());
    }

    #[test]
    fn test_auto_recovery_state_machine_for_missing_window() {
        let mut mgr = CaptureManager::new(60);
        mgr.set_recovery_cooldown_ms(1000);
        let key = CaptureManager::display_key(Some("window:100:0"), Some("VALORANT  "));
        mgr.ensure_source(
            key.clone(),
            SourceKind::Window,
            "VALORANT".to_string(),
            Some("window:100:0".to_string()),
            Some("VALORANT  ".to_string()),
        );
        mgr.set_status(&key, CaptureStatus::Missing);

        // Jogo ainda fechado: não recupera
        let empty = vec![];
        assert!(mgr.poll_missing_recoveries(&empty, 1500).is_empty());

        // Jogo reabriu com novo HWND window:999:0, mas dentro do cooldown (1500 -> 2000)
        let reopened = vec![DesktopSource {
            id: "window:999:0".to_string(),
            name: "VALORANT  ".to_string(),
            kind: SourceKind::Window,
            thumbnail: String::new(),
            app_icon: None,
            display_id: "999".to_string(),
        }];
        assert!(mgr.poll_missing_recoveries(&reopened, 2000).is_empty());

        // Após cooldown (3100ms >= 2000 + 1000): recupera e muda para Loading com novo ID
        let actions = mgr.poll_missing_recoveries(&reopened, 3100);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].resolved_source_id, "window:999:0");
        assert_eq!(mgr.status(&key), Some(CaptureStatus::Loading));
        assert_eq!(
            mgr.get_entry(&key).unwrap().capture_id.as_deref(),
            Some("window:999:0")
        );
    }
}

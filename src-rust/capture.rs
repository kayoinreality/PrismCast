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
    #[cfg(windows)]
    {
        win_enum::enumerate_sources()
    }
    #[cfg(not(windows))]
    {
        vec![DesktopSource {
            id: "screen:0:0".to_string(),
            name: "Tela Principal".to_string(),
            kind: SourceKind::Screen,
            thumbnail: String::new(),
            app_icon: None,
            display_id: "0".to_string(),
        }]
    }
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

#[cfg(windows)]
mod win_enum {
    use super::*;
    use std::ffi::c_void;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    // `EnumDisplayMonitors` vem direto da user32.dll sem exigir feature extra em `Cargo.toml`.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumDisplayMonitors(
            hdc: *mut c_void,
            lprc_clip: *const RECT,
            lpfn_enum: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut RECT, LPARAM) -> BOOL,
            dw_data: LPARAM,
        ) -> BOOL;
    }

    pub fn enumerate_sources() -> Vec<DesktopSource> {
        let mut sources = Vec::new();

        unsafe extern "system" fn monitor_cb(
            _hmon: *mut c_void,
            _hdc: *mut c_void,
            _rect: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            let list = &mut *(lparam.0 as *mut Vec<DesktopSource>);
            let idx = list.len();
            list.push(DesktopSource {
                id: format!("screen:{idx}:0"),
                name: format!("Tela {}", idx + 1),
                kind: SourceKind::Screen,
                thumbnail: String::new(),
                app_icon: None,
                display_id: idx.to_string(),
            });
            BOOL(1)
        }

        unsafe extern "system" fn window_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return BOOL(1);
            }
            let mut buf = vec![0u16; (len as usize) + 1];
            let read = GetWindowTextW(hwnd, &mut buf);
            if read > 0 {
                let title = String::from_utf16_lossy(&buf[..read as usize])
                    .trim()
                    .to_string();
                if !title.is_empty() {
                    let list = &mut *(lparam.0 as *mut Vec<DesktopSource>);
                    let hwnd_num = hwnd.0 as isize;
                    list.push(DesktopSource {
                        id: format!("window:{hwnd_num}:0"),
                        name: title,
                        kind: SourceKind::Window,
                        thumbnail: String::new(),
                        app_icon: None,
                        display_id: hwnd_num.to_string(),
                    });
                }
            }
            BOOL(1)
        }

        unsafe {
            let _ = EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                monitor_cb,
                LPARAM(&mut sources as *mut Vec<DesktopSource> as isize),
            );
            let _ = EnumWindows(
                Some(window_cb),
                LPARAM(&mut sources as *mut Vec<DesktopSource> as isize),
            );
        }

        sources
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

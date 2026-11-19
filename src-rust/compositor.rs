use crate::capture::{CaptureManager, FrameBuffer};
use crate::protocol::{
    Scene, SceneItem, SourceTransform, SourceType, StudioCompositorSettings, TelemetryStats,
    TransitionType,
};
use std::collections::VecDeque;

pub const CANVAS_H: (u32, u32) = (1920, 1080);
pub const CANVAS_V: (u32, u32) = (1080, 1920);
pub const PREVIEW_H: (u32, u32) = (960, 540);
pub const PREVIEW_V: (u32, u32) = (360, 640);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FitMode {
    Contain,
    Cover,
    Stretch,
}

fn fit_for(item: &SceneItem) -> FitMode {
    match item.source_type {
        SourceType::CameraInput => FitMode::Cover,
        SourceType::BrowserSource => FitMode::Stretch,
        _ => FitMode::Contain,
    }
}

/// Normaliza `z_index` mantendo ordenação independente entre horizontal e vertical
/// (ou preservando índices existentes quando `vertical_order` for especificado).
pub fn normalize_z(scene: &mut Scene, preserve_vertical_z: bool) {
    for (idx, item) in scene.items.iter_mut().enumerate() {
        item.transform.z_index = idx as i32;
        if !preserve_vertical_z {
            item.vertical_transform.z_index = idx as i32;
        }
    }
}

/// Backend abstrato de composição (permite acoplar `wgpu` GPU sem alterar a API do compositor).
pub trait CompositorBackend {
    fn compose_scene(
        &mut self,
        target: &mut FrameBuffer,
        scene: &Scene,
        vertical: bool,
        captures: &CaptureManager,
        studio: &StudioCompositorSettings,
    );
}

/// Converte cor hex `#RRGGBB` / `#RGB` para BGRA `[b, g, r, 255]`.
pub fn parse_hex_bgra(hex: Option<&str>, fallback: [u8; 4]) -> [u8; 4] {
    let Some(raw) = hex.map(str::trim) else {
        return fallback;
    };
    let s = raw.strip_prefix('#').unwrap_or(raw);
    if s.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&s[0..2], 16),
            u8::from_str_radix(&s[2..4], 16),
            u8::from_str_radix(&s[4..6], 16),
        ) {
            return [b, g, r, 255];
        }
    } else if s.len() == 3 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&s[0..1], 16),
            u8::from_str_radix(&s[1..2], 16),
            u8::from_str_radix(&s[2..3], 16),
        ) {
            return [b * 17, g * 17, r * 17, 255];
        }
    }
    fallback
}

/// Calcula a chave de captura correspondente ao `SceneItem`.
pub fn capture_key_for_item(item: &SceneItem) -> Option<String> {
    match item.source_type {
        SourceType::ScreenCapture | SourceType::WindowCapture => Some(CaptureManager::display_key(
            item.settings.capture_id.as_deref(),
            item.settings.capture_name.as_deref(),
        )),
        SourceType::CameraInput => Some(CaptureManager::camera_key(
            item.settings.device_id.as_deref(),
        )),
        SourceType::BrowserSource => {
            let url = item.settings.url.as_deref()?.trim();
            if url.is_empty() {
                return None;
            }
            Some(format!(
                "{}x{}@{}|{}",
                item.settings.width.unwrap_or(1920),
                item.settings.height.unwrap_or(1080),
                item.settings.fps.unwrap_or(30),
                url
            ))
        }
        SourceType::ImageSource => item
            .settings
            .path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(|p| format!("image:{p}")),
        _ => None,
    }
}

/// Backend CPU BGRA otimizado (zero alocação por frame quando os buffers já existem),
/// pronto para ser trocado por backend `wgpu` DX12/Vulkan mantendo exatamente a mesma semântica.
pub struct SoftwareBackend;

impl CompositorBackend for SoftwareBackend {
    fn compose_scene(
        &mut self,
        target: &mut FrameBuffer,
        scene: &Scene,
        vertical: bool,
        captures: &CaptureManager,
        studio: &StudioCompositorSettings,
    ) {
        clear_bgra(&mut target.bgra, [0, 0, 0, 255]);
        let mut items: Vec<&SceneItem> = scene
            .items
            .iter()
            .filter(|it| {
                if !it.visible || it.source_type == SourceType::AudioInput {
                    return false;
                }
                if vertical && !it.show_on_vertical {
                    return false;
                }
                if !vertical && !it.show_on_horizontal {
                    return false;
                }
                if !studio.camera_enabled && it.source_type == SourceType::CameraInput {
                    return false;
                }
                true
            })
            .collect();

        items.sort_by_key(|it| {
            if vertical {
                it.vertical_transform.z_index
            } else {
                it.transform.z_index
            }
        });

        for item in items {
            let tf = if vertical {
                &item.vertical_transform
            } else {
                &item.transform
            };
            draw_item_bgra(target, item, tf, captures);
        }
    }
}

fn clear_bgra(buf: &mut [u8], px: [u8; 4]) {
    for chunk in buf.chunks_exact_mut(4) {
        chunk.copy_from_slice(&px);
    }
}

fn blend_pixel(dst: &mut [u8], src: [u8; 4], alpha_mul: f32) {
    let sa = (src[3] as f32 / 255.0) * alpha_mul;
    if sa <= 0.001 {
        return;
    }
    if sa >= 0.999 {
        dst[0] = src[0];
        dst[1] = src[1];
        dst[2] = src[2];
        dst[3] = 255;
        return;
    }
    let inv = 1.0 - sa;
    dst[0] = (src[0] as f32 * sa + dst[0] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[1] = (src[1] as f32 * sa + dst[1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[2] = (src[2] as f32 * sa + dst[2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
    dst[3] = 255;
}

fn rounded_corner_mask(px: f32, py: f32, w: f32, h: f32, r: f32) -> bool {
    if r <= 0.5 {
        return true;
    }
    let cx = if px < r {
        r - px
    } else if px > w - r {
        px - (w - r)
    } else {
        return true;
    };
    let cy = if py < r {
        r - py
    } else if py > h - r {
        py - (h - r)
    } else {
        return true;
    };
    cx * cx + cy * cy <= r * r
}

fn draw_item_bgra(
    target: &mut FrameBuffer,
    item: &SceneItem,
    tf: &SourceTransform,
    captures: &CaptureManager,
) {
    let cw = target.width as f32;
    let ch = target.height as f32;
    let x0 = (tf.x * cw).round() as i32;
    let y0 = (tf.y * ch).round() as i32;
    let dw = (tf.width * cw).round() as i32;
    let dh = (tf.height * ch).round() as i32;
    if dw <= 0 || dh <= 0 {
        return;
    }
    let opacity = tf.opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }

    let dx_start = x0.max(0);
    let dy_start = y0.max(0);
    let dx_end = (x0 + dw).min(target.width as i32);
    let dy_end = (y0 + dh).min(target.height as i32);
    if dx_start >= dx_end || dy_start >= dy_end {
        return;
    }

    let rounded = item.settings.rounded.unwrap_or(false);
    let radius = if rounded {
        (dw.min(dh) as f32) * 0.06
    } else {
        0.0
    };

    if item.source_type == SourceType::ColorSource {
        let col = parse_hex_bgra(item.settings.color.as_deref(), [0, 0, 0, 255]);
        let stride = target.width as usize * 4;
        for py in dy_start..dy_end {
            let local_y = (py - y0) as f32 + 0.5;
            for px in dx_start..dx_end {
                let local_x = (px - x0) as f32 + 0.5;
                if rounded && !rounded_corner_mask(local_x, local_y, dw as f32, dh as f32, radius) {
                    continue;
                }
                let off = (py as usize) * stride + (px as usize) * 4;
                blend_pixel(&mut target.bgra[off..off + 4], col, opacity);
            }
        }
        return;
    }

    let Some(key) = capture_key_for_item(item) else {
        return;
    };
    let Some(src) = captures.get_frame(&key) else {
        return;
    };
    if src.width == 0 || src.height == 0 {
        return;
    }

    // Aplica crop normalizado (0.0..0.9) sobre a textura fonte
    let cl = tf.crop_left.unwrap_or(0.0).clamp(0.0, 0.9);
    let cr = tf.crop_right.unwrap_or(0.0).clamp(0.0, 0.9);
    let ct = tf.crop_top.unwrap_or(0.0).clamp(0.0, 0.9);
    let cb = tf.crop_bottom.unwrap_or(0.0).clamp(0.0, 0.9);
    if cl + cr >= 0.99 || ct + cb >= 0.99 {
        return;
    }

    let crop_x0 = cl * src.width as f32;
    let crop_y0 = ct * src.height as f32;
    let crop_w = ((1.0 - cl - cr) * src.width as f32).max(1.0);
    let crop_h = ((1.0 - ct - cb) * src.height as f32).max(1.0);

    // Mapeia retângulo de destino conforme fit_for (Contain / Cover / Stretch)
    let (draw_x, draw_y, draw_w, draw_h, sub_x, sub_y, sub_w, sub_h) = match fit_for(item) {
        FitMode::Stretch => (
            x0 as f32,
            y0 as f32,
            dw as f32,
            dh as f32,
            crop_x0,
            crop_y0,
            crop_w,
            crop_h,
        ),
        FitMode::Cover => {
            let scale = (dw as f32 / crop_w).max(dh as f32 / crop_h);
            let sw = (dw as f32) / scale;
            let sh = (dh as f32) / scale;
            (
                x0 as f32,
                y0 as f32,
                dw as f32,
                dh as f32,
                crop_x0 + (crop_w - sw) * 0.5,
                crop_y0 + (crop_h - sh) * 0.5,
                sw,
                sh,
            )
        }
        FitMode::Contain => {
            let scale = (dw as f32 / crop_w).min(dh as f32 / crop_h);
            let rw = crop_w * scale;
            let rh = crop_h * scale;
            (
                x0 as f32 + (dw as f32 - rw) * 0.5,
                y0 as f32 + (dh as f32 - rh) * 0.5,
                rw,
                rh,
                crop_x0,
                crop_y0,
                crop_w,
                crop_h,
            )
        }
    };

    let dst_stride = target.width as usize * 4;
    let src_stride = src.width as usize * 4;

    for py in dy_start..dy_end {
        let fy = py as f32 + 0.5;
        if fy < draw_y || fy >= draw_y + draw_h {
            continue;
        }
        let local_y = (py - y0) as f32 + 0.5;
        let v = (fy - draw_y) / draw_h;
        let sy = ((sub_y + v * sub_h).floor() as i32).clamp(0, (src.height as i32) - 1) as usize;

        for px in dx_start..dx_end {
            let fx = px as f32 + 0.5;
            if fx < draw_x || fx >= draw_x + draw_w {
                continue;
            }
            let local_x = (px - x0) as f32 + 0.5;
            if rounded && !rounded_corner_mask(local_x, local_y, dw as f32, dh as f32, radius) {
                continue;
            }
            let mut u = (fx - draw_x) / draw_w;
            if tf.flip_x {
                u = 1.0 - u;
            }
            let sx = ((sub_x + u * sub_w).floor() as i32).clamp(0, (src.width as i32) - 1) as usize;

            let src_off = sy * src_stride + sx * 4;
            let dst_off = (py as usize) * dst_stride + (px as usize) * 4;
            let pixel = [
                src.bgra[src_off],
                src.bgra[src_off + 1],
                src.bgra[src_off + 2],
                src.bgra[src_off + 3],
            ];
            blend_pixel(&mut target.bgra[dst_off..dst_off + 4], pixel, opacity);
        }
    }
}

fn fade_to_black_buffer(dst: &mut FrameBuffer, src: &FrameBuffer, brightness: f32) {
    let k = brightness.clamp(0.0, 1.0);
    for (d, s) in dst.bgra.chunks_exact_mut(4).zip(src.bgra.chunks_exact(4)) {
        d[0] = (s[0] as f32 * k).round().clamp(0.0, 255.0) as u8;
        d[1] = (s[1] as f32 * k).round().clamp(0.0, 255.0) as u8;
        d[2] = (s[2] as f32 * k).round().clamp(0.0, 255.0) as u8;
        d[3] = 255;
    }
}

/// Slide horizontal: `outgoing` desliza para a esquerda enquanto `incoming` entra pela direita.
fn slide_buffers(dst: &mut FrameBuffer, outgoing: &FrameBuffer, incoming: &FrameBuffer, progress: f32) {
    let p = progress.clamp(0.0, 1.0);
    let w = dst.width as usize;
    let h = dst.height as usize;
    let shift = ((w as f32) * p).round() as usize;
    let split = w.saturating_sub(shift);
    let stride = w * 4;

    for y in 0..h {
        let row_off = y * stride;
        if split > 0 {
            let out_start = shift * 4;
            let len = split * 4;
            dst.bgra[row_off..row_off + len]
                .copy_from_slice(&outgoing.bgra[row_off + out_start..row_off + out_start + len]);
        }
        if shift > 0 {
            let dst_start = row_off + split * 4;
            let len = shift * 4;
            dst.bgra[dst_start..dst_start + len]
                .copy_from_slice(&incoming.bgra[row_off..row_off + len]);
        }
    }
}

/// Desenha uma barra discreta para o Privacy Shield no buffer final (sem dependência de fonte externa).
fn apply_privacy_shield(target: &mut FrameBuffer) {
    clear_bgra(&mut target.bgra, [18, 13, 11, 255]); // #0b0d12 em BGRA
    let w = target.width as usize;
    let h = target.height as usize;
    let bx0 = w / 3;
    let bx1 = (w * 2) / 3;
    let by0 = h / 2 - (h / 40).max(2);
    let by1 = h / 2 + (h / 40).max(2);
    let stride = w * 4;
    for y in by0..by1.min(h) {
        for x in bx0..bx1.min(w) {
            let off = y * stride + x * 4;
            target.bgra[off..off + 4].copy_from_slice(&[249, 245, 243, 255]);
        }
    }
}

/// Downscale rápido por amostragem box/nearest para gerar frames de preview leves (`960x540` e `360x640`).
pub fn downscale_preview(src: &FrameBuffer, dst: &mut FrameBuffer) {
    if src.width == 0 || src.height == 0 || dst.width == 0 || dst.height == 0 {
        return;
    }
    let dst_stride = dst.width as usize * 4;
    let src_stride = src.width as usize * 4;
    for dy in 0..dst.height as usize {
        let sy = (dy * src.height as usize) / dst.height as usize;
        for dx in 0..dst.width as usize {
            let sx = (dx * src.width as usize) / dst.width as usize;
            let s_off = sy * src_stride + sx * 4;
            let d_off = dy * dst_stride + dx * 4;
            dst.bgra[d_off..d_off + 4].copy_from_slice(&src.bgra[s_off..s_off + 4]);
        }
    }
    dst.seq = src.seq;
    dst.updated_at_ms = src.updated_at_ms;
}

/// Motor de Composição Dual Output (16:9 + 9:16) em Rust.
///
/// Correções críticas implementadas:
/// - `live_vertical` é mantido separado de `studio.dual_output_enabled`: alterar
///   configurações (`apply_settings`) durante a live **nunca** desliga ou congela o 9:16!
/// - Transições (`Cut`, `Fade`, `FadeToBlack`, `Slide`) usam buffers offscreen separados;
///   trocar para `Cut` limpa `outgoing` imediatamente.
/// - Frame pacing por acumulador compensado por drift + métricas reais de `TelemetryStats`.
pub struct Compositor<B: CompositorBackend = SoftwareBackend> {
    backend: B,
    studio: StudioCompositorSettings,
    live_vertical: bool,

    program_scene: Option<Scene>,
    preview_scene: Option<Scene>,
    outgoing_scene: Option<Scene>,
    transition_type: TransitionType,
    transition_duration_ms: u32,
    transition_started_ms: u64,

    pub program: FrameBuffer,
    pub vertical: FrameBuffer,
    pub preview: FrameBuffer,
    pub preview_small_h: FrameBuffer,
    pub preview_small_v: FrameBuffer,

    scratch_h: FrameBuffer,
    scratch_v: FrameBuffer,

    // Frame pacing e telemetria
    next_tick_due_ms: Option<f64>,
    render_samples_ms: VecDeque<f32>,
    tick_timestamps_ms: VecDeque<u64>,
    dropped_frames: u64,
    telemetry: TelemetryStats,
}

impl Compositor<SoftwareBackend> {
    pub fn new(studio: StudioCompositorSettings) -> Self {
        Self::with_dimensions(studio, CANVAS_H, CANVAS_V, SoftwareBackend)
    }
}

impl<B: CompositorBackend> Compositor<B> {
    pub fn with_dimensions(
        mut studio: StudioCompositorSettings,
        horiz: (u32, u32),
        vert: (u32, u32),
        backend: B,
    ) -> Self {
        studio.normalize();
        Self {
            backend,
            live_vertical: false,
            program_scene: None,
            preview_scene: None,
            outgoing_scene: None,
            transition_type: TransitionType::Fade,
            transition_duration_ms: 300,
            transition_started_ms: 0,
            program: FrameBuffer::new(horiz.0, horiz.1),
            vertical: FrameBuffer::new(vert.0, vert.1),
            preview: FrameBuffer::new(PREVIEW_H.0, PREVIEW_H.1),
            preview_small_h: FrameBuffer::new(PREVIEW_H.0, PREVIEW_H.1),
            preview_small_v: FrameBuffer::new(PREVIEW_V.0, PREVIEW_V.1),
            scratch_h: FrameBuffer::new(horiz.0, horiz.1),
            scratch_v: FrameBuffer::new(vert.0, vert.1),
            next_tick_due_ms: None,
            render_samples_ms: VecDeque::with_capacity(120),
            tick_timestamps_ms: VecDeque::with_capacity(120),
            dropped_frames: 0,
            telemetry: TelemetryStats {
                target_fps: studio.fps,
                ..Default::default()
            },
            studio,
        }
    }

    pub fn set_live_vertical(&mut self, active: bool) {
        self.live_vertical = active;
    }

    /// Indica se o pipeline 9:16 deve renderizar (ou porque o usuário ligou preview dual,
    /// ou porque há transmissão vertical ativa no ar).
    pub fn is_vertical_active(&self) -> bool {
        self.studio.dual_output_enabled || self.live_vertical
    }

    /// Atualiza configurações do estúdio **preservando** `live_vertical` caso a live 9:16 esteja ativa.
    pub fn apply_settings(&mut self, mut next: StudioCompositorSettings) {
        next.normalize();
        if next.fps != self.studio.fps {
            self.next_tick_due_ms = None;
        }
        self.studio = next;
        self.telemetry.target_fps = self.studio.fps;
    }

    pub fn studio_settings(&self) -> &StudioCompositorSettings {
        &self.studio
    }

    pub fn set_scenes(
        &mut self,
        program: Option<Scene>,
        preview: Option<Scene>,
        transition_type: TransitionType,
        duration_ms: u32,
        now_ms: u64,
    ) {
        let same_scene = match (&self.program_scene, &program) {
            (Some(a), Some(b)) => a.id == b.id,
            _ => false,
        };

        if transition_type == TransitionType::Cut {
            // Correção #9: Cut cancela qualquer transição em andamento imediatamente.
            self.outgoing_scene = None;
        } else if !same_scene {
            if let (Some(prev), Some(_)) = (&self.program_scene, &program) {
                self.outgoing_scene = Some(prev.clone());
                self.transition_type = transition_type;
                self.transition_duration_ms = duration_ms.clamp(50, 3000);
                self.transition_started_ms = now_ms;
            }
        }

        self.program_scene = program;
        self.preview_scene = preview;
    }

    /// Frame pacing compensado por drift: retorna quantos ticks estão devidos e atualiza
    /// contagem de `dropped_frames` se um quadro atrasou mais que 1.5 intervalos.
    pub fn should_render_at(&mut self, now_ms: f64) -> bool {
        let interval = 1000.0 / (self.studio.fps.max(1) as f64);
        let Some(due) = self.next_tick_due_ms else {
            self.next_tick_due_ms = Some(now_ms + interval);
            return true;
        };
        if now_ms + 0.25 < due {
            return false;
        }
        let behind = now_ms - due;
        if behind > interval * 1.5 {
            let skipped = (behind / interval).floor() as u64;
            self.dropped_frames = self.dropped_frames.saturating_add(skipped);
            self.next_tick_due_ms = Some(now_ms + interval);
        } else {
            self.next_tick_due_ms = Some(due + interval);
        }
        true
    }

    /// Renderiza um tick completo (Program 16:9, opcionalmente Vertical 9:16 e Preview)
    /// e atualiza as métricas em `TelemetryStats`.
    pub fn tick(
        &mut self,
        captures: &CaptureManager,
        now_ms: u64,
        measured_render_ms: Option<f32>,
    ) {
        let started = std::time::Instant::now();

        let mut progress = 1.0f32;
        if self.outgoing_scene.is_some() {
            let elapsed = now_ms.saturating_sub(self.transition_started_ms) as f32;
            let dur = self.transition_duration_ms.max(50) as f32;
            progress = (elapsed / dur).clamp(0.0, 1.0);
            if progress >= 1.0 {
                self.outgoing_scene = None;
            }
        }

        Self::render_orientation(
            &mut self.backend,
            &mut self.program,
            &mut self.scratch_h,
            self.program_scene.as_ref(),
            self.outgoing_scene.as_ref(),
            false,
            self.transition_type,
            progress,
            captures,
            &self.studio,
        );
        self.program.seq = self.program.seq.saturating_add(1);
        self.program.updated_at_ms = now_ms;
        downscale_preview(&self.program, &mut self.preview_small_h);

        if self.is_vertical_active() {
            Self::render_orientation(
                &mut self.backend,
                &mut self.vertical,
                &mut self.scratch_v,
                self.program_scene.as_ref(),
                self.outgoing_scene.as_ref(),
                true,
                self.transition_type,
                progress,
                captures,
                &self.studio,
            );
            self.vertical.seq = self.vertical.seq.saturating_add(1);
            self.vertical.updated_at_ms = now_ms;
            downscale_preview(&self.vertical, &mut self.preview_small_v);
        }

        if let Some(ref prev) = self.preview_scene {
            self.backend
                .compose_scene(&mut self.preview, prev, false, captures, &self.studio);
            self.preview.seq = self.preview.seq.saturating_add(1);
            self.preview.updated_at_ms = now_ms;
        }

        let elapsed_ms = measured_render_ms
            .unwrap_or_else(|| started.elapsed().as_secs_f32() * 1000.0);
        self.record_telemetry(now_ms, elapsed_ms);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_orientation(
        backend: &mut B,
        target: &mut FrameBuffer,
        scratch: &mut FrameBuffer,
        program: Option<&Scene>,
        outgoing: Option<&Scene>,
        vertical: bool,
        transition: TransitionType,
        progress: f32,
        captures: &CaptureManager,
        studio: &StudioCompositorSettings,
    ) {
        if studio.privacy_shield_active {
            apply_privacy_shield(target);
            return;
        }

        let Some(prog) = program else {
            clear_bgra(&mut target.bgra, [0, 0, 0, 255]);
            return;
        };

        if let (Some(out), true) = (outgoing, progress < 1.0) {
            match transition {
                TransitionType::Cut => {
                    backend.compose_scene(target, prog, vertical, captures, studio);
                }
                TransitionType::Fade => {
                    backend.compose_scene(scratch, out, vertical, captures, studio);
                    backend.compose_scene(target, prog, vertical, captures, studio);
                    let t = progress.clamp(0.0, 1.0);
                    let inv = 1.0 - t;
                    for (d, s_out) in target
                        .bgra
                        .chunks_exact_mut(4)
                        .zip(scratch.bgra.chunks_exact(4))
                    {
                        d[0] = (s_out[0] as f32 * inv + d[0] as f32 * t).round().clamp(0.0, 255.0) as u8;
                        d[1] = (s_out[1] as f32 * inv + d[1] as f32 * t).round().clamp(0.0, 255.0) as u8;
                        d[2] = (s_out[2] as f32 * inv + d[2] as f32 * t).round().clamp(0.0, 255.0) as u8;
                        d[3] = 255;
                    }
                }
                TransitionType::FadeToBlack => {
                    if progress < 0.5 {
                        backend.compose_scene(scratch, out, vertical, captures, studio);
                        fade_to_black_buffer(target, scratch, 1.0 - progress * 2.0);
                    } else {
                        backend.compose_scene(scratch, prog, vertical, captures, studio);
                        fade_to_black_buffer(target, scratch, (progress - 0.5) * 2.0);
                    }
                }
                TransitionType::Slide => {
                    let mut incoming_tmp = FrameBuffer::new(target.width, target.height);
                    backend.compose_scene(scratch, out, vertical, captures, studio);
                    backend.compose_scene(&mut incoming_tmp, prog, vertical, captures, studio);
                    slide_buffers(target, scratch, &incoming_tmp, progress);
                }
            }
            return;
        }

        backend.compose_scene(target, prog, vertical, captures, studio);
    }

    fn record_telemetry(&mut self, now_ms: u64, render_ms: f32) {
        if self.render_samples_ms.len() >= 120 {
            self.render_samples_ms.pop_front();
        }
        self.render_samples_ms.push_back(render_ms);

        self.tick_timestamps_ms.push_back(now_ms);
        while let Some(&front) = self.tick_timestamps_ms.front() {
            if now_ms.saturating_sub(front) > 1000 {
                self.tick_timestamps_ms.pop_front();
            } else {
                break;
            }
        }

        let mut sorted: Vec<f32> = self.render_samples_ms.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p95 = if sorted.is_empty() {
            0.0
        } else {
            let idx = ((sorted.len() as f32) * 0.95).floor() as usize;
            sorted[idx.min(sorted.len() - 1)]
        };

        let fps = if self.tick_timestamps_ms.len() >= 2 {
            let span_ms = self
                .tick_timestamps_ms
                .back()
                .unwrap()
                .saturating_sub(*self.tick_timestamps_ms.front().unwrap())
                .max(1);
            ((self.tick_timestamps_ms.len() - 1) as f32 * 1000.0) / (span_ms as f32)
        } else {
            self.studio.fps as f32
        };

        self.telemetry.compositor_fps = fps;
        self.telemetry.target_fps = self.studio.fps;
        self.telemetry.render_ms_p95 = p95;
        self.telemetry.dropped_frames = self.dropped_frames;
    }

    pub fn telemetry(&self) -> TelemetryStats {
        self.telemetry.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SourceSettings;

    fn make_color_item(id: &str, color: &str, z: i32, vz: i32) -> SceneItem {
        SceneItem {
            id: id.to_string(),
            source_id: id.to_string(),
            name: id.to_string(),
            source_type: SourceType::ColorSource,
            visible: true,
            locked: false,
            transform: SourceTransform {
                z_index: z,
                ..Default::default()
            },
            vertical_transform: SourceTransform {
                z_index: vz,
                ..Default::default()
            },
            show_on_horizontal: true,
            show_on_vertical: true,
            settings: SourceSettings {
                color: Some(color.to_string()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_live_vertical_never_freezes_after_apply_settings() {
        let mut studio = StudioCompositorSettings::default();
        studio.dual_output_enabled = false;
        let mut comp = Compositor::with_dimensions(studio.clone(), (16, 9), (9, 16), SoftwareBackend);
        let captures = CaptureManager::new(60);

        let scene_a = Scene {
            id: "a".to_string(),
            name: "A".to_string(),
            items: vec![make_color_item("bg", "#ff0000", 0, 0)],
            created_at: 0,
        };
        let scene_b = Scene {
            id: "b".to_string(),
            name: "B".to_string(),
            items: vec![make_color_item("bg2", "#0000ff", 0, 0)],
            created_at: 0,
        };

        comp.set_live_vertical(true);
        comp.set_scenes(Some(scene_a), None, TransitionType::Cut, 300, 0);
        comp.tick(&captures, 16, Some(1.0));
        assert_eq!(&comp.vertical.bgra[0..4], &[0, 0, 255, 255]); // vermelho BGRA

        // Simula usuário alterando alguma config com dual_output_enabled = false
        let mut updated = studio;
        updated.dual_output_enabled = false;
        updated.camera_enabled = false;
        comp.apply_settings(updated);
        assert!(comp.is_vertical_active());

        comp.set_scenes(Some(scene_b), None, TransitionType::Cut, 300, 32);
        comp.tick(&captures, 32, Some(1.0));
        // Vertical precisa continuar atualizando para azul!
        assert_eq!(&comp.vertical.bgra[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn test_crop_flip_x_and_opacity_pixels() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (4, 4), (4, 4), SoftwareBackend);
        let mut captures = CaptureManager::new(60);

        // Imagem 4x4: metade esquerda verde (#00ff00), metade direita azul (#0000ff)
        let mut bgra = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let off = (y * 4 + x) * 4;
                if x < 2 {
                    bgra[off..off + 4].copy_from_slice(&[0, 255, 0, 255]);
                } else {
                    bgra[off..off + 4].copy_from_slice(&[255, 0, 0, 255]);
                }
            }
        }
        captures.push_full_frame("camera:default", 4, 4, bgra, 10).unwrap();

        // Crop 50% esquerdo (fica só o lado azul) + flip_x + opacity 0.5 sobre fundo preto
        let cam = SceneItem {
            id: "cam".to_string(),
            source_id: "cam".to_string(),
            name: "Cam".to_string(),
            source_type: SourceType::CameraInput,
            visible: true,
            locked: false,
            transform: SourceTransform {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
                z_index: 0,
                crop_left: Some(0.5),
                flip_x: true,
                opacity: 0.5,
                ..Default::default()
            },
            vertical_transform: SourceTransform::default(),
            show_on_horizontal: true,
            show_on_vertical: true,
            settings: SourceSettings::default(),
        };

        let scene = Scene {
            id: "s".to_string(),
            name: "S".to_string(),
            items: vec![cam],
            created_at: 0,
        };
        comp.set_scenes(Some(scene), None, TransitionType::Cut, 200, 0);
        comp.tick(&captures, 16, Some(1.0));

        // Blue = 255 * 0.5 ≈ 128, Green = 0, Red = 0
        assert!((comp.program.bgra[0] as i32 - 128).abs() <= 1);
        assert_eq!(comp.program.bgra[1], 0);
        assert_eq!(comp.program.bgra[2], 0);
    }

    #[test]
    fn test_transitions_cut_cancels_fade_and_fade_to_black_and_slide() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (4, 2), (2, 4), SoftwareBackend);
        let captures = CaptureManager::new(60);

        let s_white = Scene {
            id: "w".to_string(),
            name: "White".to_string(),
            items: vec![make_color_item("w", "#ffffff", 0, 0)],
            created_at: 0,
        };
        let s_red = Scene {
            id: "r".to_string(),
            name: "Red".to_string(),
            items: vec![make_color_item("r", "#ff0000", 0, 0)],
            created_at: 0,
        };
        let s_green = Scene {
            id: "g".to_string(),
            name: "Green".to_string(),
            items: vec![make_color_item("g", "#00ff00", 0, 0)],
            created_at: 0,
        };

        // 1. Inicia FadeToBlack de White -> Red (dur 1000ms). Em t=500ms deve estar preto!
        comp.set_scenes(Some(s_white.clone()), None, TransitionType::Cut, 1000, 0);
        comp.set_scenes(Some(s_red.clone()), None, TransitionType::FadeToBlack, 1000, 100);
        comp.tick(&captures, 600, Some(1.0)); // progress = 0.5 -> brilho 0
        assert_eq!(&comp.program.bgra[0..4], &[0, 0, 0, 255]);

        // 2. Cut durante transição ativa deve cancelar outgoing imediatamente
        comp.set_scenes(Some(s_green), None, TransitionType::Cut, 1000, 620);
        comp.tick(&captures, 636, Some(1.0));
        assert_eq!(&comp.program.bgra[0..4], &[0, 255, 0, 255]);

        // 3. Slide em t=0.5 numa tela 4x2: 2 colunas da esquerda = outgoing (White), 2 da direita = incoming (Red)
        comp.set_scenes(Some(s_white), None, TransitionType::Cut, 1000, 700);
        comp.set_scenes(Some(s_red), None, TransitionType::Slide, 1000, 800);
        comp.tick(&captures, 1300, Some(1.0)); // progress = 0.5
        assert_eq!(&comp.program.bgra[0..4], &[255, 255, 255, 255]); // x=0: White
        assert_eq!(&comp.program.bgra[8..12], &[0, 0, 255, 255]); // x=2: Red
    }

    #[test]
    fn test_independent_vertical_z_order_and_telemetry() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (4, 4), (4, 4), SoftwareBackend);
        let captures = CaptureManager::new(60);

        // Horizontal: Red (z=0) atrás de Blue (z=1). Vertical: Blue (vz=0) atrás de Red (vz=1)!
        let mut scene = Scene {
            id: "z".to_string(),
            name: "Z".to_string(),
            items: vec![
                make_color_item("red", "#ff0000", 0, 1),
                make_color_item("blue", "#0000ff", 1, 0),
            ],
            created_at: 0,
        };
        normalize_z(&mut scene, true);
        assert_eq!(scene.items[0].vertical_transform.z_index, 1);
        assert_eq!(scene.items[1].vertical_transform.z_index, 0);

        comp.set_live_vertical(true);
        comp.set_scenes(Some(scene), None, TransitionType::Cut, 200, 0);

        assert!(comp.should_render_at(0.0));
        assert!(!comp.should_render_at(5.0)); // muito cedo para 60fps
        assert!(comp.should_render_at(100.0)); // simulando drift grande -> renderiza e conta drop
        comp.tick(&captures, 16, Some(2.5));
        comp.tick(&captures, 33, Some(12.0));

        // Horizontal termina com Blue no topo; Vertical termina com Red no topo
        assert_eq!(&comp.program.bgra[0..4], &[255, 0, 0, 255]);
        assert_eq!(&comp.vertical.bgra[0..4], &[0, 0, 255, 255]);

        let tel = comp.telemetry();
        assert_eq!(tel.target_fps, 60);
        assert!(tel.render_ms_p95 >= 11.9);
        assert!(tel.dropped_frames >= 1);
        assert!(tel.compositor_fps > 0.0);
    }
}

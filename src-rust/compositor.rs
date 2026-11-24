use crate::capture::{CaptureManager, FrameBuffer};
use crate::gpu_compositor::{eval_luma_mask, eval_luma_preset, WgpuBackend};
use crate::protocol::{
    CustomShaderPreset, CustomTransitionConfig, GpuPipelineStatus, Scene, SceneItem,
    SourceTransform, SourceType, StudioCompositorSettings, TelemetryStats, TransitionType,
};
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

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

/// Layout normalizado de uma camada pronto para consumo pelo shader `LAYER_COMPOSITE_WGSL`.
#[derive(Debug, Clone)]
pub(crate) struct ComputedLayerLayout {
    pub dst_rect: [f32; 4],
    pub uv_rect: [f32; 4],
    pub color_rgba: [f32; 4],
    pub opacity: f32,
    pub flip_x: bool,
    pub corner_radius_px: f32,
    pub is_solid_color: bool,
    pub rect_px: [f32; 4],
    pub draw_px: [f32; 4],
    pub source_key: Option<String>,
}

/// Calcula todas as camadas visíveis ordenadas por `z_index` com crop, `FitMode` e cantos arredondados
/// para o backend GPU `wgpu` com a mesma semântica exata do `SoftwareBackend`.
pub(crate) fn compute_scene_layers(
    width: u32,
    height: u32,
    scene: &Scene,
    vertical: bool,
    captures: &CaptureManager,
    studio: &StudioCompositorSettings,
) -> Vec<ComputedLayerLayout> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let cw = width as f32;
    let ch = height as f32;

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

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let tf = if vertical {
            &item.vertical_transform
        } else {
            &item.transform
        };
        let x0 = (tf.x * cw).round();
        let y0 = (tf.y * ch).round();
        let dw = (tf.width * cw).round();
        let dh = (tf.height * ch).round();
        if dw <= 0.0 || dh <= 0.0 {
            continue;
        }
        let opacity = tf.opacity.clamp(0.0, 1.0);
        if opacity <= 0.001 {
            continue;
        }
        let rounded = item.settings.rounded.unwrap_or(false);
        let corner_radius_px = if rounded { dw.min(dh) * 0.06 } else { 0.0 };

        if item.source_type == SourceType::ColorSource {
            let col = parse_hex_bgra(item.settings.color.as_deref(), [0, 0, 0, 255]);
            out.push(ComputedLayerLayout {
                dst_rect: [x0 / cw, y0 / ch, dw / cw, dh / ch],
                uv_rect: [0.0, 0.0, 1.0, 1.0],
                color_rgba: [
                    col[2] as f32 / 255.0,
                    col[1] as f32 / 255.0,
                    col[0] as f32 / 255.0,
                    col[3] as f32 / 255.0,
                ],
                opacity,
                flip_x: false,
                corner_radius_px,
                is_solid_color: true,
                rect_px: [dw, dh, 0.0, 0.0],
                draw_px: [dw, dh, 0.0, 0.0],
                source_key: None,
            });
            continue;
        }

        let Some(key) = capture_key_for_item(item) else {
            continue;
        };
        let Some(src) = captures.get_frame(&key) else {
            continue;
        };
        if src.width == 0 || src.height == 0 {
            continue;
        }

        let cl = tf.crop_left.unwrap_or(0.0).clamp(0.0, 0.9);
        let cr = tf.crop_right.unwrap_or(0.0).clamp(0.0, 0.9);
        let ct = tf.crop_top.unwrap_or(0.0).clamp(0.0, 0.9);
        let cb = tf.crop_bottom.unwrap_or(0.0).clamp(0.0, 0.9);
        if cl + cr >= 0.99 || ct + cb >= 0.99 {
            continue;
        }

        let sw = src.width as f32;
        let sh = src.height as f32;
        let crop_x0 = cl * sw;
        let crop_y0 = ct * sh;
        let crop_w = ((1.0 - cl - cr) * sw).max(1.0);
        let crop_h = ((1.0 - ct - cb) * sh).max(1.0);

        let (draw_x, draw_y, draw_w, draw_h, sub_x, sub_y, sub_w, sub_h) = match fit_for(item) {
            FitMode::Stretch => (x0, y0, dw, dh, crop_x0, crop_y0, crop_w, crop_h),
            FitMode::Cover => {
                let scale = (dw / crop_w).max(dh / crop_h);
                let vw = dw / scale;
                let vh = dh / scale;
                (
                    x0,
                    y0,
                    dw,
                    dh,
                    crop_x0 + (crop_w - vw) * 0.5,
                    crop_y0 + (crop_h - vh) * 0.5,
                    vw,
                    vh,
                )
            }
            FitMode::Contain => {
                let scale = (dw / crop_w).min(dh / crop_h);
                let rw = crop_w * scale;
                let rh = crop_h * scale;
                (
                    x0 + (dw - rw) * 0.5,
                    y0 + (dh - rh) * 0.5,
                    rw,
                    rh,
                    crop_x0,
                    crop_y0,
                    crop_w,
                    crop_h,
                )
            }
        };

        out.push(ComputedLayerLayout {
            dst_rect: [draw_x / cw, draw_y / ch, draw_w / cw, draw_h / ch],
            uv_rect: [sub_x / sw, sub_y / sh, sub_w / sw, sub_h / sh],
            color_rgba: [1.0, 1.0, 1.0, 1.0],
            opacity,
            flip_x: tf.flip_x,
            corner_radius_px,
            is_solid_color: false,
            rect_px: [dw, dh, draw_x - x0, draw_y - y0],
            draw_px: [draw_w, draw_h, 0.0, 0.0],
            source_key: Some(key),
        });
    }

    out
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

    /// Executa transição entre `outgoing` e `incoming` gravando em `target`.
    /// O padrão executa o motor CPU (`apply_transition_software`); o `WgpuBackend` executa em shader WGSL.
    #[allow(clippy::too_many_arguments)]
    fn compose_transition(
        &mut self,
        target: &mut FrameBuffer,
        outgoing: &FrameBuffer,
        incoming: &FrameBuffer,
        transition: TransitionType,
        progress: f32,
        elapsed_ms: u64,
        custom: &CustomTransitionConfig,
        stinger_frame: Option<&FrameBuffer>,
        luma_matte: Option<&FrameBuffer>,
    ) {
        apply_transition_software(
            target,
            outgoing,
            incoming,
            transition,
            progress,
            elapsed_ms,
            custom,
            stinger_frame,
            luma_matte,
        );
    }

    /// Retorna `(backend_name, adapter_name, driver_info)`.
    fn backend_info(&self) -> (String, String, String) {
        (
            "software".to_string(),
            "CPU BGRA".to_string(),
            "PrismCast Software Compositor".to_string(),
        )
    }

    /// Quantidade de texturas ativas em cache GPU (0 para software).
    fn active_textures(&self) -> usize {
        0
    }
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

#[inline]
fn sample_bgra_nearest(src: &FrameBuffer, u: f32, v: f32) -> [u8; 4] {
    if src.width == 0 || src.height == 0 {
        return [0, 0, 0, 255];
    }
    let sx = ((u.clamp(0.0, 1.0) * (src.width as f32 - 1.0)).round() as usize)
        .min((src.width as usize) - 1);
    let sy = ((v.clamp(0.0, 1.0) * (src.height as f32 - 1.0)).round() as usize)
        .min((src.height as usize) - 1);
    let off = (sy * (src.width as usize) + sx) * 4;
    if off + 4 > src.bgra.len() {
        return [0, 0, 0, 255];
    }
    [
        src.bgra[off],
        src.bgra[off + 1],
        src.bgra[off + 2],
        src.bgra[off + 3],
    ]
}

#[inline]
fn smoothstep_f32(edge0: f32, edge1: f32, x: f32) -> f32 {
    let span = (edge1 - edge0).max(1e-6);
    let t = ((x - edge0) / span).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp_bgra(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let k = t.clamp(0.0, 1.0);
    let inv = 1.0 - k;
    [
        (a[0] as f32 * inv + b[0] as f32 * k).round().clamp(0.0, 255.0) as u8,
        (a[1] as f32 * inv + b[1] as f32 * k).round().clamp(0.0, 255.0) as u8,
        (a[2] as f32 * inv + b[2] as f32 * k).round().clamp(0.0, 255.0) as u8,
        255,
    ]
}

/// Motor de transições em software/CPU suportando todos os modos do `WgpuBackend`:
/// `Cut`, `Fade`, `FadeToBlack`, `Slide`, `Stinger` (alpha overlay + cut point),
/// `LumaWipe` (matte customizado ou presets procedurais com softness/invert) e
/// `CustomShader` (`ZoomBlur`, `RadialWipe`, `GlitchCut`, `Swirl`, fallback para `CustomWgsl`).
#[allow(clippy::too_many_arguments)]
pub fn apply_transition_software(
    target: &mut FrameBuffer,
    outgoing: &FrameBuffer,
    incoming: &FrameBuffer,
    transition: TransitionType,
    progress: f32,
    elapsed_ms: u64,
    custom: &CustomTransitionConfig,
    stinger_frame: Option<&FrameBuffer>,
    luma_matte: Option<&FrameBuffer>,
) {
    let t = progress.clamp(0.0, 1.0);
    match transition {
        TransitionType::Cut => {
            target.bgra.copy_from_slice(&incoming.bgra);
        }
        TransitionType::Fade => {
            let inv = 1.0 - t;
            for ((d, s_out), s_in) in target
                .bgra
                .chunks_exact_mut(4)
                .zip(outgoing.bgra.chunks_exact(4))
                .zip(incoming.bgra.chunks_exact(4))
            {
                d[0] = (s_out[0] as f32 * inv + s_in[0] as f32 * t)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                d[1] = (s_out[1] as f32 * inv + s_in[1] as f32 * t)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                d[2] = (s_out[2] as f32 * inv + s_in[2] as f32 * t)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                d[3] = 255;
            }
        }
        TransitionType::FadeToBlack => {
            if t < 0.5 {
                fade_to_black_buffer(target, outgoing, 1.0 - t * 2.0);
            } else {
                fade_to_black_buffer(target, incoming, (t - 0.5) * 2.0);
            }
        }
        TransitionType::Slide => {
            slide_buffers(target, outgoing, incoming, t);
        }
        TransitionType::Stinger => {
            let cut_reached = elapsed_ms >= custom.stinger_transition_point_ms as u64;
            let base = if cut_reached { incoming } else { outgoing };
            target.bgra.copy_from_slice(&base.bgra);

            if let Some(stinger) = stinger_frame {
                if stinger.width > 0 && stinger.height > 0 && target.width > 0 && target.height > 0
                {
                    let w = target.width as usize;
                    let h = target.height as usize;
                    let sw = stinger.width as usize;
                    let sh = stinger.height as usize;
                    for py in 0..h {
                        let sy = (py * sh) / h;
                        for px in 0..w {
                            let sx = (px * sw) / w;
                            let s_off = (sy * sw + sx) * 4;
                            if s_off + 4 <= stinger.bgra.len() {
                                let spx = [
                                    stinger.bgra[s_off],
                                    stinger.bgra[s_off + 1],
                                    stinger.bgra[s_off + 2],
                                    stinger.bgra[s_off + 3],
                                ];
                                let d_off = (py * w + px) * 4;
                                blend_pixel(&mut target.bgra[d_off..d_off + 4], spx, 1.0);
                            }
                        }
                    }
                }
            }
        }
        TransitionType::LumaWipe => {
            let w = target.width as usize;
            let h = target.height as usize;
            if w == 0 || h == 0 {
                return;
            }
            let wf = w as f32;
            let hf = h as f32;
            let valid_matte = luma_matte.filter(|m| m.width > 0 && m.height > 0);
            for py in 0..h {
                let v = (py as f32 + 0.5) / hf;
                for px in 0..w {
                    let u = (px as f32 + 0.5) / wf;
                    let luma = if let Some(matte) = valid_matte {
                        let m = sample_bgra_nearest(matte, u, v);
                        (0.114 * (m[0] as f32) + 0.587 * (m[1] as f32) + 0.299 * (m[2] as f32))
                            / 255.0
                    } else {
                        eval_luma_preset(&custom.luma_preset, u, v)
                    };
                    let mask = eval_luma_mask(luma, t, custom.luma_softness, custom.luma_invert);
                    let off = (py * w + px) * 4;
                    let a = [
                        outgoing.bgra[off],
                        outgoing.bgra[off + 1],
                        outgoing.bgra[off + 2],
                        255,
                    ];
                    let b = [
                        incoming.bgra[off],
                        incoming.bgra[off + 1],
                        incoming.bgra[off + 2],
                        255,
                    ];
                    target.bgra[off..off + 4].copy_from_slice(&lerp_bgra(a, b, mask));
                }
            }
        }
        TransitionType::CustomShader => {
            let w = target.width as usize;
            let h = target.height as usize;
            if w == 0 || h == 0 {
                return;
            }
            let wf = w as f32;
            let hf = h as f32;
            match custom.shader_preset {
                CustomShaderPreset::ZoomBlur => {
                    let strength = (t * PI).sin() * 0.18;
                    let blend_k = smoothstep_f32(0.25, 0.75, t);
                    for py in 0..h {
                        let v = (py as f32 + 0.5) / hf;
                        let dy = v - 0.5;
                        for px in 0..w {
                            let u = (px as f32 + 0.5) / wf;
                            let dx = u - 0.5;
                            let mut acc_out = [0.0f32; 3];
                            let mut acc_in = [0.0f32; 3];
                            for i in 0..5 {
                                let frac = (i as f32) * 0.25;
                                let s_out = 1.0 - strength * frac;
                                let s_in = 1.0 + strength * (1.0 - frac);
                                let po = sample_bgra_nearest(outgoing, 0.5 + dx * s_out, 0.5 + dy * s_out);
                                let pi = sample_bgra_nearest(incoming, 0.5 + dx * s_in, 0.5 + dy * s_in);
                                for c in 0..3 {
                                    acc_out[c] += po[c] as f32;
                                    acc_in[c] += pi[c] as f32;
                                }
                            }
                            let out_px = [
                                (acc_out[0] * 0.2).round().clamp(0.0, 255.0) as u8,
                                (acc_out[1] * 0.2).round().clamp(0.0, 255.0) as u8,
                                (acc_out[2] * 0.2).round().clamp(0.0, 255.0) as u8,
                                255,
                            ];
                            let in_px = [
                                (acc_in[0] * 0.2).round().clamp(0.0, 255.0) as u8,
                                (acc_in[1] * 0.2).round().clamp(0.0, 255.0) as u8,
                                (acc_in[2] * 0.2).round().clamp(0.0, 255.0) as u8,
                                255,
                            ];
                            let off = (py * w + px) * 4;
                            target.bgra[off..off + 4].copy_from_slice(&lerp_bgra(out_px, in_px, blend_k));
                        }
                    }
                }
                CustomShaderPreset::RadialWipe => {
                    for py in 0..h {
                        let v = (py as f32 + 0.5) / hf;
                        let dy = v - 0.5;
                        for px in 0..w {
                            let u = (px as f32 + 0.5) / wf;
                            let dx = u - 0.5;
                            let mut ang = dx.atan2(-dy);
                            if ang < 0.0 {
                                ang += TAU;
                            }
                            let norm_ang = ang / TAU;
                            let mask = eval_luma_mask(norm_ang, t, 0.03, false);
                            let off = (py * w + px) * 4;
                            let a = [
                                outgoing.bgra[off],
                                outgoing.bgra[off + 1],
                                outgoing.bgra[off + 2],
                                255,
                            ];
                            let b = [
                                incoming.bgra[off],
                                incoming.bgra[off + 1],
                                incoming.bgra[off + 2],
                                255,
                            ];
                            target.bgra[off..off + 4].copy_from_slice(&lerp_bgra(a, b, mask));
                        }
                    }
                }
                CustomShaderPreset::GlitchCut => {
                    let g = (t * PI).sin();
                    let ca = g * 0.025;
                    for py in 0..h {
                        let v = (py as f32 + 0.5) / hf;
                        let band = (v * 24.0).floor();
                        let seed = band * 127.1 + (t * 17.0).floor() * 311.7;
                        let h_hash = (seed.sin() * 43758.5453).fract().abs() * 2.0 - 1.0;
                        let shift = h_hash * g * 0.08;
                        let use_in = (t + h_hash * 0.15) >= 0.5;
                        let src = if use_in { incoming } else { outgoing };
                        for px in 0..w {
                            let u = (px as f32 + 0.5) / wf;
                            let pr = sample_bgra_nearest(src, u + shift + ca, v);
                            let pg = sample_bgra_nearest(src, u + shift, v);
                            let pb = sample_bgra_nearest(src, u + shift - ca, v);
                            let off = (py * w + px) * 4;
                            // BGRA layout: B from pb[0], G from pg[1], R from pr[2]
                            target.bgra[off..off + 4].copy_from_slice(&[pb[0], pg[1], pr[2], 255]);
                        }
                    }
                }
                CustomShaderPreset::Swirl => {
                    let twist_peak = (t * PI).sin() * 5.0;
                    for py in 0..h {
                        let v = (py as f32 + 0.5) / hf;
                        let dy = v - 0.5;
                        for px in 0..w {
                            let u = (px as f32 + 0.5) / wf;
                            let dx = u - 0.5;
                            let dist = (dx * dx + dy * dy).sqrt();
                            let theta = twist_peak * (1.0 - dist).max(0.0);
                            let c1 = theta.cos();
                            let s1 = theta.sin();
                            let out_px = sample_bgra_nearest(
                                outgoing,
                                dx * c1 - dy * s1 + 0.5,
                                dx * s1 + dy * c1 + 0.5,
                            );
                            let in_px = sample_bgra_nearest(
                                incoming,
                                dx * c1 + dy * s1 + 0.5,
                                -dx * s1 + dy * c1 + 0.5,
                            );
                            let off = (py * w + px) * 4;
                            target.bgra[off..off + 4].copy_from_slice(&lerp_bgra(out_px, in_px, t));
                        }
                    }
                }
                CustomShaderPreset::CustomWgsl => {
                    // Fallback limpo em CPU quando um shader WGSL customizado roda sem GPU
                    apply_transition_software(
                        target,
                        outgoing,
                        incoming,
                        TransitionType::Fade,
                        t,
                        elapsed_ms,
                        custom,
                        None,
                        None,
                    );
                }
            }
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

/// Motor de Composição Dual Output (16:9 + 9:16) em Rust com suporte a `wgpu` GPU,
/// Stinger alpha video, Luma Wipe, Shaders WGSL customizados, Easing e Overrides por cena.
///
/// Correções críticas implementadas:
/// - `live_vertical` é mantido separado de `studio.dual_output_enabled`: alterar
///   configurações (`apply_settings`) durante a live **nunca** desliga ou congela o 9:16!
/// - Transições (`Cut`, `Fade`, `FadeToBlack`, `Slide`, `Stinger`, `LumaWipe`, `CustomShader`)
///   usam buffers offscreen reutilizáveis; trocar para `Cut` limpa `outgoing` imediatamente.
/// - Frame pacing por acumulador compensado por drift + métricas reais de `TelemetryStats`.
pub struct Compositor<B: CompositorBackend = SoftwareBackend> {
    backend: B,
    gpu_backend: Option<Box<WgpuBackend>>,
    studio: StudioCompositorSettings,
    live_vertical: bool,

    program_scene: Option<Scene>,
    preview_scene: Option<Scene>,
    outgoing_scene: Option<Scene>,
    transition_type: TransitionType,
    transition_duration_ms: u32,
    transition_started_ms: u64,
    custom_transition: CustomTransitionConfig,
    active_transition_config: CustomTransitionConfig,
    stinger_frame: Option<FrameBuffer>,
    luma_matte: Option<FrameBuffer>,
    stinger_audio_pending: bool,

    pub program: FrameBuffer,
    pub vertical: FrameBuffer,
    pub preview: FrameBuffer,
    pub preview_small_h: FrameBuffer,
    pub preview_small_v: FrameBuffer,

    scratch_h: FrameBuffer,
    scratch_in_h: FrameBuffer,
    scratch_v: FrameBuffer,
    scratch_in_v: FrameBuffer,

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
        let (backend_name, _, _) = backend.backend_info();
        Self {
            backend,
            gpu_backend: None,
            live_vertical: false,
            program_scene: None,
            preview_scene: None,
            outgoing_scene: None,
            transition_type: TransitionType::Fade,
            transition_duration_ms: 300,
            transition_started_ms: 0,
            custom_transition: CustomTransitionConfig::default(),
            active_transition_config: CustomTransitionConfig::default(),
            stinger_frame: None,
            luma_matte: None,
            stinger_audio_pending: false,
            program: FrameBuffer::new(horiz.0, horiz.1),
            vertical: FrameBuffer::new(vert.0, vert.1),
            preview: FrameBuffer::new(PREVIEW_H.0, PREVIEW_H.1),
            preview_small_h: FrameBuffer::new(PREVIEW_H.0, PREVIEW_H.1),
            preview_small_v: FrameBuffer::new(PREVIEW_V.0, PREVIEW_V.1),
            scratch_h: FrameBuffer::new(horiz.0, horiz.1),
            scratch_in_h: FrameBuffer::new(horiz.0, horiz.1),
            scratch_v: FrameBuffer::new(vert.0, vert.1),
            scratch_in_v: FrameBuffer::new(vert.0, vert.1),
            next_tick_due_ms: None,
            render_samples_ms: VecDeque::with_capacity(120),
            tick_timestamps_ms: VecDeque::with_capacity(120),
            dropped_frames: 0,
            telemetry: TelemetryStats {
                target_fps: studio.fps,
                compositor_backend: backend_name,
                ..Default::default()
            },
            studio,
        }
    }

    /// Tenta ativar o backend GPU `wgpu` em tempo de execução e retorna o `GpuPipelineStatus`.
    /// Em ambientes sem GPU (ex.: CI headless), mantém o fallback limpo sem pânico.
    pub fn try_enable_gpu_backend(&mut self) -> GpuPipelineStatus {
        if let Some(ref gpu) = self.gpu_backend {
            let (name, adapter, driver) = gpu.backend_info();
            self.telemetry.compositor_backend = name.clone();
            self.telemetry.zero_copy_active = true;
            return GpuPipelineStatus {
                zero_copy_active: true,
                compositor_backend: name,
                capture_backend: "auto".to_string(),
                adapter_name: adapter,
                driver_info: driver,
                frame_copy_count: 0,
                active_textures: gpu.active_textures(),
            };
        }

        let (cur_name, cur_adapter, cur_driver) = self.backend.backend_info();
        if cur_name == "wgpu" {
            self.telemetry.compositor_backend = cur_name.clone();
            self.telemetry.zero_copy_active = true;
            return GpuPipelineStatus {
                zero_copy_active: true,
                compositor_backend: cur_name,
                capture_backend: "auto".to_string(),
                adapter_name: cur_adapter,
                driver_info: cur_driver,
                frame_copy_count: 0,
                active_textures: self.backend.active_textures(),
            };
        }

        match WgpuBackend::try_new() {
            Ok(gpu) => {
                let (name, adapter, driver) = gpu.backend_info();
                let active_textures = gpu.active_textures();
                self.gpu_backend = Some(Box::new(gpu));
                self.telemetry.compositor_backend = name.clone();
                self.telemetry.zero_copy_active = true;
                GpuPipelineStatus {
                    zero_copy_active: true,
                    compositor_backend: name,
                    capture_backend: "auto".to_string(),
                    adapter_name: adapter,
                    driver_info: driver,
                    frame_copy_count: 0,
                    active_textures,
                }
            }
            Err(err) => {
                self.telemetry.compositor_backend = cur_name.clone();
                self.telemetry.zero_copy_active = false;
                GpuPipelineStatus {
                    zero_copy_active: false,
                    compositor_backend: cur_name,
                    capture_backend: "software-fallback".to_string(),
                    adapter_name: cur_adapter,
                    driver_info: format!("fallback: {err}"),
                    frame_copy_count: 1,
                    active_textures: 0,
                }
            }
        }
    }

    /// Desativa o override `wgpu` e volta ao backend padrão `B`.
    pub fn disable_gpu_backend(&mut self) {
        self.gpu_backend = None;
        let (name, _, _) = self.backend.backend_info();
        self.telemetry.compositor_backend = name;
        self.telemetry.zero_copy_active = false;
    }

    /// Retorna `(backend_name, adapter_name, driver_info)` do backend atualmente ativo.
    pub fn backend_info(&self) -> (String, String, String) {
        if let Some(ref gpu) = self.gpu_backend {
            gpu.backend_info()
        } else {
            self.backend.backend_info()
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

    /// Atualiza a configuração global de transições customizadas (Stinger / LumaWipe / Shaders / Easing).
    pub fn set_custom_transition(&mut self, mut config: CustomTransitionConfig) {
        config.normalize();
        self.custom_transition = config;
    }

    pub fn custom_transition(&self) -> &CustomTransitionConfig {
        &self.custom_transition
    }

    /// Empurra o frame BGRA atual (com canal alfa) do vídeo de Stinger para overlay da transição.
    pub fn push_stinger_frame(
        &mut self,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    ) -> Result<(), String> {
        let next_seq = self
            .stinger_frame
            .as_ref()
            .map(|f| f.seq.saturating_add(1))
            .unwrap_or(1);
        let mut fb = FrameBuffer::from_bgra(width, height, bgra, 0)?;
        fb.seq = next_seq;
        self.stinger_frame = Some(fb);
        Ok(())
    }

    pub fn clear_stinger_frame(&mut self) {
        self.stinger_frame = None;
    }

    /// Define uma imagem de matte grayscale/BGRA customizada para transições `LumaWipe`.
    pub fn set_luma_matte(
        &mut self,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    ) -> Result<(), String> {
        let next_seq = self
            .luma_matte
            .as_ref()
            .map(|f| f.seq.saturating_add(1))
            .unwrap_or(1);
        let mut fb = FrameBuffer::from_bgra(width, height, bgra, 0)?;
        fb.seq = next_seq;
        self.luma_matte = Some(fb);
        Ok(())
    }

    pub fn clear_luma_matte(&mut self) {
        self.luma_matte = None;
    }

    /// Consome o gatilho de áudio do Stinger (retorna `true` exatamente uma vez quando uma transição
    /// `Stinger` é disparada, para o runtime de áudio disparar o SFX sincronizado).
    pub fn take_stinger_audio_trigger(&mut self) -> bool {
        std::mem::replace(&mut self.stinger_audio_pending, false)
    }

    /// Indica se o ponto de corte (`stinger_transition_point_ms`) já foi atingido no instante `now_ms`.
    pub fn stinger_cut_reached(&self, now_ms: u64) -> bool {
        if self.outgoing_scene.is_none() || self.transition_type != TransitionType::Stinger {
            return true;
        }
        now_ms.saturating_sub(self.transition_started_ms)
            >= self.active_transition_config.stinger_transition_point_ms as u64
    }

    /// Define cenas de Program e Preview aplicando `CustomTransitionConfig` e resolvendo
    /// automaticamente `Scene::transition_override` quando presente na cena de destino.
    pub fn set_scenes_with_config(
        &mut self,
        program: Option<Scene>,
        preview: Option<Scene>,
        transition_type: TransitionType,
        duration_ms: u32,
        custom_config: Option<CustomTransitionConfig>,
        now_ms: u64,
    ) {
        if let Some(mut cfg) = custom_config {
            cfg.normalize();
            self.custom_transition = cfg;
        }

        let same_scene = match (&self.program_scene, &program) {
            (Some(a), Some(b)) => a.id == b.id,
            _ => false,
        };

        // Resolve override por cena de destino (se configurado)
        let (effective_type, effective_duration, mut effective_custom) =
            if let Some(ref ov) = program.as_ref().and_then(|s| s.transition_override.as_ref()) {
                (
                    ov.transition_type,
                    ov.duration_ms.clamp(50, 10_000),
                    ov.custom
                        .clone()
                        .unwrap_or_else(|| self.custom_transition.clone()),
                )
            } else {
                (
                    transition_type,
                    duration_ms.clamp(50, 10_000),
                    self.custom_transition.clone(),
                )
            };
        effective_custom.normalize();

        if effective_type == TransitionType::Cut {
            // Correção #9: Cut cancela qualquer transição em andamento imediatamente.
            self.outgoing_scene = None;
            self.transition_type = TransitionType::Cut;
            self.stinger_audio_pending = false;
        } else if !same_scene {
            if let (Some(prev), Some(_)) = (&self.program_scene, &program) {
                self.outgoing_scene = Some(prev.clone());
                self.transition_type = effective_type;
                self.transition_duration_ms = effective_duration;
                self.transition_started_ms = now_ms;
                self.active_transition_config = effective_custom;
                self.stinger_audio_pending = effective_type == TransitionType::Stinger;
            }
        }

        self.program_scene = program;
        self.preview_scene = preview;
    }

    pub fn set_scenes(
        &mut self,
        program: Option<Scene>,
        preview: Option<Scene>,
        transition_type: TransitionType,
        duration_ms: u32,
        now_ms: u64,
    ) {
        self.set_scenes_with_config(program, preview, transition_type, duration_ms, None, now_ms);
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

        let mut raw_progress = 1.0f32;
        let mut elapsed_ms = 0u64;
        if self.outgoing_scene.is_some() {
            elapsed_ms = now_ms.saturating_sub(self.transition_started_ms);
            let dur = self.transition_duration_ms.max(50) as f32;
            raw_progress = (elapsed_ms as f32 / dur).clamp(0.0, 1.0);
            if raw_progress >= 1.0 {
                self.outgoing_scene = None;
            }
        }
        let eased_progress = if raw_progress >= 1.0 {
            1.0
        } else {
            self.active_transition_config.easing.apply(raw_progress)
        };

        let vertical_active = self.is_vertical_active();
        let active_backend: &mut dyn CompositorBackend = match self.gpu_backend.as_deref_mut() {
            Some(gpu) => gpu,
            None => &mut self.backend,
        };

        Self::render_orientation(
            active_backend,
            &mut self.program,
            &mut self.scratch_h,
            &mut self.scratch_in_h,
            self.program_scene.as_ref(),
            self.outgoing_scene.as_ref(),
            false,
            self.transition_type,
            eased_progress,
            elapsed_ms,
            &self.active_transition_config,
            self.stinger_frame.as_ref(),
            self.luma_matte.as_ref(),
            captures,
            &self.studio,
        );
        self.program.seq = self.program.seq.saturating_add(1);
        self.program.updated_at_ms = now_ms;
        downscale_preview(&self.program, &mut self.preview_small_h);

        if vertical_active {
            Self::render_orientation(
                active_backend,
                &mut self.vertical,
                &mut self.scratch_v,
                &mut self.scratch_in_v,
                self.program_scene.as_ref(),
                self.outgoing_scene.as_ref(),
                true,
                self.transition_type,
                eased_progress,
                elapsed_ms,
                &self.active_transition_config,
                self.stinger_frame.as_ref(),
                self.luma_matte.as_ref(),
                captures,
                &self.studio,
            );
            self.vertical.seq = self.vertical.seq.saturating_add(1);
            self.vertical.updated_at_ms = now_ms;
            downscale_preview(&self.vertical, &mut self.preview_small_v);
        }

        if let Some(ref prev) = self.preview_scene {
            active_backend.compose_scene(&mut self.preview, prev, false, captures, &self.studio);
            self.preview.seq = self.preview.seq.saturating_add(1);
            self.preview.updated_at_ms = now_ms;
        }

        let elapsed_render_ms = measured_render_ms
            .unwrap_or_else(|| started.elapsed().as_secs_f32() * 1000.0);
        self.record_telemetry(now_ms, elapsed_render_ms);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_orientation(
        backend: &mut dyn CompositorBackend,
        target: &mut FrameBuffer,
        scratch_out: &mut FrameBuffer,
        scratch_in: &mut FrameBuffer,
        program: Option<&Scene>,
        outgoing: Option<&Scene>,
        vertical: bool,
        transition: TransitionType,
        progress: f32,
        elapsed_ms: u64,
        custom: &CustomTransitionConfig,
        stinger_frame: Option<&FrameBuffer>,
        luma_matte: Option<&FrameBuffer>,
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
            if transition == TransitionType::Cut {
                backend.compose_scene(target, prog, vertical, captures, studio);
                return;
            }
            backend.compose_scene(scratch_out, out, vertical, captures, studio);
            scratch_out.seq = scratch_out.seq.saturating_add(1);
            backend.compose_scene(scratch_in, prog, vertical, captures, studio);
            scratch_in.seq = scratch_in.seq.saturating_add(1);
            backend.compose_transition(
                target,
                scratch_out,
                scratch_in,
                transition,
                progress,
                elapsed_ms,
                custom,
                stinger_frame,
                luma_matte,
            );
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

        let (backend_name, _, _) = self.backend_info();
        self.telemetry.compositor_fps = fps;
        self.telemetry.target_fps = self.studio.fps;
        self.telemetry.render_ms_p95 = p95;
        self.telemetry.dropped_frames = self.dropped_frames;
        self.telemetry.compositor_backend = backend_name;
    }

    pub fn telemetry(&self) -> TelemetryStats {
        self.telemetry.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{SceneTransitionOverride, SourceSettings, TransitionEasing};

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

    fn make_solid_scene(id: &str, color: &str) -> Scene {
        Scene {
            id: id.to_string(),
            name: id.to_uppercase(),
            items: vec![make_color_item(id, color, 0, 0)],
            created_at: 0,
            transition_override: None,
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
            transition_override: None,
        };
        let scene_b = Scene {
            id: "b".to_string(),
            name: "B".to_string(),
            items: vec![make_color_item("bg2", "#0000ff", 0, 0)],
            created_at: 0,
            transition_override: None,
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
            transition_override: None,
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
            transition_override: None,
        };
        let s_red = Scene {
            id: "r".to_string(),
            name: "Red".to_string(),
            items: vec![make_color_item("r", "#ff0000", 0, 0)],
            created_at: 0,
            transition_override: None,
        };
        let s_green = Scene {
            id: "g".to_string(),
            name: "Green".to_string(),
            items: vec![make_color_item("g", "#00ff00", 0, 0)],
            created_at: 0,
            transition_override: None,
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
            transition_override: None,
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

    #[test]
    fn test_stinger_transition_point_alpha_overlay_and_audio_trigger() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (4, 2), (2, 4), SoftwareBackend);
        let captures = CaptureManager::new(60);

        let s_red = make_solid_scene("red", "#ff0000"); // BGRA [0, 0, 255, 255]
        let s_blue = make_solid_scene("blue", "#0000ff"); // BGRA [255, 0, 0, 255]

        // Stinger 4x2: coluna x=0 verde opaco ([0,255,0,255]), restante transparente ([0,0,0,0])
        let mut stinger_bgra = vec![0u8; 4 * 2 * 4];
        for y in 0..2 {
            let off = (y * 4) * 4;
            stinger_bgra[off..off + 4].copy_from_slice(&[0, 255, 0, 255]);
        }
        comp.push_stinger_frame(4, 2, stinger_bgra).unwrap();

        let cfg = CustomTransitionConfig {
            stinger_transition_point_ms: 400,
            ..Default::default()
        };
        comp.set_scenes(Some(s_red), None, TransitionType::Cut, 1000, 0);
        assert!(!comp.take_stinger_audio_trigger());

        comp.set_scenes_with_config(
            Some(s_blue),
            None,
            TransitionType::Stinger,
            1000,
            Some(cfg),
            100,
        );
        // Gatilho de áudio do stinger dispara exatamente uma vez
        assert!(comp.take_stinger_audio_trigger());
        assert!(!comp.take_stinger_audio_trigger());

        // Antes do ponto de corte (elapsed = 200ms < 400ms): x=0 = stinger verde, x=1 = outgoing vermelho
        comp.tick(&captures, 300, Some(1.0));
        assert!(!comp.stinger_cut_reached(300));
        assert_eq!(&comp.program.bgra[0..4], &[0, 255, 0, 255]);
        assert_eq!(&comp.program.bgra[4..8], &[0, 0, 255, 255]);

        // Após o ponto de corte (elapsed = 500ms >= 400ms): x=0 = stinger verde, x=1 = incoming azul!
        comp.tick(&captures, 600, Some(1.0));
        assert!(comp.stinger_cut_reached(600));
        assert_eq!(&comp.program.bgra[0..4], &[0, 255, 0, 255]);
        assert_eq!(&comp.program.bgra[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn test_luma_wipe_procedural_invert_and_custom_matte() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (8, 2), (2, 8), SoftwareBackend);
        let captures = CaptureManager::new(60);

        let s_black = make_solid_scene("blk", "#000000");
        let s_white = make_solid_scene("wht", "#ffffff");

        // 1. Horizontal wipe normal em t=0.5: x=0 deve ser branco (incoming), x=7 preto (outgoing)
        let cfg_horiz = CustomTransitionConfig {
            luma_preset: "horizontal-wipe".to_string(),
            luma_softness: 0.05,
            luma_invert: false,
            easing: TransitionEasing::Linear,
            ..Default::default()
        };
        comp.set_scenes(Some(s_black.clone()), None, TransitionType::Cut, 1000, 0);
        comp.set_scenes_with_config(
            Some(s_white.clone()),
            None,
            TransitionType::LumaWipe,
            1000,
            Some(cfg_horiz),
            100,
        );
        comp.tick(&captures, 600, Some(1.0)); // progress = 0.5
        assert!(comp.program.bgra[0] > 240); // x=0: incoming (branco)
        assert!(comp.program.bgra[7 * 4] < 15); // x=7: outgoing (preto)

        // 2. Horizontal wipe invertido em t=0.5: x=0 preto, x=7 branco
        let cfg_inv = CustomTransitionConfig {
            luma_preset: "horizontal-wipe".to_string(),
            luma_softness: 0.05,
            luma_invert: true,
            easing: TransitionEasing::Linear,
            ..Default::default()
        };
        comp.set_scenes(Some(s_black.clone()), None, TransitionType::Cut, 1000, 1200);
        comp.set_scenes_with_config(
            Some(s_white.clone()),
            None,
            TransitionType::LumaWipe,
            1000,
            Some(cfg_inv),
            1300,
        );
        comp.tick(&captures, 1800, Some(1.0));
        assert!(comp.program.bgra[0] < 15);
        assert!(comp.program.bgra[7 * 4] > 240);

        // 3. Matte customizado: metade esquerda branca (luma=1.0), metade direita preta (luma=0.0)
        // Em t=0.5 sem inversão, metade direita (luma=0.0) vira incoming (branco) e esquerda fica outgoing (preto)
        let mut matte = vec![0u8; 8 * 2 * 4];
        for y in 0..2 {
            for x in 0..8 {
                let off = (y * 8 + x) * 4;
                let v = if x < 4 { 255u8 } else { 0u8 };
                matte[off..off + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        comp.set_luma_matte(8, 2, matte).unwrap();
        let cfg_custom = CustomTransitionConfig {
            luma_softness: 0.05,
            luma_invert: false,
            easing: TransitionEasing::Linear,
            ..Default::default()
        };
        comp.set_scenes(Some(s_black), None, TransitionType::Cut, 1000, 2500);
        comp.set_scenes_with_config(
            Some(s_white),
            None,
            TransitionType::LumaWipe,
            1000,
            Some(cfg_custom),
            2600,
        );
        comp.tick(&captures, 3100, Some(1.0));
        assert!(comp.program.bgra[0] < 15);
        assert!(comp.program.bgra[7 * 4] > 240);
    }

    #[test]
    fn test_custom_shader_presets_and_easing_curves() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (8, 8), (8, 8), SoftwareBackend);
        let captures = CaptureManager::new(60);

        let s_black = make_solid_scene("blk", "#000000");
        let s_white = make_solid_scene("wht", "#ffffff");

        // Testa os 4 presets de CustomShader em t=0.5
        for preset in [
            CustomShaderPreset::ZoomBlur,
            CustomShaderPreset::RadialWipe,
            CustomShaderPreset::GlitchCut,
            CustomShaderPreset::Swirl,
        ] {
            let cfg = CustomTransitionConfig {
                shader_preset: preset,
                easing: TransitionEasing::Linear,
                ..Default::default()
            };
            comp.set_scenes(Some(s_black.clone()), None, TransitionType::Cut, 1000, 0);
            comp.set_scenes_with_config(
                Some(s_white.clone()),
                None,
                TransitionType::CustomShader,
                1000,
                Some(cfg),
                100,
            );
            comp.tick(&captures, 600, Some(1.0));
            // Todos os pixels devem ser opacos (alpha=255)
            assert!(comp.program.bgra.chunks_exact(4).all(|px| px[3] == 255));
        }

        // Compara curvas de Easing em t=0.25 (Fade preto -> branco)
        let mut sample_at_quarter = |easing: TransitionEasing| -> u8 {
            let cfg = CustomTransitionConfig {
                easing,
                ..Default::default()
            };
            comp.set_scenes(Some(s_black.clone()), None, TransitionType::Cut, 1000, 0);
            comp.set_scenes_with_config(
                Some(s_white.clone()),
                None,
                TransitionType::Fade,
                1000,
                Some(cfg),
                100,
            );
            comp.tick(&captures, 350, Some(1.0)); // raw t = 0.25
            comp.program.bgra[0]
        };

        let v_ease_in_out = sample_at_quarter(TransitionEasing::EaseInOut);
        let v_linear = sample_at_quarter(TransitionEasing::Linear);
        let v_cubic_out = sample_at_quarter(TransitionEasing::CubicOut);
        let v_expo_out = sample_at_quarter(TransitionEasing::ExpoOut);

        assert!(v_ease_in_out < v_linear);
        assert!(v_linear < v_cubic_out);
        assert!(v_cubic_out < v_expo_out);
    }

    #[test]
    fn test_per_scene_transition_override_and_gpu_backend_probe() {
        let studio = StudioCompositorSettings::default();
        let mut comp = Compositor::with_dimensions(studio, (4, 4), (4, 4), SoftwareBackend);
        let captures = CaptureManager::new(60);

        let s_red = make_solid_scene("red", "#ff0000");
        let mut s_stinger_target = make_solid_scene("stinger_scene", "#0000ff");
        s_stinger_target.transition_override = Some(SceneTransitionOverride {
            transition_type: TransitionType::Stinger,
            duration_ms: 1000,
            custom: Some(CustomTransitionConfig {
                stinger_transition_point_ms: 300,
                ..Default::default()
            }),
        });

        let mut s_cut_target = make_solid_scene("cut_scene", "#00ff00");
        s_cut_target.transition_override = Some(SceneTransitionOverride {
            transition_type: TransitionType::Cut,
            duration_ms: 200,
            custom: None,
        });

        // Mesmo pedindo TransitionType::Fade globalmente, o override da cena ativa Stinger!
        comp.set_scenes(Some(s_red), None, TransitionType::Cut, 300, 0);
        comp.set_scenes(Some(s_stinger_target), None, TransitionType::Fade, 300, 100);
        assert!(comp.take_stinger_audio_trigger());

        // Antes do ponto de corte (elapsed = 200ms < 300ms): ainda mostra outgoing (vermelho)
        comp.tick(&captures, 300, Some(1.0));
        assert_eq!(&comp.program.bgra[0..4], &[0, 0, 255, 255]);

        // Após o ponto de corte (elapsed = 350ms >= 300ms): troca para incoming (azul)
        comp.tick(&captures, 450, Some(1.0));
        assert_eq!(&comp.program.bgra[0..4], &[255, 0, 0, 255]);

        // Cena com override Cut cancela a transição imediatamente mesmo se chamada com Fade
        comp.set_scenes(Some(s_cut_target), None, TransitionType::Fade, 1000, 500);
        comp.tick(&captures, 516, Some(1.0));
        assert_eq!(&comp.program.bgra[0..4], &[0, 255, 0, 255]);

        // Verifica backend_info e try_enable_gpu_backend sem pânico em qualquer ambiente
        let (bname, _, _) = comp.backend_info();
        assert_eq!(bname, "software");
        let gpu_status = comp.try_enable_gpu_backend();
        assert!(
            gpu_status.compositor_backend == "wgpu"
                || gpu_status.compositor_backend == "software"
        );
    }
}

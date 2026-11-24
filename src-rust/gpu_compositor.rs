//! Motor de composição GPU em tempo real baseado em `wgpu 24`.
//!
//! Recursos principais:
//! - Inicialização segura com fallback automático para adaptador software (`WgpuBackend::try_new`).
//! - Shaders WGSL compilados nativamente:
//!   - [`LAYER_COMPOSITE_WGSL`]: quad por camada com transform, crop UV, `flip_x`, `opacity`,
//!     máscara SDF anti-aliased de cantos arredondados e premultiplied alpha blending.
//!   - [`TRANSITION_WGSL`]: transições `Cut`, `Fade`, `FadeToBlack`, `Slide`, `Stinger`
//!     (alpha overlay + corte exato em `stinger_transition_point_ms`), `LumaWipe` (matte customizado
//!     ou presets procedurais com suavidade e inversão) e `CustomShader` (`ZoomBlur`, `RadialWipe`,
//!     `GlitchCut`, `Swirl`, além de compilação sandboxed para `CustomWgsl`).
//! - Cache persistente de `wgpu::Texture` por `(source_key, seq)` evitando re-upload redundante.
//! - Renderização pura GPU-para-GPU (`render_to_gpu_texture`) com readback CPU sob demanda.

use crate::capture::{CaptureManager, FrameBuffer};
use crate::compositor::{compute_scene_layers, CompositorBackend, ComputedLayerLayout};
use crate::protocol::{
    CustomShaderPreset, CustomTransitionConfig, Scene, StudioCompositorSettings, TransitionType,
};
use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// Shader WGSL de composição de camadas (transform, crop UV, flip_x, opacity, SDF rounded corners, premultiplied alpha).
pub const LAYER_COMPOSITE_WGSL: &str = r#"
struct LayerUniforms {
    dst_rect: vec4<f32>,
    uv_rect: vec4<f32>,
    color: vec4<f32>,
    params: vec4<f32>,
    rect_px: vec4<f32>,
    draw_px: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: LayerUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local_uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0)
    );
    let corner = corners[vid];
    let cx = u.dst_rect.x + corner.x * u.dst_rect.z;
    let cy = u.dst_rect.y + corner.y * u.dst_rect.w;

    var out: VertexOutput;
    out.pos = vec4<f32>(cx * 2.0 - 1.0, 1.0 - cy * 2.0, 0.0, 1.0);
    let ux = select(corner.x, 1.0 - corner.x, u.params.y > 0.5);
    out.uv = vec2<f32>(
        u.uv_rect.x + ux * u.uv_rect.z,
        u.uv_rect.y + corner.y * u.uv_rect.w
    );
    out.local_uv = corner;
    return out;
}

fn rounded_box_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius, radius);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let clamped_uv = clamp(in.uv, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0));
    let texel = textureSampleLevel(src_tex, src_sampler, clamped_uv, 0.0);
    let is_solid = u.params.w > 0.5;
    let base_rgb = select(texel.rgb * u.color.rgb, u.color.rgb, is_solid);
    let base_a = select(texel.a * u.color.a, u.color.a, is_solid);

    var aa: f32 = 1.0;
    let radius = u.params.z;
    if (radius > 0.5) {
        let local_px = u.rect_px.zw + in.local_uv * u.draw_px.xy;
        let half_size = u.rect_px.xy * 0.5;
        let p = local_px - half_size;
        let d = rounded_box_sdf(p, half_size, radius);
        aa = 1.0 - smoothstep(-0.75, 0.75, d);
    }

    let final_a = clamp(base_a * u.params.x * aa, 0.0, 1.0);
    return vec4<f32>(base_rgb * final_a, final_a);
}
"#;

/// Cabeçalho padrão WGSL injetado automaticamente em shaders customizados que fornecem apenas `fs_main`.
const CUSTOM_TRANSITION_HEADER_WGSL: &str = r#"
struct TransitionUniforms {
    control: vec4<f32>,
    params: vec4<f32>,
    resolution: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: TransitionUniforms;
@group(0) @binding(1) var tex_out: texture_2d<f32>;
@group(0) @binding(2) var tex_in: texture_2d<f32>;
@group(0) @binding(3) var tex_aux: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
"#;

/// Shader WGSL de transições completas: Cut, Fade, FadeToBlack, Slide, Stinger, LumaWipe e CustomShader presets.
pub const TRANSITION_WGSL: &str = r#"
const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318530;

struct TransitionUniforms {
    control: vec4<f32>,
    params: vec4<f32>,
    resolution: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: TransitionUniforms;
@group(0) @binding(1) var tex_out: texture_2d<f32>;
@group(0) @binding(2) var tex_in: texture_2d<f32>;
@group(0) @binding(3) var tex_aux: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0)
    );
    let corner = corners[vid];
    var out: VertexOutput;
    out.pos = vec4<f32>(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);
    out.uv = corner;
    return out;
}

fn sample_clamped(tex: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(tex, samp, clamp(uv, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0)), 0.0);
}

fn eval_luma_mask_wgsl(luma_in: f32, t: f32, softness: f32, invert_flag: f32) -> f32 {
    let luma = select(luma_in, 1.0 - luma_in, invert_flag > 0.5);
    let s = max(clamp(softness, 0.0, 0.5), 0.0001);
    let edge = t * (1.0 + 2.0 * s) - s;
    return 1.0 - smoothstep(edge - s, edge + s, luma);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t = clamp(u.control.x, 0.0, 1.0);
    let mode = i32(round(u.control.z));
    let sub_mode = i32(round(u.control.w));

    let out_c = sample_clamped(tex_out, uv);
    let in_c = sample_clamped(tex_in, uv);

    // 0 = Cut
    if (mode == 0) {
        return vec4<f32>(in_c.rgb, 1.0);
    }

    // 1 = Fade
    if (mode == 1) {
        return vec4<f32>(mix(out_c.rgb, in_c.rgb, t), 1.0);
    }

    // 2 = FadeToBlack
    if (mode == 2) {
        if (t < 0.5) {
            let k = 1.0 - t * 2.0;
            return vec4<f32>(out_c.rgb * k, 1.0);
        } else {
            let k = (t - 0.5) * 2.0;
            return vec4<f32>(in_c.rgb * k, 1.0);
        }
    }

    // 3 = Slide (outgoing desliza para esquerda, incoming entra pela direita)
    if (mode == 3) {
        let split = 1.0 - t;
        let out_slide = sample_clamped(tex_out, vec2<f32>(uv.x + t, uv.y));
        let in_slide = sample_clamped(tex_in, vec2<f32>(uv.x - split, uv.y));
        let picked = select(in_slide.rgb, out_slide.rgb, uv.x < split);
        return vec4<f32>(picked, 1.0);
    }

    // 4 = Stinger (corte exato em stinger_transition_point_ms + overlay alfa de tex_aux)
    if (mode == 4) {
        let base = select(out_c.rgb, in_c.rgb, u.params.z > 0.5);
        if (u.params.w > 0.5) {
            let stinger = sample_clamped(tex_aux, uv);
            let rgb = mix(base, stinger.rgb, clamp(stinger.a, 0.0, 1.0));
            return vec4<f32>(rgb, 1.0);
        }
        return vec4<f32>(base, 1.0);
    }

    // 5 = LumaWipe
    if (mode == 5) {
        var luma: f32 = uv.x;
        if (sub_mode == 0 && u.params.w > 0.5) {
            let m = sample_clamped(tex_aux, uv);
            luma = dot(m.rgb, vec3<f32>(0.299, 0.587, 0.114));
        } else if (sub_mode == 2) {
            luma = uv.y;
        } else if (sub_mode == 3) {
            let d = (uv - vec2<f32>(0.5, 0.5)) * 1.41421356;
            luma = clamp(length(d), 0.0, 1.0);
        } else if (sub_mode == 4) {
            luma = clamp((uv.x + uv.y) * 0.5, 0.0, 1.0);
        } else if (sub_mode == 5) {
            luma = clamp(abs(uv.x - 0.5) * 2.0, 0.0, 1.0);
        } else {
            luma = uv.x;
        }
        let mask = eval_luma_mask_wgsl(luma, t, u.params.x, u.params.y);
        return vec4<f32>(mix(out_c.rgb, in_c.rgb, mask), 1.0);
    }

    // 6 = CustomShader presets
    if (mode == 6) {
        // 0 = ZoomBlur
        if (sub_mode == 0) {
            let strength = sin(t * PI) * 0.18;
            let center = vec2<f32>(0.5, 0.5);
            let dir = uv - center;
            var acc_out = vec3<f32>(0.0, 0.0, 0.0);
            var acc_in = vec3<f32>(0.0, 0.0, 0.0);
            for (var i: i32 = 0; i < 5; i = i + 1) {
                let frac = f32(i) * 0.25;
                let s_out = 1.0 - strength * frac;
                let s_in = 1.0 + strength * (1.0 - frac);
                acc_out = acc_out + sample_clamped(tex_out, center + dir * s_out).rgb;
                acc_in = acc_in + sample_clamped(tex_in, center + dir * s_in).rgb;
            }
            let blend_k = smoothstep(0.25, 0.75, t);
            return vec4<f32>(mix(acc_out * 0.2, acc_in * 0.2, blend_k), 1.0);
        }

        // 1 = RadialWipe (varredura horária a partir das 12h)
        if (sub_mode == 1) {
            let d = uv - vec2<f32>(0.5, 0.5);
            var ang = atan2(d.x, -d.y);
            if (ang < 0.0) {
                ang = ang + TAU;
            }
            let norm_ang = ang / TAU;
            let mask = eval_luma_mask_wgsl(norm_ang, t, 0.03, 0.0);
            return vec4<f32>(mix(out_c.rgb, in_c.rgb, mask), 1.0);
        }

        // 2 = GlitchCut
        if (sub_mode == 2) {
            let g = sin(t * PI);
            let band = floor(uv.y * 24.0);
            let seed = band * 127.1 + floor(t * 17.0) * 311.7;
            let h = fract(sin(seed) * 43758.5453) * 2.0 - 1.0;
            let shift = h * g * 0.08;
            let ca = g * 0.025;
            let use_in = (t + h * 0.15) >= 0.5;
            let r_out = sample_clamped(tex_out, vec2<f32>(uv.x + shift + ca, uv.y)).r;
            let g_out = sample_clamped(tex_out, vec2<f32>(uv.x + shift, uv.y)).g;
            let b_out = sample_clamped(tex_out, vec2<f32>(uv.x + shift - ca, uv.y)).b;
            let r_in = sample_clamped(tex_in, vec2<f32>(uv.x + shift + ca, uv.y)).r;
            let g_in = sample_clamped(tex_in, vec2<f32>(uv.x + shift, uv.y)).g;
            let b_in = sample_clamped(tex_in, vec2<f32>(uv.x + shift - ca, uv.y)).b;
            let col_out = vec3<f32>(r_out, g_out, b_out);
            let col_in = vec3<f32>(r_in, g_in, b_in);
            return vec4<f32>(select(col_out, col_in, use_in), 1.0);
        }

        // 3 = Swirl
        if (sub_mode == 3) {
            let center = vec2<f32>(0.5, 0.5);
            let d = uv - center;
            let dist = length(d);
            let theta = sin(t * PI) * max(0.0, 1.0 - dist) * 5.0;
            let c1 = cos(theta);
            let s1 = sin(theta);
            let rot_out = vec2<f32>(d.x * c1 - d.y * s1, d.x * s1 + d.y * c1) + center;
            let rot_in = vec2<f32>(d.x * c1 + d.y * s1, -d.x * s1 + d.y * c1) + center;
            let col_out = sample_clamped(tex_out, rot_out).rgb;
            let col_in = sample_clamped(tex_in, rot_in).rgb;
            return vec4<f32>(mix(col_out, col_in, t), 1.0);
        }
    }

    return vec4<f32>(mix(out_c.rgb, in_c.rgb, t), 1.0);
}
"#;

/// Uniforms de uma camada no shader `LAYER_COMPOSITE_WGSL` (96 bytes, alinhado a 16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct LayerUniforms {
    /// Retângulo desenhado no canvas normalizado `[0..1]`: `(x, y, width, height)`.
    pub dst_rect: [f32; 4],
    /// Sub-retângulo UV na textura fonte `[0..1]`: `(u0, v0, u_span, v_span)`.
    pub uv_rect: [f32; 4],
    /// Cor sólida ou multiplicador RGBA `[0..1]`.
    pub color: [f32; 4],
    /// `(opacity, flip_x, corner_radius_px, is_solid_color)`.
    pub params: [f32; 4],
    /// `(outer_w_px, outer_h_px, local_offset_x_px, local_offset_y_px)`.
    pub rect_px: [f32; 4],
    /// `(draw_w_px, draw_h_px, 0.0, 0.0)`.
    pub draw_px: [f32; 4],
}

/// Uniforms do shader de transição `TRANSITION_WGSL` (48 bytes, alinhado a 16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct TransitionUniforms {
    /// `(eased_progress, raw_progress, mode, sub_mode)`.
    pub control: [f32; 4],
    /// `(luma_softness, luma_invert, stinger_cut_reached, has_aux_texture)`.
    pub params: [f32; 4],
    /// `(width, height, aspect, elapsed_sec)`.
    pub resolution: [f32; 4],
}

/// Mapeia o nome do preset de Luma Wipe para o código inteiro usado pelo shader GPU.
pub fn luma_preset_code(preset: &str) -> u32 {
    match preset.trim().to_ascii_lowercase().as_str() {
        "custom" | "matte" => 0,
        "horizontal-wipe" | "horizontal" | "wipe-right" => 1,
        "vertical-wipe" | "vertical" | "wipe-down" => 2,
        "radial" | "circle" | "iris" => 3,
        "diagonal" | "diag" => 4,
        "barn-door" | "barndoor" | "split" => 5,
        _ => 1,
    }
}

/// Avalia o valor procedural de luminância `[0.0, 1.0]` para o preset de Luma Wipe em `(u, v)`.
pub fn eval_luma_preset(preset: &str, u: f32, v: f32) -> f32 {
    let uu = u.clamp(0.0, 1.0);
    let vv = v.clamp(0.0, 1.0);
    match luma_preset_code(preset) {
        2 => vv,
        3 => {
            let dx = (uu - 0.5) * std::f32::consts::SQRT_2;
            let dy = (vv - 0.5) * std::f32::consts::SQRT_2;
            (dx * dx + dy * dy).sqrt().clamp(0.0, 1.0)
        }
        4 => ((uu + vv) * 0.5).clamp(0.0, 1.0),
        5 => ((uu - 0.5).abs() * 2.0).clamp(0.0, 1.0),
        _ => uu,
    }
}

/// Calcula o fator de mistura `[0.0, 1.0]` de um Luma Wipe (0 = outgoing, 1 = incoming)
/// respeitando `softness` e `invert`, com extremos exatos em `progress = 0` e `progress = 1`.
pub fn eval_luma_mask(luma: f32, progress: f32, softness: f32, invert: bool) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    let raw_luma = luma.clamp(0.0, 1.0);
    let l = if invert { 1.0 - raw_luma } else { raw_luma };
    let s = softness.clamp(0.0, 0.5).max(0.0001);
    let edge = t * (1.0 + 2.0 * s) - s;
    let lo = edge - s;
    let hi = edge + s;
    let x = ((l - lo) / (hi - lo)).clamp(0.0, 1.0);
    let smooth = x * x * (3.0 - 2.0 * x);
    (1.0 - smooth).clamp(0.0, 1.0)
}

/// Remove comentários WGSL (`//...` e `/*...*/`) para validação léxica segura.
fn strip_wgsl_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            }
            out.push(' ');
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Verifica se a palavra-chave `kw` ocorre como token isolado em `code`.
fn contains_wgsl_keyword(code: &str, kw: &str) -> bool {
    let bytes = code.as_bytes();
    let kw_bytes = kw.as_bytes();
    if kw_bytes.is_empty() || bytes.len() < kw_bytes.len() {
        return false;
    }
    let mut i = 0;
    while i + kw_bytes.len() <= bytes.len() {
        if &bytes[i..i + kw_bytes.len()] == kw_bytes {
            let prev_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let next_ok =
                i + kw_bytes.len() == bytes.len() || !is_ident_char(bytes[i + kw_bytes.len()]);
            if prev_ok && next_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Validador de segurança estática para shaders WGSL customizados enviados pelo usuário.
///
/// Regras:
/// - Rejeita código vazio ou maior que 64 KiB.
/// - Exige entry point `@fragment` e `fn fs_main`.
/// - Bloqueia laços potencialmente infinitos (`loop`, `while`, `continuing`, `for (;;)`).
/// - Bloqueia estágios compute (`@compute`, `@workgroup_size`) e buffers `read_write`.
/// - Verifica balanceamento de `{}` e `()`.
pub fn validate_custom_wgsl(src: &str) -> Result<(), String> {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return Err("Shader WGSL vazio.".to_string());
    }
    if trimmed.len() > 64 * 1024 {
        return Err("Shader WGSL excede o limite de segurança de 64 KiB.".to_string());
    }

    let cleaned = strip_wgsl_comments(trimmed);

    // Balanceamento de chaves e parênteses
    let mut brace_depth: i32 = 0;
    let mut paren_depth: i32 = 0;
    for ch in cleaned.chars() {
        match ch {
            '{' => brace_depth += 1,
            '}' => {
                brace_depth -= 1;
                if brace_depth < 0 {
                    return Err("Chave '}' inesperada no shader WGSL.".to_string());
                }
            }
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err("Parêntese ')' inesperado no shader WGSL.".to_string());
                }
            }
            _ => {}
        }
    }
    if brace_depth != 0 || paren_depth != 0 {
        return Err("Delimitadores '{ }' ou '( )' desbalanceados no shader WGSL.".to_string());
    }

    // Entry point obrigatório `@fragment` + `fn fs_main`
    if !cleaned.contains("@fragment") {
        return Err("Shader WGSL customizado precisa declarar o atributo '@fragment'.".to_string());
    }
    let has_fs_main = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0] == "fn" && (w[1] == "fs_main" || w[1].starts_with("fs_main(")));
    if !has_fs_main {
        return Err(
            "Shader WGSL customizado precisa definir o entry point 'fn fs_main(...)'.".to_string(),
        );
    }

    // Bloqueio de compute shaders e storage read_write (sandbox fragment-only)
    if cleaned.contains("@compute") || cleaned.contains("@workgroup_size") {
        return Err("Compute shaders não são permitidos em transições customizadas.".to_string());
    }
    if contains_wgsl_keyword(&cleaned, "read_write") {
        return Err("Buffers de escrita (read_write) não são permitidos em transições.".to_string());
    }

    // Bloqueio de padrões de loop infinito (evita GPU TDR / Device Lost)
    if contains_wgsl_keyword(&cleaned, "loop") {
        return Err(
            "Instrução 'loop' não limitada é proibida em shaders customizados (risco de TDR)."
                .to_string(),
        );
    }
    if contains_wgsl_keyword(&cleaned, "while") {
        return Err(
            "Instrução 'while' não limitada é proibida em shaders customizados (risco de TDR)."
                .to_string(),
        );
    }
    if contains_wgsl_keyword(&cleaned, "continuing") {
        return Err("Bloco 'continuing' não é permitido em shaders customizados.".to_string());
    }

    // Verifica `for` sem condição (ex.: `for (;;)` ou `for (var i = 0; ; i++)`)
    let compact: String = cleaned.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.contains("for(;;)") || compact.contains("for(;;") || compact.contains(";;)") {
        return Err(
            "Laço 'for' sem condição de parada é proibido em shaders customizados.".to_string(),
        );
    }

    Ok(())
}

struct CachedGpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    seq: u64,
}

struct GpuRenderTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// Backend GPU nativo (`wgpu 24`: DX12 / Vulkan / Metal / fallback software) implementando [`CompositorBackend`].
pub struct WgpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    driver_info: String,

    layer_bind_group_layout: wgpu::BindGroupLayout,
    layer_pipeline: wgpu::RenderPipeline,
    layer_uniform_buffer: wgpu::Buffer,
    layer_uniform_capacity: usize,
    uniform_align: usize,

    transition_shader: wgpu::ShaderModule,
    transition_bind_group_layout: wgpu::BindGroupLayout,
    transition_pipeline_layout: wgpu::PipelineLayout,
    transition_pipeline: wgpu::RenderPipeline,
    transition_uniform_buffer: wgpu::Buffer,

    custom_shader_hash: u64,
    custom_transition_pipeline: Option<wgpu::RenderPipeline>,

    linear_sampler: wgpu::Sampler,
    _white_1x1_texture: wgpu::Texture,
    white_1x1_view: wgpu::TextureView,

    texture_cache: HashMap<String, CachedGpuTexture>,
    render_targets: HashMap<String, GpuRenderTarget>,
    readback_buffer: Option<(wgpu::Buffer, u64)>,
    transition_upload_seq: u64,
}

impl WgpuBackend {
    /// Inicializa o dispositivo `wgpu 24`, preferindo GPU de alto desempenho com fallback automático
    /// para adaptador software se necessário. Retorna `Err` limpo (sem pânico) em ambientes sem GPU.
    pub fn try_new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .or_else(|| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: true,
                compatible_surface: None,
            }))
        })
        .ok_or_else(|| "Nenhum adaptador GPU/wgpu disponível neste ambiente.".to_string())?;

        let info = adapter.get_info();
        let adapter_name = if info.name.trim().is_empty() {
            format!("{:?}", info.device_type)
        } else {
            info.name.clone()
        };
        let driver_info = format!(
            "{:?} | {} {}",
            info.backend,
            info.driver.trim(),
            info.driver_info.trim()
        )
        .trim()
        .to_string();

        let adapter_limits = adapter.limits();
        let candidate_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits.clone());
        let required_limits = if candidate_limits.check_limits(&adapter_limits) {
            candidate_limits
        } else {
            adapter_limits
        };

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("prismcast-wgpu-compositor"),
                required_features: wgpu::Features::empty(),
                required_limits,
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|e| format!("Falha ao criar wgpu::Device: {e}"))?;

        let uniform_align = (device.limits().min_uniform_buffer_offset_alignment as usize).max(256);

        // Textura branca 1x1 usada como fallback para ColorSource / bindings auxiliares
        let white_1x1_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prismcast-white-1x1"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_1x1_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_1x1_view = white_1x1_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("prismcast-linear-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 32.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        // Pipeline de composição de camadas
        let layer_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismcast-layer-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LAYER_COMPOSITE_WGSL)),
        });

        let layer_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("prismcast-layer-bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<LayerUniforms>() as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let layer_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("prismcast-layer-pl"),
                bind_group_layouts: &[&layer_bind_group_layout],
                push_constant_ranges: &[],
            });

        let layer_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prismcast-layer-pipeline"),
            layout: Some(&layer_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &layer_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &layer_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let initial_layer_capacity = 32usize;
        let layer_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismcast-layer-uniforms"),
            size: (initial_layer_capacity * uniform_align) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Pipeline de transições
        let transition_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismcast-transition-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(TRANSITION_WGSL)),
        });

        let transition_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("prismcast-transition-bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<TransitionUniforms>() as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let transition_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("prismcast-transition-pl"),
                bind_group_layouts: &[&transition_bind_group_layout],
                push_constant_ranges: &[],
            });

        let transition_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prismcast-transition-pipeline"),
            layout: Some(&transition_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &transition_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &transition_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let transition_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismcast-transition-uniforms"),
            size: std::mem::size_of::<TransitionUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            adapter_name,
            driver_info,
            layer_bind_group_layout,
            layer_pipeline,
            layer_uniform_buffer,
            layer_uniform_capacity: initial_layer_capacity,
            uniform_align,
            transition_shader,
            transition_bind_group_layout,
            transition_pipeline_layout,
            transition_pipeline,
            transition_uniform_buffer,
            custom_shader_hash: 0,
            custom_transition_pipeline: None,
            linear_sampler,
            _white_1x1_texture: white_1x1_texture,
            white_1x1_view,
            texture_cache: HashMap::new(),
            render_targets: HashMap::new(),
            readback_buffer: None,
            transition_upload_seq: 0,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn driver_info(&self) -> &str {
        &self.driver_info
    }

    pub fn cached_texture_count(&self) -> usize {
        self.texture_cache.len()
    }

    /// Libera texturas GPU de fontes que saíram das cenas ativas.
    pub fn prune_texture_cache(&mut self, active_keys: &HashSet<String>) -> usize {
        let before = self.texture_cache.len();
        self.texture_cache
            .retain(|k, _| k.starts_with("__") || active_keys.contains(k));
        before.saturating_sub(self.texture_cache.len())
    }

    /// Garante que o render target GPU para `slot` exista nas dimensões solicitadas.
    fn ensure_render_target(&mut self, slot: &str, width: u32, height: u32) {
        let needs_alloc = match self.render_targets.get(slot) {
            Some(rt) => rt.width != width || rt.height != height,
            None => true,
        };
        if needs_alloc {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(slot),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_targets.insert(
                slot.to_string(),
                GpuRenderTarget {
                    texture,
                    view,
                    width: width.max(1),
                    height: height.max(1),
                },
            );
        }
    }

    /// Faz upload de `FrameBuffer` para cache GPU apenas se `(width, height, seq)` mudou
    /// (ou se `force_upload` for `true`, como em buffers temporários de transição).
    fn upload_or_reuse_texture(&mut self, key: &str, fb: &FrameBuffer, force_upload: bool) {
        if fb.width == 0 || fb.height == 0 {
            return;
        }
        let expected_bytes = (fb.width as usize)
            .saturating_mul(fb.height as usize)
            .saturating_mul(4);
        if fb.bgra.len() < expected_bytes {
            return;
        }

        let needs_recreate = match self.texture_cache.get(key) {
            Some(c) => c.width != fb.width || c.height != fb.height,
            None => true,
        };
        let needs_upload = needs_recreate
            || force_upload
            || self
                .texture_cache
                .get(key)
                .map(|c| c.seq != fb.seq)
                .unwrap_or(true);

        if needs_recreate {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(key),
                size: wgpu::Extent3d {
                    width: fb.width,
                    height: fb.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.texture_cache.insert(
                key.to_string(),
                CachedGpuTexture {
                    texture,
                    view,
                    width: fb.width,
                    height: fb.height,
                    seq: u64::MAX,
                },
            );
        }

        if needs_upload {
            if let Some(cached) = self.texture_cache.get_mut(key) {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &cached.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &fb.bgra[..expected_bytes],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(fb.width * 4),
                        rows_per_image: Some(fb.height),
                    },
                    wgpu::Extent3d {
                        width: fb.width,
                        height: fb.height,
                        depth_or_array_layers: 1,
                    },
                );
                cached.seq = fb.seq;
            }
        }
    }

    fn ensure_layer_uniform_capacity(&mut self, count: usize) {
        if count <= self.layer_uniform_capacity {
            return;
        }
        let next_cap = count.next_power_of_two().max(32);
        self.layer_uniform_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismcast-layer-uniforms"),
            size: (next_cap * self.uniform_align) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.layer_uniform_capacity = next_cap;
    }

    fn ensure_readback_buffer(&mut self, needed_bytes: u64) {
        let needs_alloc = match self.readback_buffer {
            Some((_, cap)) => cap < needed_bytes,
            None => true,
        };
        if needs_alloc {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("prismcast-readback-staging"),
                size: needed_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            self.readback_buffer = Some((buf, needed_bytes));
        }
    }

    /// Tenta compilar (com cache por hash e error scope `Validation`) um shader WGSL customizado.
    fn ensure_custom_transition_pipeline(&mut self, wgsl_src: &str) -> bool {
        if validate_custom_wgsl(wgsl_src).is_err() {
            return false;
        }
        let mut hasher = DefaultHasher::new();
        wgsl_src.hash(&mut hasher);
        let h = hasher.finish();
        if self.custom_shader_hash == h && self.custom_transition_pipeline.is_some() {
            return true;
        }

        let has_vertex = wgsl_src.contains("@vertex");
        let full_source = if !has_vertex
            && !wgsl_src.contains("struct TransitionUniforms")
            && !wgsl_src.contains("@group(0)")
        {
            format!("{CUSTOM_TRANSITION_HEADER_WGSL}\n{wgsl_src}")
        } else {
            wgsl_src.to_string()
        };

        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("prismcast-custom-transition-wgsl"),
                source: wgpu::ShaderSource::Wgsl(Cow::Owned(full_source)),
            });

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("prismcast-custom-transition-pipeline"),
                layout: Some(&self.transition_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: if has_vertex {
                        &module
                    } else {
                        &self.transition_shader
                    },
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            });

        let validation_err = pollster::block_on(self.device.pop_error_scope());
        if validation_err.is_none() {
            self.custom_shader_hash = h;
            self.custom_transition_pipeline = Some(pipeline);
            true
        } else {
            self.custom_shader_hash = h;
            self.custom_transition_pipeline = None;
            false
        }
    }

    /// Renderiza toda a cena diretamente para uma textura `wgpu::Texture` residente em GPU
    /// (caminho zero-copy sem readback CPU).
    #[allow(clippy::too_many_arguments)]
    pub fn render_to_gpu_texture(
        &mut self,
        slot: &str,
        width: u32,
        height: u32,
        scene: &Scene,
        vertical: bool,
        captures: &CaptureManager,
        studio: &StudioCompositorSettings,
    ) -> Result<&wgpu::Texture, String> {
        if width == 0 || height == 0 {
            return Err("Dimensões do render target GPU inválidas (0).".to_string());
        }

        self.ensure_render_target(slot, width, height);

        let layouts: Vec<ComputedLayerLayout> =
            compute_scene_layers(width, height, scene, vertical, captures, studio);

        // Faz upload incremental apenas de fontes cujo seq mudou
        for layer in &layouts {
            if let Some(ref key) = layer.source_key {
                if let Some(fb) = captures.get_frame(key) {
                    self.upload_or_reuse_texture(key, fb, false);
                }
            }
        }

        let draw_count = layouts.len();
        self.ensure_layer_uniform_capacity(draw_count.max(1));

        let mut staging_uniforms = vec![0u8; draw_count.max(1) * self.uniform_align];
        for (idx, layer) in layouts.iter().enumerate() {
            let uniform = LayerUniforms {
                dst_rect: layer.dst_rect,
                uv_rect: layer.uv_rect,
                color: layer.color_rgba,
                params: [
                    layer.opacity,
                    if layer.flip_x { 1.0 } else { 0.0 },
                    layer.corner_radius_px,
                    if layer.is_solid_color { 1.0 } else { 0.0 },
                ],
                rect_px: layer.rect_px,
                draw_px: layer.draw_px,
            };
            let bytes = bytemuck::bytes_of(&uniform);
            let off = idx * self.uniform_align;
            staging_uniforms[off..off + bytes.len()].copy_from_slice(bytes);
        }
        if draw_count > 0 {
            self.queue
                .write_buffer(&self.layer_uniform_buffer, 0, &staging_uniforms);
        }

        let uniform_size = NonZeroU64::new(std::mem::size_of::<LayerUniforms>() as u64);
        let mut bind_groups = Vec::with_capacity(draw_count);
        for layer in &layouts {
            let tex_view = layer
                .source_key
                .as_deref()
                .and_then(|k| self.texture_cache.get(k))
                .map(|c| &c.view)
                .unwrap_or(&self.white_1x1_view);

            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("prismcast-layer-bg"),
                layout: &self.layer_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.layer_uniform_buffer,
                            offset: 0,
                            size: uniform_size,
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(tex_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });
            bind_groups.push(bg);
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismcast-layer-encoder"),
            });

        {
            let rt = self
                .render_targets
                .get(slot)
                .ok_or_else(|| "Render target GPU ausente.".to_string())?;
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismcast-layer-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &rt.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if !bind_groups.is_empty() {
                rpass.set_pipeline(&self.layer_pipeline);
                for (idx, bg) in bind_groups.iter().enumerate() {
                    let dyn_offset = (idx * self.uniform_align) as u32;
                    rpass.set_bind_group(0, bg, &[dyn_offset]);
                    rpass.draw(0..6, 0..1);
                }
            }
        }

        self.queue.submit(Some(encoder.finish()));

        self.render_targets
            .get(slot)
            .map(|rt| &rt.texture)
            .ok_or_else(|| "Render target GPU indisponível após submit.".to_string())
    }

    /// Executa a transição em GPU entre `outgoing` e `incoming`, gravando o resultado no
    /// render target `slot` (sem readback CPU).
    #[allow(clippy::too_many_arguments)]
    pub fn render_transition_to_gpu_texture(
        &mut self,
        slot: &str,
        width: u32,
        height: u32,
        outgoing: &FrameBuffer,
        incoming: &FrameBuffer,
        transition: TransitionType,
        progress: f32,
        elapsed_ms: u64,
        custom: &CustomTransitionConfig,
        stinger_frame: Option<&FrameBuffer>,
        luma_matte: Option<&FrameBuffer>,
    ) -> Result<&wgpu::Texture, String> {
        if width == 0 || height == 0 {
            return Err("Dimensões da transição GPU inválidas (0).".to_string());
        }

        self.ensure_render_target(slot, width, height);
        self.transition_upload_seq = self.transition_upload_seq.saturating_add(1);

        self.upload_or_reuse_texture("__transition_out", outgoing, true);
        self.upload_or_reuse_texture("__transition_in", incoming, true);

        let mut has_aux = false;
        let aux_key = match transition {
            TransitionType::Stinger => {
                if let Some(stinger) = stinger_frame {
                    self.upload_or_reuse_texture("__stinger_frame", stinger, false);
                    has_aux = true;
                    Some("__stinger_frame")
                } else {
                    None
                }
            }
            TransitionType::LumaWipe => {
                if let Some(matte) = luma_matte {
                    self.upload_or_reuse_texture("__luma_matte", matte, false);
                    has_aux = true;
                    Some("__luma_matte")
                } else {
                    None
                }
            }
            _ => None,
        };

        let mode_code: f32 = match transition {
            TransitionType::Cut => 0.0,
            TransitionType::Fade => 1.0,
            TransitionType::FadeToBlack => 2.0,
            TransitionType::Slide => 3.0,
            TransitionType::Stinger => 4.0,
            TransitionType::LumaWipe => 5.0,
            TransitionType::CustomShader => 6.0,
        };

        let sub_mode_code: f32 = match transition {
            TransitionType::LumaWipe => {
                if has_aux {
                    0.0
                } else {
                    luma_preset_code(&custom.luma_preset) as f32
                }
            }
            TransitionType::CustomShader => match custom.shader_preset {
                CustomShaderPreset::ZoomBlur => 0.0,
                CustomShaderPreset::RadialWipe => 1.0,
                CustomShaderPreset::GlitchCut => 2.0,
                CustomShaderPreset::Swirl => 3.0,
                CustomShaderPreset::CustomWgsl => 4.0,
            },
            _ => 0.0,
        };

        let use_custom_pipeline = transition == TransitionType::CustomShader
            && custom.shader_preset == CustomShaderPreset::CustomWgsl
            && custom
                .custom_wgsl
                .as_deref()
                .map(|src| self.ensure_custom_transition_pipeline(src))
                .unwrap_or(false);

        let stinger_cut_reached = elapsed_ms >= custom.stinger_transition_point_ms as u64;
        let uniforms = TransitionUniforms {
            control: [
                progress.clamp(0.0, 1.0),
                progress.clamp(0.0, 1.0),
                mode_code,
                sub_mode_code,
            ],
            params: [
                custom.luma_softness.clamp(0.0, 0.5),
                if custom.luma_invert { 1.0 } else { 0.0 },
                if stinger_cut_reached { 1.0 } else { 0.0 },
                if has_aux { 1.0 } else { 0.0 },
            ],
            resolution: [
                width as f32,
                height as f32,
                (width as f32) / (height.max(1) as f32),
                (elapsed_ms as f32) / 1000.0,
            ],
        };
        self.queue.write_buffer(
            &self.transition_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        let out_view = self
            .texture_cache
            .get("__transition_out")
            .map(|c| &c.view)
            .unwrap_or(&self.white_1x1_view);
        let in_view = self
            .texture_cache
            .get("__transition_in")
            .map(|c| &c.view)
            .unwrap_or(&self.white_1x1_view);
        let aux_view = aux_key
            .and_then(|k| self.texture_cache.get(k))
            .map(|c| &c.view)
            .unwrap_or(&self.white_1x1_view);

        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismcast-transition-bg"),
            layout: &self.transition_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.transition_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(out_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(in_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(aux_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismcast-transition-encoder"),
            });

        {
            let rt = self
                .render_targets
                .get(slot)
                .ok_or_else(|| "Render target de transição ausente.".to_string())?;
            let pipeline = if use_custom_pipeline {
                self.custom_transition_pipeline
                    .as_ref()
                    .unwrap_or(&self.transition_pipeline)
            } else {
                &self.transition_pipeline
            };
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismcast-transition-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &rt.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(pipeline);
            rpass.set_bind_group(0, &bg, &[]);
            rpass.draw(0..6, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));

        self.render_targets
            .get(slot)
            .map(|rt| &rt.texture)
            .ok_or_else(|| "Render target de transição indisponível.".to_string())
    }

    /// Copia uma `wgpu::Texture` (`Bgra8Unorm`) de volta para um `FrameBuffer` CPU sob demanda,
    /// respeitando o alinhamento obrigatório de 256 bytes (`COPY_BYTES_PER_ROW_ALIGNMENT`).
    pub fn readback_texture_to_framebuffer(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
        target: &mut FrameBuffer,
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let unpadded_bpr = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;
        let needed_bytes = (padded_bpr as u64) * (height as u64);

        self.ensure_readback_buffer(needed_bytes);
        let (ref staging, _) = self
            .readback_buffer
            .as_ref()
            .ok_or_else(|| "Buffer staging de readback indisponível.".to_string())?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismcast-readback-encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..needed_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        self.device.poll(wgpu::Maintain::Wait);

        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("Falha ao mapear buffer de readback GPU: {e}")),
            Err(e) => return Err(format!("Canal de readback GPU fechado: {e}")),
        }

        let expected_len = (unpadded_bpr as usize).saturating_mul(height as usize);
        if target.bgra.len() != expected_len {
            target.bgra.resize(expected_len, 0);
        }
        target.width = width;
        target.height = height;

        {
            let data = slice.get_mapped_range();
            let row_len = unpadded_bpr as usize;
            let stride = padded_bpr as usize;
            for row in 0..(height as usize) {
                let src_off = row * stride;
                let dst_off = row * row_len;
                target.bgra[dst_off..dst_off + row_len]
                    .copy_from_slice(&data[src_off..src_off + row_len]);
            }
        }
        staging.unmap();
        Ok(())
    }

    /// Helper interno para fazer readback direto de um slot em `self.render_targets` sem conflito de borrow.
    fn readback_slot_to_framebuffer(
        &mut self,
        slot: &str,
        width: u32,
        height: u32,
        target: &mut FrameBuffer,
    ) -> Result<(), String> {
        let unpadded_bpr = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;
        let needed_bytes = (padded_bpr as u64) * (height as u64);
        self.ensure_readback_buffer(needed_bytes);

        let rt = self
            .render_targets
            .get(slot)
            .ok_or_else(|| format!("Render target '{slot}' inexistente."))?;
        let (ref staging, _) = self
            .readback_buffer
            .as_ref()
            .ok_or_else(|| "Buffer staging ausente.".to_string())?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismcast-slot-readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &rt.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..needed_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        self.device.poll(wgpu::Maintain::Wait);

        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("Falha map_async readback: {e}")),
            Err(e) => return Err(format!("Falha canal readback: {e}")),
        }

        let expected_len = (unpadded_bpr as usize).saturating_mul(height as usize);
        if target.bgra.len() != expected_len {
            target.bgra.resize(expected_len, 0);
        }
        target.width = width;
        target.height = height;

        {
            let data = slice.get_mapped_range();
            let row_len = unpadded_bpr as usize;
            let stride = padded_bpr as usize;
            for row in 0..(height as usize) {
                let src_off = row * stride;
                let dst_off = row * row_len;
                target.bgra[dst_off..dst_off + row_len]
                    .copy_from_slice(&data[src_off..src_off + row_len]);
            }
        }
        staging.unmap();
        Ok(())
    }
}

impl CompositorBackend for WgpuBackend {
    fn compose_scene(
        &mut self,
        target: &mut FrameBuffer,
        scene: &Scene,
        vertical: bool,
        captures: &CaptureManager,
        studio: &StudioCompositorSettings,
    ) {
        let w = target.width;
        let h = target.height;
        if w == 0 || h == 0 {
            return;
        }
        let slot = if vertical { "__rt_vert" } else { "__rt_horiz" };
        if self
            .render_to_gpu_texture(slot, w, h, scene, vertical, captures, studio)
            .is_ok()
        {
            let _ = self.readback_slot_to_framebuffer(slot, w, h, target);
        }
    }

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
        let w = target.width;
        let h = target.height;
        if w == 0 || h == 0 {
            return;
        }
        if self
            .render_transition_to_gpu_texture(
                "__rt_transition",
                w,
                h,
                outgoing,
                incoming,
                transition,
                progress,
                elapsed_ms,
                custom,
                stinger_frame,
                luma_matte,
            )
            .is_ok()
        {
            let _ = self.readback_slot_to_framebuffer("__rt_transition", w, h, target);
        }
    }

    fn backend_info(&self) -> (String, String, String) {
        (
            "wgpu".to_string(),
            self.adapter_name.clone(),
            self.driver_info.clone(),
        )
    }

    fn active_textures(&self) -> usize {
        self.texture_cache.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TransitionEasing;

    fn assert_send<T: Send>() {}

    #[test]
    fn test_wgpu_backend_is_send() {
        assert_send::<WgpuBackend>();
    }

    #[test]
    fn test_validate_custom_wgsl_accepts_safe_shader_and_blocks_unsafe() {
        // Shader customizado válido (somente fragment entry point)
        let valid = r#"
            // Comentário com palavra loop e while não deve bloquear!
            @fragment
            fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                let a = textureSampleLevel(tex_out, samp, in.uv, 0.0);
                let b = textureSampleLevel(tex_in, samp, in.uv, 0.0);
                return mix(a, b, u.control.x);
            }
        "#;
        assert!(validate_custom_wgsl(valid).is_ok());

        // Built-in TRANSITION_WGSL também deve passar na validação!
        assert!(validate_custom_wgsl(TRANSITION_WGSL).is_ok());
        assert!(validate_custom_wgsl(LAYER_COMPOSITE_WGSL).is_ok());

        // Vazio -> rejeitado
        assert!(validate_custom_wgsl("   ").is_err());

        // Sem @fragment -> rejeitado
        assert!(validate_custom_wgsl("fn fs_main() -> vec4<f32> { return vec4<f32>(0.0); }").is_err());

        // Sem fn fs_main -> rejeitado
        assert!(validate_custom_wgsl("@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }").is_err());

        // Loop infinito via `loop` -> bloqueado
        let bad_loop = r#"
            @fragment
            fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                loop { break; }
                return vec4<f32>(1.0);
            }
        "#;
        assert!(validate_custom_wgsl(bad_loop).is_err());

        // Loop infinito via `while` -> bloqueado
        let bad_while = r#"
            @fragment
            fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                var i = 0;
                while (i < 10) { i = i + 1; }
                return vec4<f32>(1.0);
            }
        "#;
        assert!(validate_custom_wgsl(bad_while).is_err());

        // `for (;;)` infinito -> bloqueado
        let bad_for = r#"
            @fragment
            fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                for (;;) {}
                return vec4<f32>(1.0);
            }
        "#;
        assert!(validate_custom_wgsl(bad_for).is_err());

        // Compute shader -> bloqueado
        let bad_compute = r#"
            @compute @workgroup_size(1)
            @fragment
            fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
        "#;
        assert!(validate_custom_wgsl(bad_compute).is_err());

        // Chaves desbalanceadas -> bloqueado
        let bad_braces = "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0);";
        assert!(validate_custom_wgsl(bad_braces).is_err());
    }

    #[test]
    fn test_luma_matte_presets_and_mask_math() {
        // Presets devem estar em [0.0, 1.0] e seguir geometria esperada
        assert!((eval_luma_preset("horizontal-wipe", 0.0, 0.5) - 0.0).abs() < 1e-5);
        assert!((eval_luma_preset("horizontal-wipe", 1.0, 0.5) - 1.0).abs() < 1e-5);
        assert!((eval_luma_preset("vertical-wipe", 0.5, 0.25) - 0.25).abs() < 1e-5);
        assert!((eval_luma_preset("radial", 0.5, 0.5) - 0.0).abs() < 1e-5);
        assert!((eval_luma_preset("radial", 0.0, 0.0) - 1.0).abs() < 1e-4);
        assert!((eval_luma_preset("diagonal", 0.5, 0.5) - 0.5).abs() < 1e-5);
        assert!((eval_luma_preset("barn-door", 0.5, 0.5) - 0.0).abs() < 1e-5);
        assert!((eval_luma_preset("barn-door", 0.0, 0.5) - 1.0).abs() < 1e-5);
        assert!((eval_luma_preset("barn-door", 1.0, 0.5) - 1.0).abs() < 1e-5);

        // Em progress = 0.0 a máscara deve ser 0.0 (100% outgoing) para qualquer luma
        for &luma in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!(eval_luma_mask(luma, 0.0, 0.15, false) <= 0.001);
            assert!(eval_luma_mask(luma, 1.0, 0.15, false) >= 0.999);
        }

        // Em progress = 0.5, luma baixo (0.1) já virou incoming (1.0) e luma alto (0.9) ainda é outgoing (0.0)
        assert!(eval_luma_mask(0.1, 0.5, 0.1, false) > 0.99);
        assert!(eval_luma_mask(0.9, 0.5, 0.1, false) < 0.01);

        // Com invert=true o comportamento se inverte
        assert!(eval_luma_mask(0.1, 0.5, 0.1, true) < 0.01);
        assert!(eval_luma_mask(0.9, 0.5, 0.1, true) > 0.99);
    }

    #[test]
    fn test_easing_curves_boundaries_and_monotonicity() {
        for easing in [
            TransitionEasing::Linear,
            TransitionEasing::EaseInOut,
            TransitionEasing::CubicOut,
            TransitionEasing::ExpoOut,
        ] {
            assert!((easing.apply(0.0) - 0.0).abs() < 1e-5);
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-5);
            let mut prev = 0.0f32;
            for step in 1..=20 {
                let t = step as f32 / 20.0;
                let v = easing.apply(t);
                assert!(v >= prev - 1e-5, "easing {easing:?} not monotonic at t={t}");
                prev = v;
            }
        }
        // CubicOut e ExpoOut aceleram no início em relação ao Linear
        assert!(TransitionEasing::CubicOut.apply(0.25) > TransitionEasing::Linear.apply(0.25));
        assert!(TransitionEasing::ExpoOut.apply(0.25) > TransitionEasing::CubicOut.apply(0.25));
        // EaseInOut é mais lento no início que Linear, mas igual a 0.5 no ponto médio
        assert!(TransitionEasing::EaseInOut.apply(0.25) < TransitionEasing::Linear.apply(0.25));
        assert!((TransitionEasing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_wgpu_backend_try_new_graceful() {
        match WgpuBackend::try_new() {
            Ok(mut backend) => {
                assert!(!backend.adapter_name().is_empty());
                assert!(!backend.driver_info().is_empty());
                let (name, adapter, driver) = backend.backend_info();
                assert_eq!(name, "wgpu");
                assert!(!adapter.is_empty());
                assert!(!driver.is_empty());

                // Render mínimo 4x4 validando pipelines e readback alinhado a 256 bytes
                let mut target = FrameBuffer::new(4, 4);
                let scene = Scene::default();
                let captures = CaptureManager::new(60);
                let studio = StudioCompositorSettings::default();
                backend.compose_scene(&mut target, &scene, false, &captures, &studio);
                assert_eq!(target.bgra.len(), 4 * 4 * 4);
                assert_eq!(&target.bgra[0..4], &[0, 0, 0, 255]);
            }
            Err(err) => {
                // Em CI headless sem GPU/Vulkan ICD deve falhar de forma limpa sem panicar
                assert!(!err.is_empty());
            }
        }
    }
}

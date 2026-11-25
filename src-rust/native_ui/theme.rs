//! Sistema visual do PrismCast Studio — inspirado diretamente na landing page GitHub Pages.
//!
//! Paleta Broadcast Carbon + Glassmorphism Espectral:
//! - Chassi carbono fosco (`#141412`, `#1C1C19`, `#242420`) com painéis translúcidos foscos
//!   e borda especular superior de 1px.
//! - Tipografia marfim quente (`#F1EDE4`, `#A19B8E`, `#706B60`).
//! - Vermilion Tally de broadcast (`#D63626`) para estado ao vivo e ações primárias.
//! - Espectro óptico do prisma (`#FF3E52`, `#FFAE2B`, `#18DCB8`, `#3892FF`) em feixes atmosféricos
//!   suaves atrás dos painéis de vidro e no logotipo vetorial nativo.

use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Frame, Margin, Painter, Pos2, Rect, Shadow,
    Shape, Stroke, StrokeKind, Vec2, Visuals,
};

// ------------------------------------------------------------------ Paleta GH Pages

pub const CHASSIS: Color32 = Color32::from_rgb(0x14, 0x14, 0x12);
pub const CHASSIS_2: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x19);
pub const CHASSIS_3: Color32 = Color32::from_rgb(0x24, 0x24, 0x20);
pub const CHASSIS_RULE: Color32 = Color32::from_rgb(0x2E, 0x2D, 0x28);
pub const CHASSIS_RULE_STRONG: Color32 = Color32::from_rgb(0x3F, 0x3D, 0x36);

pub const INK: Color32 = Color32::from_rgb(0xF1, 0xED, 0xE4);
pub const MUTED: Color32 = Color32::from_rgb(0xA1, 0x9B, 0x8E);
pub const DIM: Color32 = Color32::from_rgb(0x70, 0x6B, 0x60);

pub const TALLY: Color32 = Color32::from_rgb(0xD6, 0x36, 0x26);
pub const TALLY_DARK: Color32 = Color32::from_rgb(0xB3, 0x29, 0x1B);
pub const TALLY_SOFT: Color32 = Color32::from_rgba_premultiplied(214, 54, 38, 42);

pub const METER_OK: Color32 = Color32::from_rgb(0x33, 0xB0, 0x6B);
pub const METER_WARN: Color32 = Color32::from_rgb(0xD9, 0x8A, 0x22);
pub const METER_CLIP: Color32 = Color32::from_rgb(0xE0, 0x3E, 0x2D);

pub const SPEC_RED: Color32 = Color32::from_rgb(0xFF, 0x3E, 0x52);
pub const SPEC_AMBER: Color32 = Color32::from_rgb(0xFF, 0xAE, 0x2B);
pub const SPEC_TEAL: Color32 = Color32::from_rgb(0x18, 0xDC, 0xB8);
pub const SPEC_BLUE: Color32 = Color32::from_rgb(0x38, 0x92, 0xFF);

const fn rgba_const(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    let pr = ((r as u16 * a as u16 + 127) / 255) as u8;
    let pg = ((g as u16 * a as u16 + 127) / 255) as u8;
    let pb = ((b as u16 * a as u16 + 127) / 255) as u8;
    Color32::from_rgba_premultiplied(pr, pg, pb, a)
}

// Superfícies translúcidas (frosted glass sobre o campo espectral)
pub const GLASS_PANEL: Color32 = rgba_const(28, 28, 25, 214);
pub const GLASS_CARD: Color32 = rgba_const(36, 36, 32, 198);
pub const GLASS_CARD_HOVER: Color32 = rgba_const(48, 47, 41, 224);
pub const GLASS_MODAL: Color32 = rgba_const(24, 24, 21, 242);
pub const GLASS_EDGE: Color32 = rgba_const(241, 237, 228, 30);
pub const GLASS_EDGE_STRONG: Color32 = rgba_const(241, 237, 228, 56);
pub const GLASS_SPECULAR: Color32 = rgba_const(255, 253, 248, 36);

pub fn apply_prismcast_theme(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.window_fill = GLASS_MODAL;
    visuals.faint_bg_color = Color32::from_rgba_unmultiplied(255, 253, 248, 8);
    visuals.extreme_bg_color = Color32::from_rgb(0x0E, 0x0E, 0x0C);
    visuals.code_bg_color = Color32::from_rgb(0x18, 0x18, 0x15);
    visuals.override_text_color = Some(INK);
    visuals.hyperlink_color = SPEC_TEAL;
    visuals.warn_fg_color = SPEC_AMBER;
    visuals.error_fg_color = TALLY;

    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(214, 54, 38, 68);
    visuals.selection.stroke = Stroke::new(1.0_f32, TALLY);

    visuals.window_corner_radius = CornerRadius::same(16);
    visuals.window_stroke = Stroke::new(1.0_f32, GLASS_EDGE_STRONG);
    visuals.window_shadow = Shadow {
        offset: [0, 14],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    visuals.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(135),
    };

    visuals.widgets.noninteractive.bg_fill = GLASS_CARD;
    visuals.widgets.noninteractive.weak_bg_fill = GLASS_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, GLASS_EDGE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, INK);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(10);

    visuals.widgets.inactive.bg_fill = Color32::from_rgba_unmultiplied(40, 39, 34, 200);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgba_unmultiplied(34, 34, 30, 190);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, GLASS_EDGE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, INK);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(9);

    visuals.widgets.hovered.bg_fill = GLASS_CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = GLASS_CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, GLASS_EDGE_STRONG);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.2_f32, INK);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(9);

    visuals.widgets.active.bg_fill = Color32::from_rgba_unmultiplied(56, 54, 46, 235);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgba_unmultiplied(56, 54, 46, 235);
    visuals.widgets.active.bg_stroke = Stroke::new(1.2_f32, SPEC_AMBER);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, INK);
    visuals.widgets.active.corner_radius = CornerRadius::same(9);

    visuals.widgets.open.bg_fill = GLASS_CARD_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, GLASS_EDGE_STRONG);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, INK);
    visuals.widgets.open.corner_radius = CornerRadius::same(9);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(11.0, 6.0);
    style.spacing.window_margin = Margin::same(14);
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.interaction.tooltip_delay = 0.35;
    ctx.set_style(style);
}

/// Pinta o fundo carbono fosco + feixes espectrais suaves do prisma (efeito `.spectrum-field` do GH Pages).
pub fn paint_spectral_backdrop(painter: &Painter, rect: Rect, time_sec: f64) {
    painter.rect_filled(rect, 0.0, CHASSIS);

    let t = time_sec as f32;
    let drift_a = Vec2::new((t * 0.12).sin() * 26.0, (t * 0.09).cos() * 20.0);
    let drift_b = Vec2::new((t * 0.10).cos() * 30.0, (t * 0.13).sin() * 22.0);
    let max_dim = rect.width().max(rect.height());

    // 4 feixes suaves (vermelho, âmbar, turquesa, azul) idênticos à página GH Pages
    let beams = [
        (
            Pos2::new(rect.right() - rect.width() * 0.10, rect.top() + rect.height() * 0.06)
                + drift_a,
            max_dim * 0.36,
            SPEC_RED,
            18u8,
        ),
        (
            Pos2::new(rect.left() + rect.width() * 0.08, rect.top() + rect.height() * 0.34)
                + drift_b,
            max_dim * 0.33,
            SPEC_AMBER,
            16u8,
        ),
        (
            Pos2::new(rect.right() - rect.width() * 0.18, rect.bottom() - rect.height() * 0.08)
                - drift_a,
            max_dim * 0.38,
            SPEC_TEAL,
            15u8,
        ),
        (
            Pos2::new(rect.right() - rect.width() * 0.04, rect.top() + rect.height() * 0.56)
                - drift_b,
            max_dim * 0.32,
            SPEC_BLUE,
            14u8,
        ),
    ];

    for (center, radius, color, peak_alpha) in beams {
        // Anéis concêntricos simulando blur radial suave sem custo de shader extra
        let steps = 7usize;
        for i in 0..steps {
            let frac = (i + 1) as f32 / steps as f32;
            let r = radius * frac;
            let falloff = (1.0 - frac).powf(1.45);
            let a = ((peak_alpha as f32) * falloff).round().clamp(1.0, 40.0) as u8;
            painter.circle_filled(
                center,
                r,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
            );
        }
    }

    // Linha espectral sutil no topo da janela (identidade óptica do PrismCast)
    let bar_h = 2.0;
    let segments = [
        (0.0, 0.25, SPEC_RED),
        (0.25, 0.50, SPEC_AMBER),
        (0.50, 0.75, SPEC_TEAL),
        (0.75, 1.0, SPEC_BLUE),
    ];
    for (a, b, col) in segments {
        let seg_rect = Rect::from_min_max(
            Pos2::new(rect.left() + rect.width() * a, rect.top()),
            Pos2::new(rect.left() + rect.width() * b, rect.top() + bar_h),
        );
        painter.rect_filled(
            seg_rect,
            0.0,
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 170),
        );
    }
}

/// Frame de painel glassmorphism (header, rail lateral, docks).
pub fn glass_panel_frame() -> Frame {
    Frame::new()
        .fill(GLASS_PANEL)
        .stroke(Stroke::new(1.0_f32, GLASS_EDGE))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(10))
        .shadow(Shadow {
            offset: [0, 8],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

/// Frame de cartão glassmorphism interno (fontes, cenas, canais de áudio, destinos).
pub fn glass_card_frame(selected: bool) -> Frame {
    Frame::new()
        .fill(if selected {
            Color32::from_rgba_unmultiplied(48, 46, 39, 224)
        } else {
            GLASS_CARD
        })
        .stroke(Stroke::new(
            if selected { 1.3_f32 } else { 1.0_f32 },
            if selected { SPEC_AMBER } else { GLASS_EDGE },
        ))
        .corner_radius(CornerRadius::same(11))
        .inner_margin(Margin::symmetric(10, 8))
}

/// Pinta linha especular fina no topo de um painel de vidro para profundidade tátil.
pub fn paint_glass_top_sheen(painter: &Painter, rect: Rect) {
    let y = rect.top() + 1.0;
    painter.line_segment(
        [Pos2::new(rect.left() + 12.0, y), Pos2::new(rect.right() - 12.0, y)],
        Stroke::new(1.0_f32, GLASS_SPECULAR),
    );
}

/// Logotipo vetorial nativo do PrismCast (prisma óptico refratando luz branca no espectro).
pub fn paint_prism_logo(painter: &Painter, rect: Rect) {
    let c = rect.center();
    let s = rect.width().min(rect.height());

    // Raio incidente branco quente à esquerda
    painter.line_segment(
        [
            Pos2::new(rect.left() + s * 0.04, c.y + s * 0.08),
            Pos2::new(c.x - s * 0.14, c.y - s * 0.01),
        ],
        Stroke::new(1.8_f32, INK),
    );

    // 4 feixes espectrais saindo à direita
    let rays = [
        (-0.18, SPEC_RED),
        (-0.05, SPEC_AMBER),
        (0.08, SPEC_TEAL),
        (0.21, SPEC_BLUE),
    ];
    for (dy, col) in rays {
        painter.line_segment(
            [
                Pos2::new(c.x + s * 0.12, c.y - s * 0.02 + dy * s * 0.25),
                Pos2::new(rect.right() - s * 0.03, c.y + dy * s),
            ],
            Stroke::new(1.6_f32, col),
        );
    }

    // Triângulo de vidro do prisma
    let pts = vec![
        Pos2::new(c.x - s * 0.02, rect.top() + s * 0.12),
        Pos2::new(c.x - s * 0.30, rect.bottom() - s * 0.14),
        Pos2::new(c.x + s * 0.26, rect.bottom() - s * 0.14),
    ];
    painter.add(Shape::convex_polygon(
        pts.clone(),
        Color32::from_rgba_unmultiplied(241, 237, 228, 34),
        Stroke::new(1.5_f32, INK),
    ));
}

/// Pílula broadcast Tally (`AO VIVO` em vermelho vermilion ou `FORA DO AR` em carbono fosco).
pub fn paint_tally_pill(ui: &mut egui::Ui, live: bool, recording: bool, time_sec: f64) {
    let label = if live && recording {
        "AO VIVO · REC"
    } else if live {
        "AO VIVO"
    } else if recording {
        "GRAVANDO"
    } else {
        "FORA DO AR"
    };

    let desired = Vec2::new(if live && recording { 126.0 } else { 104.0 }, 28.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let p = ui.painter();

    let (bg, border, dot_col, text_col) = if live {
        let pulse = ((time_sec * 3.2).sin() * 0.5 + 0.5) as f32;
        let alpha = (42.0 + pulse * 36.0) as u8;
        (
            Color32::from_rgba_unmultiplied(214, 54, 38, alpha),
            TALLY,
            SPEC_RED,
            INK,
        )
    } else if recording {
        (
            Color32::from_rgba_unmultiplied(217, 138, 34, 46),
            METER_WARN,
            SPEC_AMBER,
            INK,
        )
    } else {
        (GLASS_CARD, GLASS_EDGE_STRONG, DIM, MUTED)
    };

    p.rect_filled(rect, CornerRadius::same(14), bg);
    p.rect_stroke(rect, CornerRadius::same(14), Stroke::new(1.0_f32, border), StrokeKind::Inside);

    let dot_center = Pos2::new(rect.left() + 14.0, rect.center().y);
    if live {
        p.circle_filled(
            dot_center,
            6.0,
            Color32::from_rgba_unmultiplied(255, 62, 82, 70),
        );
    }
    p.circle_filled(dot_center, 3.6, dot_col);
    p.text(
        Pos2::new(rect.left() + 24.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::monospace(11.0),
        text_col,
    );
}

/// Medidor VU segmentado estilo console broadcast (`#33B06B` -> `#D98A22` -> `#E03E2D`).
pub fn paint_vu_meter(ui: &mut egui::Ui, peak_db: f32, muted: bool, width: f32) {
    let height = 7.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(40.0), height), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(0x11, 0x11, 0x0F));

    let norm = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
    let segments = 24usize;
    let gap = 1.5;
    let seg_w = (rect.width() - gap * (segments as f32 - 1.0)) / segments as f32;
    let lit = (norm * segments as f32).round() as usize;

    for i in 0..segments {
        let x = rect.left() + i as f32 * (seg_w + gap);
        let seg_rect = Rect::from_min_size(Pos2::new(x, rect.top() + 1.0), Vec2::new(seg_w.max(1.0), height - 2.0));
        let frac = i as f32 / segments as f32;
        let base_col = if muted {
            DIM
        } else if frac > 0.86 {
            METER_CLIP
        } else if frac > 0.68 {
            METER_WARN
        } else {
            METER_OK
        };
        let col = if i < lit {
            base_col
        } else {
            Color32::from_rgba_unmultiplied(base_col.r(), base_col.g(), base_col.b(), 28)
        };
        p.rect_filled(seg_rect, CornerRadius::same(1), col);
    }
}

/// Botão primário em vermilion Tally (usado em "Entrar ao vivo", "Confirmar", etc.).
pub fn tally_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(INK).strong())
            .fill(TALLY)
            .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 45)))
            .corner_radius(CornerRadius::same(9)),
    )
}

/// Botão de ação secundária translúcido com destaque opcional.
pub fn glass_toggle_button(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let fill = if active {
        Color32::from_rgba_unmultiplied(24, 220, 184, 42)
    } else {
        GLASS_CARD
    };
    let stroke = if active {
        Stroke::new(1.1_f32, SPEC_TEAL)
    } else {
        Stroke::new(1.0_f32, GLASS_EDGE)
    };
    let label_color = if active { INK } else { MUTED };
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(label_color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(9)),
    )
}

//! Sistema visual do PrismCast Studio alimentado por `egui-prismglass`.
//!
//! Combina a identidade Broadcast Carbon + Espectro Óptico do PrismCast com o motor
//! de design system 3D glassmorphism / Minimal Solid da biblioteca `egui-prismglass`:
//! - 6 temas prontos (`Prism Carbon`, `Onyx Dark`, `Glacial Glass`, `Frost Light`,
//!   `Minimal Dark`, `Minimal Light`) com adaptação automática de contraste escuro/claro.
//! - Superfícies translúcidas com chanfro direcional 3D, bordas especulares de 1px e
//!   sombras elevadas via `GlassCard`, `GlassPanel`, `GlassSidebar` e `paint_glass_surface`.
//! - Animações físicas com mola (`animate_hover`, `animate_pulse`, `animate_spring`).

use crate::protocol::{UiAppearanceSettings, UiThemePreset};
use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Margin, Painter, Pos2, Rect, Sense, Shadow,
    Shape, Stroke, StrokeKind, Ui, Vec2,
};
use egui_prismglass::painter::paint_glass_gradient_fill;
use egui_prismglass::{
    GlassElevation, GlassSurfaceTokens, PrismContextExt, PrismPalette, PrismTheme, RadiusTokens,
    SpacingTokens, SurfaceStyle, ThemeMode, animate_hover, animate_pulse, paint_glass_surface,
    paint_prism_backdrop, paint_radial_glow_orb,
};

// ------------------------------------------------------------------ Cores de Marca Broadcast

pub const TALLY: Color32 = Color32::from_rgb(0xD6, 0x36, 0x26);
pub const TALLY_HOVER: Color32 = Color32::from_rgb(0xE6, 0x45, 0x35);
pub const TALLY_DARK: Color32 = Color32::from_rgb(0xB3, 0x29, 0x1B);

pub const METER_OK: Color32 = Color32::from_rgb(0x33, 0xB0, 0x6B);
pub const METER_WARN: Color32 = Color32::from_rgb(0xD9, 0x8A, 0x22);
pub const METER_CLIP: Color32 = Color32::from_rgb(0xE0, 0x3E, 0x2D);

pub const SPEC_RED: Color32 = Color32::from_rgb(0xFF, 0x3E, 0x52);
pub const SPEC_AMBER: Color32 = Color32::from_rgb(0xFF, 0xAE, 0x2B);
pub const SPEC_TEAL: Color32 = Color32::from_rgb(0x18, 0xDC, 0xB8);
pub const SPEC_BLUE: Color32 = Color32::from_rgb(0x38, 0x92, 0xFF);

// ------------------------------------------------------------------ Construção de Temas

/// Constrói o tema exclusivo **PrismCast Broadcast Carbon** sobre `egui-prismglass`:
/// chassi carbono fosco quente, vidro fumê translúcido com chanfro especular 3D,
/// tipografia marfim quente e reflexos sutis do espectro óptico do prisma.
pub fn prism_carbon_theme() -> PrismTheme {
    let palette = PrismPalette {
        canvas: Color32::from_rgb(0x11, 0x11, 0x0F),
        canvas_secondary: Color32::from_rgb(0x19, 0x18, 0x15),
        orb_primary: Color32::from_rgb(0xE8, 0x9C, 0x38),
        orb_secondary: Color32::from_rgb(0x18, 0xDC, 0xB8),
        orb_accent: Color32::from_rgb(0xF1, 0xED, 0xE4),
        text_primary: Color32::from_rgb(0xF1, 0xED, 0xE4),
        text_secondary: Color32::from_rgb(0xB3, 0xAC, 0x9E),
        text_muted: Color32::from_rgb(0x7C, 0x76, 0x6A),
        accent: SPEC_TEAL,
        accent_warm: SPEC_AMBER,
        success: METER_OK,
        warning: SPEC_AMBER,
        danger: TALLY,
    };

    let radius = RadiusTokens {
        sm: 8.0,
        md: 10.0,
        lg: 14.0,
        xl: 18.0,
        pill: 999.0,
    };

    let surface_subtle = GlassSurfaceTokens {
        tint: Color32::from_rgba_unmultiplied(0x21, 0x20, 0x1C, 152),
        gradient_top: Color32::from_rgba_unmultiplied(0x2C, 0x2B, 0x25, 162),
        gradient_bottom: Color32::from_rgba_unmultiplied(0x18, 0x18, 0x15, 166),
        border: Color32::from_rgba_unmultiplied(0xF1, 0xED, 0xE4, 24),
        specular_top: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 68),
        specular_accent: Color32::from_rgba_unmultiplied(0xFF, 0xE4, 0xB8, 62),
        inner_glow: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 16),
        shadow: Shadow::NONE,
        corner_radius: radius.md,
        blur_radius: 18.0,
        border_width: 1.0,
        prism_intensity: 0.30,
    };

    let surface_card = GlassSurfaceTokens {
        tint: Color32::from_rgba_unmultiplied(0x1E, 0x1E, 0x1A, 184),
        gradient_top: Color32::from_rgba_unmultiplied(0x2B, 0x2A, 0x24, 190),
        gradient_bottom: Color32::from_rgba_unmultiplied(0x16, 0x16, 0x13, 202),
        border: Color32::from_rgba_unmultiplied(0xF1, 0xED, 0xE4, 32),
        specular_top: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 98),
        specular_accent: Color32::from_rgba_unmultiplied(0xFF, 0xD7, 0x9C, 86),
        inner_glow: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 22),
        shadow: Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, 150),
        },
        corner_radius: radius.lg,
        blur_radius: 24.0,
        border_width: 1.0,
        prism_intensity: 0.40,
    };

    let surface_floating = GlassSurfaceTokens {
        tint: Color32::from_rgba_unmultiplied(0x1A, 0x1A, 0x17, 226),
        gradient_top: Color32::from_rgba_unmultiplied(0x28, 0x27, 0x22, 232),
        gradient_bottom: Color32::from_rgba_unmultiplied(0x13, 0x13, 0x11, 240),
        border: Color32::from_rgba_unmultiplied(0xF1, 0xED, 0xE4, 48),
        specular_top: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 130),
        specular_accent: Color32::from_rgba_unmultiplied(0xFF, 0xDE, 0xAA, 110),
        inner_glow: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 30),
        shadow: Shadow {
            offset: [0, 18],
            blur: 42,
            spread: 1,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, 185),
        },
        corner_radius: radius.xl,
        blur_radius: 30.0,
        border_width: 1.0,
        prism_intensity: 0.50,
    };

    let surface_sidebar = GlassSurfaceTokens {
        tint: Color32::from_rgba_unmultiplied(0x19, 0x19, 0x16, 192),
        gradient_top: Color32::from_rgba_unmultiplied(0x24, 0x23, 0x1F, 198),
        gradient_bottom: Color32::from_rgba_unmultiplied(0x13, 0x13, 0x10, 212),
        border: Color32::from_rgba_unmultiplied(0xF1, 0xED, 0xE4, 30),
        specular_top: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 86),
        specular_accent: Color32::from_rgba_unmultiplied(0xFF, 0xD9, 0xA0, 74),
        inner_glow: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 20),
        shadow: Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, 145),
        },
        corner_radius: radius.lg,
        blur_radius: 26.0,
        border_width: 1.0,
        prism_intensity: 0.34,
    };

    let surface_panel = GlassSurfaceTokens {
        tint: Color32::from_rgba_unmultiplied(0x1B, 0x1B, 0x18, 188),
        gradient_top: Color32::from_rgba_unmultiplied(0x26, 0x25, 0x20, 194),
        gradient_bottom: Color32::from_rgba_unmultiplied(0x14, 0x14, 0x12, 204),
        border: Color32::from_rgba_unmultiplied(0xF1, 0xED, 0xE4, 30),
        specular_top: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 88),
        specular_accent: Color32::from_rgba_unmultiplied(0xFF, 0xD9, 0xA0, 76),
        inner_glow: Color32::from_rgba_unmultiplied(0xFF, 0xFD, 0xF7, 18),
        shadow: Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, 130),
        },
        corner_radius: radius.lg,
        blur_radius: 22.0,
        border_width: 1.0,
        prism_intensity: 0.34,
    };

    PrismTheme {
        name: "Prism Carbon".to_owned(),
        mode: ThemeMode::Dark,
        surface_style: SurfaceStyle::Glass,
        palette,
        spacing: SpacingTokens::default(),
        radius,
        surface_subtle,
        surface_card,
        surface_floating,
        surface_sidebar,
        surface_panel,
        glass_opacity_scale: 1.0,
        specular_scale: 1.0,
        radius_scale: 1.0,
        prism_scale: 1.0,
    }
}

/// Constrói o `PrismTheme` completo a partir das preferências persistidas do usuário.
pub fn build_prism_theme(appearance: &UiAppearanceSettings) -> PrismTheme {
    let mut theme = match appearance.preset {
        UiThemePreset::PrismCarbon => prism_carbon_theme(),
        UiThemePreset::Onyx => PrismTheme::onyx(),
        UiThemePreset::Glacial => PrismTheme::glacial(),
        UiThemePreset::Frost => PrismTheme::frost(),
        UiThemePreset::MinimalDark => PrismTheme::minimal_dark(),
        UiThemePreset::MinimalLight => PrismTheme::minimal_light(),
    };
    theme.glass_opacity_scale = appearance.glass_opacity.clamp(0.45, 1.55);
    theme.specular_scale = appearance.specular_intensity.clamp(0.0, 1.8);
    theme.radius_scale = appearance.radius_scale.clamp(0.5, 1.4);
    theme
}

/// Aplica o tema `egui-prismglass` ao contexto `egui` com ajustes ergonômicos para estúdio broadcast.
pub fn apply_prismcast_theme(ctx: &Context, appearance: &UiAppearanceSettings) -> PrismTheme {
    let theme = build_prism_theme(appearance);
    ctx.set_prism_theme(theme.clone());

    // Mantém painéis nativos transparentes para que o backdrop 3D da `egui-prismglass`
    // apareça por trás dos painéis e cartões translúcidos.
    let popup_shadow = Shadow {
        offset: [0, 8],
        blur: 22,
        spread: 0,
        color: match theme.mode {
            ThemeMode::Dark => Color32::from_black_alpha(140),
            ThemeMode::Light => Color32::from_rgba_unmultiplied(18, 19, 23, 28),
        },
    };
    let code_bg_color = match theme.mode {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(24, 24, 22, 220),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(232, 236, 244, 220),
    };

    ctx.all_styles_mut(|style| {
        style.visuals.panel_fill = Color32::TRANSPARENT;
        style.visuals.popup_shadow = popup_shadow;
        style.visuals.code_bg_color = code_bg_color;
        style.spacing.item_spacing = Vec2::new(8.0, 6.5);
        style.spacing.button_padding = Vec2::new(11.0, 6.0);
        style.spacing.window_margin = Margin::same(14);
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;
        style.interaction.tooltip_delay = 0.30;
    });

    theme
}

// ------------------------------------------------------------------ Backdrop Volumétrico

/// Pinta o fundo volumétrico 3D de `egui-prismglass` enriquecido com a refração óptica
/// do prisma no preset `Prism Carbon` e a linha espectral superior de 2px.
pub fn paint_studio_backdrop(
    painter: &Painter,
    rect: Rect,
    theme: &PrismTheme,
    appearance: &UiAppearanceSettings,
    time_sec: f64,
) {
    let anim_t = if appearance.animate_backdrop {
        time_sec as f32
    } else {
        0.0
    };
    let intensity = appearance.backdrop_intensity.clamp(0.0, 1.4);

    paint_prism_backdrop(painter, rect, theme, anim_t, intensity);

    // No tema Prism Carbon (modo vidro), acrescenta refração espectral suave (vermelho/âmbar/turquesa/azul)
    if theme.surface_style == SurfaceStyle::Glass
        && appearance.preset == UiThemePreset::PrismCarbon
        && intensity > 0.01
    {
        let max_dim = rect.width().max(rect.height());
        let drift_a = Vec2::new((anim_t * 0.14).sin() * 24.0, (anim_t * 0.11).cos() * 18.0);
        let drift_b = Vec2::new((anim_t * 0.11).cos() * 26.0, (anim_t * 0.15).sin() * 20.0);

        paint_radial_glow_orb(
            painter,
            Pos2::new(rect.right() - rect.width() * 0.12, rect.top() + rect.height() * 0.12)
                + drift_a,
            max_dim * 0.34,
            Color32::from_rgba_unmultiplied(
                SPEC_RED.r(),
                SPEC_RED.g(),
                SPEC_RED.b(),
                (20.0 * intensity).round().clamp(0.0, 48.0) as u8,
            ),
        );
        paint_radial_glow_orb(
            painter,
            Pos2::new(rect.left() + rect.width() * 0.14, rect.bottom() - rect.height() * 0.18)
                + drift_b,
            max_dim * 0.36,
            Color32::from_rgba_unmultiplied(
                SPEC_TEAL.r(),
                SPEC_TEAL.g(),
                SPEC_TEAL.b(),
                (18.0 * intensity).round().clamp(0.0, 44.0) as u8,
            ),
        );
        paint_radial_glow_orb(
            painter,
            Pos2::new(rect.right() - rect.width() * 0.20, rect.bottom() - rect.height() * 0.10)
                - drift_a,
            max_dim * 0.30,
            Color32::from_rgba_unmultiplied(
                SPEC_BLUE.r(),
                SPEC_BLUE.g(),
                SPEC_BLUE.b(),
                (16.0 * intensity).round().clamp(0.0, 40.0) as u8,
            ),
        );
    }

    // Assinatura óptica do prisma: linha espectral ultrafina de 2px no topo da janela
    let bar_h = 2.0;
    let bar_alpha = if theme.surface_style == SurfaceStyle::Minimal {
        115
    } else {
        185
    };
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
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), bar_alpha),
        );
    }
}

// ------------------------------------------------------------------ Helpers de Cor / Tema

/// Cor de destaque translúcida para cartões selecionados (adaptada para Dark e Light).
pub fn selected_card_tint(theme: &PrismTheme) -> Color32 {
    let a = theme.palette.accent;
    match (theme.surface_style, theme.mode) {
        (SurfaceStyle::Minimal, ThemeMode::Light) => Color32::from_rgb(239, 245, 255),
        (SurfaceStyle::Minimal, ThemeMode::Dark) => Color32::from_rgb(31, 37, 48),
        (SurfaceStyle::Glass, ThemeMode::Light) => {
            Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 34)
        }
        (SurfaceStyle::Glass, ThemeMode::Dark) => {
            Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 44)
        }
    }
}

/// Cor de fundo para superfícies de mídia / canvas escuro (palco e miniaturas).
pub fn media_well_fill(theme: &PrismTheme) -> Color32 {
    match theme.mode {
        ThemeMode::Dark => Color32::from_rgb(0x0B, 0x0B, 0x0A),
        ThemeMode::Light => Color32::from_rgb(0x14, 0x16, 0x1B),
    }
}

/// Pinta um container glassmorphism diretamente via `egui-prismglass::paint_glass_surface`.
pub fn paint_elevated_surface(painter: &Painter, rect: Rect, theme: &PrismTheme, elev: GlassElevation) {
    let tokens = theme.surface(elev);
    paint_glass_surface(painter, rect, &tokens);
}

/// Cabeçalho de seção em monospace discreto e legível em todos os temas.
pub fn section_heading(ui: &mut Ui, title: &str, theme: &PrismTheme) {
    ui.label(
        egui::RichText::new(title)
            .monospace()
            .size(10.5)
            .strong()
            .color(theme.palette.text_muted),
    );
}

// ------------------------------------------------------------------ Logotipo Óptico

/// Logotipo vetorial nativo do PrismCast adaptado automaticamente ao tema ativo (escuro ou claro).
pub fn paint_prism_logo(painter: &Painter, rect: Rect, theme: &PrismTheme) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let ink = theme.palette.text_primary;

    // Raio incidente à esquerda
    painter.line_segment(
        [
            Pos2::new(rect.left() + s * 0.04, c.y + s * 0.08),
            Pos2::new(c.x - s * 0.14, c.y - s * 0.01),
        ],
        Stroke::new(1.8_f32, ink),
    );

    // 4 feixes espectrais refratados à direita
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

    // Triângulo translúcido do prisma
    let glass_fill = match theme.mode {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(241, 237, 228, 38),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(18, 19, 23, 22),
    };
    let pts = vec![
        Pos2::new(c.x - s * 0.02, rect.top() + s * 0.12),
        Pos2::new(c.x - s * 0.30, rect.bottom() - s * 0.14),
        Pos2::new(c.x + s * 0.26, rect.bottom() - s * 0.14),
    ];
    painter.add(Shape::convex_polygon(
        pts,
        glass_fill,
        Stroke::new(1.5_f32, ink),
    ));
}

// ------------------------------------------------------------------ Componentes Broadcast

/// Pílula broadcast Tally com pulso suave via `egui_prismglass::animate_pulse`.
pub fn paint_tally_pill(ui: &mut Ui, live: bool, recording: bool) {
    let theme = ui.ctx().prism_theme();
    let label = if live && recording {
        "AO VIVO · REC"
    } else if live {
        "AO VIVO"
    } else if recording {
        "GRAVANDO"
    } else {
        "FORA DO AR"
    };

    let font = FontId::monospace(10.8);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme.palette.text_primary);
    let desired = Vec2::new((galley.size().x + 36.0).max(98.0), 26.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());

    if !ui.is_rect_visible(rect) {
        return;
    }

    let p = ui.painter();
    let pulse = if live || recording {
        animate_pulse(ui.ctx(), if live { 0.85 } else { 0.55 })
    } else {
        0.0
    };

    let (bg, border, dot_col, text_col) = if live {
        let alpha = (34.0 + pulse * 36.0) as u8;
        (
            Color32::from_rgba_unmultiplied(TALLY.r(), TALLY.g(), TALLY.b(), alpha),
            Color32::from_rgba_unmultiplied(TALLY.r(), TALLY.g(), TALLY.b(), 210),
            SPEC_RED,
            if theme.mode == ThemeMode::Light {
                TALLY_DARK
            } else {
                Color32::WHITE
            },
        )
    } else if recording {
        let alpha = (30.0 + pulse * 26.0) as u8;
        (
            Color32::from_rgba_unmultiplied(METER_WARN.r(), METER_WARN.g(), METER_WARN.b(), alpha),
            Color32::from_rgba_unmultiplied(METER_WARN.r(), METER_WARN.g(), METER_WARN.b(), 190),
            SPEC_AMBER,
            theme.palette.text_primary,
        )
    } else {
        let subt = theme.surface(GlassElevation::Subtle);
        (subt.tint, subt.border, theme.palette.text_muted, theme.palette.text_secondary)
    };

    let cr = CornerRadius::same((rect.height() * 0.5).round() as u8);
    p.rect_filled(rect, cr, bg);
    p.rect_stroke(rect, cr, Stroke::new(1.0_f32, border), StrokeKind::Inside);

    let dot_center = Pos2::new(rect.left() + 13.0, rect.center().y);
    if live || recording {
        p.circle_filled(
            dot_center,
            4.8 + pulse * 2.4,
            Color32::from_rgba_unmultiplied(
                dot_col.r(),
                dot_col.g(),
                dot_col.b(),
                (55.0 + pulse * 45.0) as u8,
            ),
        );
    }
    p.circle_filled(dot_center, 3.4, dot_col);
    p.text(
        Pos2::new(rect.left() + 23.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        text_col,
    );
}

/// Medidor VU segmentado estilo console broadcast (`#33B06B` -> `#D98A22` -> `#E03E2D`)
/// com calha translúcida glassmorphism e indicador de pico.
pub fn paint_vu_meter(ui: &mut Ui, peak_db: f32, muted: bool, width: f32) {
    let theme = ui.ctx().prism_theme();
    let height = 8.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(48.0), height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let p = ui.painter();
    let trough_fill = match theme.mode {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(0x0B, 0x0B, 0x0A, 195),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(0x12, 0x13, 0x17, 26),
    };
    let trough_border = match theme.mode {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(255, 255, 255, 18),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(18, 19, 23, 24),
    };
    p.rect_filled(rect, CornerRadius::same(4), trough_fill);
    p.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, trough_border),
        StrokeKind::Inside,
    );

    let inner = rect.shrink2(Vec2::new(2.0, 1.5));
    let norm = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
    let segments = 26usize;
    let gap = 1.5;
    let seg_w = (inner.width() - gap * (segments as f32 - 1.0)) / segments as f32;
    let lit = (norm * segments as f32).round() as usize;

    for i in 0..segments {
        let x = inner.left() + i as f32 * (seg_w + gap);
        let seg_rect = Rect::from_min_size(
            Pos2::new(x, inner.top()),
            Vec2::new(seg_w.max(1.0), inner.height().max(2.0)),
        );
        let frac = i as f32 / segments as f32;
        let base_col = if muted {
            theme.palette.text_muted
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
            let dim_a = if theme.mode == ThemeMode::Light { 38 } else { 24 };
            Color32::from_rgba_unmultiplied(base_col.r(), base_col.g(), base_col.b(), dim_a)
        };
        p.rect_filled(seg_rect, CornerRadius::same(1), col);
    }
}

/// Botão primário em vermilion Tally de broadcast com bevel 3D glass e micro-animação spring.
pub fn tally_button(ui: &mut Ui, text: &str) -> egui::Response {
    tally_button_ex(ui, text, true, true)
}

/// Versão configurável do botão Tally (pill/rounded + habilitado/desabilitado).
pub fn tally_button_ex(ui: &mut Ui, text: &str, pill: bool, enabled: bool) -> egui::Response {
    let theme = ui.ctx().prism_theme();
    let font_id = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), Color32::WHITE);
    let padding = Vec2::new(15.0, 7.0);
    let desired = Vec2::new(
        (galley.size().x + padding.x * 2.0).max(68.0),
        (galley.size().y + padding.y * 2.0).max(32.0),
    );

    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(desired, sense);

    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if ui.is_rect_visible(rect) {
        let hover_t = animate_hover(ui.ctx(), response.id, enabled && response.hovered());
        let pressed = enabled && response.is_pointer_button_down_on();
        let draw_rect = rect.translate(Vec2::new(0.0, if pressed { 0.5 } else { -0.8 * hover_t }));

        let radius = if pill {
            draw_rect.height() * 0.5
        } else {
            theme.radius.md * theme.radius_scale.clamp(0.4, 1.5)
        };

        let (top_col, bot_col) = if pressed {
            (TALLY_DARK, Color32::from_rgb(0x98, 0x22, 0x16))
        } else if hover_t > 0.05 {
            (TALLY_HOVER, TALLY)
        } else {
            (TALLY, TALLY_DARK)
        };

        let alpha = if enabled { 1.0 } else { 0.42 };
        let scale_c = |c: Color32| -> Color32 {
            Color32::from_rgba_unmultiplied(
                c.r(),
                c.g(),
                c.b(),
                (f32::from(c.a()) * alpha).round() as u8,
            )
        };

        paint_glass_gradient_fill(
            ui.painter(),
            draw_rect,
            radius,
            scale_c(top_col),
            scale_c(bot_col),
            Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, (52.0 * alpha) as u8),
            ),
            Color32::from_rgba_unmultiplied(255, 240, 235, (44.0 * alpha) as u8),
        );

        ui.painter().text(
            draw_rect.center(),
            Align2::CENTER_CENTER,
            galley.text(),
            font_id,
            Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * alpha) as u8),
        );
    }

    response
}

/// Botão Tally compacto (`22px` de altura) para cabeçalhos densos como o mixer de áudio.
pub fn tally_button_compact(ui: &mut Ui, text: &str) -> egui::Response {
    let font_id = FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), Color32::WHITE);
    let desired = Vec2::new((galley.size().x + 18.0).max(46.0), 22.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let pressed = response.is_pointer_button_down_on();
        let (top_col, bot_col) = if pressed {
            (TALLY_DARK, Color32::from_rgb(0x98, 0x22, 0x16))
        } else if hovered {
            (TALLY_HOVER, TALLY)
        } else {
            (TALLY, TALLY_DARK)
        };
        paint_glass_gradient_fill(
            ui.painter(),
            rect,
            rect.height() * 0.5,
            top_col,
            bot_col,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 52)),
            Color32::from_rgba_unmultiplied(255, 240, 235, 38),
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            galley.text(),
            font_id,
            Color32::WHITE,
        );
    }

    response
}

/// Botão compacto translúcido de alternância baseado em `GlassButton` da `egui-prismglass`.
pub fn glass_toggle_button(ui: &mut Ui, text: &str, active: bool) -> egui::Response {
    ui.add(
        egui_prismglass::GlassButton::secondary(text)
            .selected(active)
            .pill(true)
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_build_distinct_prism_themes() {
        for preset in UiThemePreset::ALL {
            let appearance = UiAppearanceSettings {
                preset,
                ..Default::default()
            };
            let theme = build_prism_theme(&appearance);
            assert!(!theme.name.is_empty());
            match preset {
                UiThemePreset::Frost | UiThemePreset::MinimalLight => {
                    assert_eq!(theme.mode, ThemeMode::Light);
                }
                _ => assert_eq!(theme.mode, ThemeMode::Dark),
            }
            match preset {
                UiThemePreset::MinimalDark | UiThemePreset::MinimalLight => {
                    assert_eq!(theme.surface_style, SurfaceStyle::Minimal);
                }
                _ => assert_eq!(theme.surface_style, SurfaceStyle::Glass),
            }
        }
    }

    #[test]
    fn apply_prismcast_theme_installs_theme_in_context() {
        let ctx = Context::default();
        let appearance = UiAppearanceSettings {
            preset: UiThemePreset::PrismCarbon,
            glass_opacity: 1.2,
            specular_intensity: 1.35,
            radius_scale: 1.1,
            backdrop_intensity: 0.9,
            animate_backdrop: true,
            sidebar_collapsed: false,
        };
        let applied = apply_prismcast_theme(&ctx, &appearance);
        let read_back = ctx.prism_theme();
        assert_eq!(applied.name, "Prism Carbon");
        assert_eq!(read_back.name, "Prism Carbon");
        assert!((read_back.glass_opacity_scale - 1.2).abs() < 1e-4);
        assert!((read_back.specular_scale - 1.35).abs() < 1e-4);
        assert!((read_back.radius_scale - 1.1).abs() < 1e-4);
    }
}


//! Sistema visual do PrismCast Studio sobre `egui-prismglass` 0.3.
//!
//! A versão 0.3 da biblioteca introduz a **linguagem Prism**: tipografia Inter (com algarismos
//! tabulares), rampa neutra de 12 passos, uma única cor de destaque, bordas hairline e dois
//! materiais — **Layered** (conteúdo sólido, vidro apenas no que flutua) e **All glass**
//! (todos os contêineres em vidro fosco). Este módulo:
//!
//! - Constrói os 10 temas do estúdio (5 Prism + 5 clássicos) e aplica a cor de destaque escolhida.
//! - Recria a identidade **Prism Carbon** (neutros carbono quentes + feixes espectrais) na nova
//!   linguagem, mantendo-a como tema padrão.
//! - Expõe o kit de componentes broadcast usado pelos painéis: botões com ícone vetorial,
//!   linhas de lista, Tally, medidores VU, cabeçalhos de seção/página e estados vazios.

use crate::native_ui::icons::{paint_icon, Icon};
use crate::protocol::{UiAccent, UiAppearanceSettings, UiThemePreset};
use egui::text::LayoutJob;
use egui::{
    pos2, vec2, Color32, Context, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense,
    Shadow, Shape, Stroke, StrokeKind, TextFormat, Ui, Vec2,
};
use egui_prismglass::{
    animate_hover, animate_pulse, paint_glass_surface, paint_prism_backdrop, prism_font,
    prism_numeric_font, AccentPalette, FontWeight, GlassElevation, GlassSurfaceTokens,
    PrismContextExt, PrismNeutrals, PrismTheme, SurfaceStyle, ThemeMode,
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

/// Cores de plataforma usadas em chips do Multichat e destinos.
pub const TWITCH: Color32 = Color32::from_rgb(0xA9, 0x70, 0xFF);
pub const YOUTUBE: Color32 = Color32::from_rgb(0xFF, 0x4E, 0x45);
pub const KICK: Color32 = Color32::from_rgb(0x53, 0xFC, 0x18);
pub const TIKTOK: Color32 = Color32::from_rgb(0x25, 0xF4, 0xEE);

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_srgba_unmultiplied();
    let [br, bg, bb, ba] = b.to_srgba_unmultiplied();
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(ar, br), l(ag, bg), l(ab, bb), l(aa, ba))
}

// ------------------------------------------------------------------ Construção de Temas

/// Superfície sólida (conteúdo do material Layered): preenchimento chapado + hairline.
fn solid_surface(fill: Color32, border: Color32, radius: f32) -> GlassSurfaceTokens {
    GlassSurfaceTokens {
        tint: fill,
        gradient_top: fill,
        gradient_bottom: fill,
        border,
        specular_top: Color32::TRANSPARENT,
        specular_accent: Color32::TRANSPARENT,
        inner_glow: Color32::TRANSPARENT,
        shadow: Shadow::NONE,
        contact_shadow: Shadow::NONE,
        corner_radius: radius,
        blur_radius: 0.0,
        border_width: 1.0,
        prism_intensity: 0.0,
        vibrancy: 1.0,
        refraction_strength: 0.0,
        chromatic_dispersion: 0.0,
        grain_intensity: 0.0,
        blur_tint_alpha: 1.0,
    }
}

/// Neutros carbono quentes do PrismCast (substituem a rampa fria do Prism Dark).
pub const CARBON_NEUTRALS: PrismNeutrals = PrismNeutrals {
    surface: Color32::from_rgb(20, 19, 17),
    raised: Color32::from_rgb(27, 26, 23),
    sunken: Color32::from_rgb(38, 36, 32),
    control_on: Color32::from_rgb(48, 46, 41),
    border: Color32::from_rgba_premultiplied(19, 18, 17, 19),
    border_strong: Color32::from_rgba_premultiplied(32, 31, 29, 32),
    hover: Color32::from_rgba_premultiplied(10, 10, 9, 10),
    selected: Color32::from_rgba_premultiplied(21, 20, 18, 21),
    muted: Color32::from_rgb(98, 94, 86),
};

/// **Prism Carbon**: a identidade PrismCast reconstruída na linguagem Prism (Layered) —
/// chassi carbono fosco quente, tipografia marfim, Aurora espectral (vermelho/âmbar/turquesa/azul)
/// visível apenas nas frestas e no vidro da barra lateral.
pub fn prism_carbon_theme() -> PrismTheme {
    let mut theme = PrismTheme::prism(ThemeMode::Dark, SurfaceStyle::Layered);
    theme.name = "Prism Carbon".to_owned();

    let p = &mut theme.palette;
    p.canvas = Color32::from_rgb(10, 10, 9);
    p.canvas_secondary = Color32::from_rgb(150, 36, 48);
    p.orb_primary = Color32::from_rgb(168, 100, 30);
    p.orb_secondary = Color32::from_rgb(14, 132, 112);
    p.orb_accent = Color32::from_rgb(30, 86, 170);
    p.text_primary = Color32::from_rgb(241, 237, 228);
    p.text_secondary = Color32::from_rgb(176, 169, 156);
    p.text_muted = Color32::from_rgb(132, 126, 115);
    p.success = Color32::from_rgb(72, 205, 130);
    p.warning = Color32::from_rgb(245, 178, 66);
    p.danger = Color32::from_rgb(244, 92, 76);

    let n = CARBON_NEUTRALS;
    theme.neutrals = n;
    let r = theme.radius;
    theme.surface_subtle = solid_surface(n.raised, n.border, r.lg);
    theme.surface_card = solid_surface(n.raised, n.border, r.lg);
    theme.surface_panel = solid_surface(n.surface, n.border, r.lg);

    // Camadas flutuantes em vidro fumê quente.
    theme.surface_sidebar.tint = rgba(24, 23, 20, 204);
    theme.surface_sidebar.gradient_top = rgba(24, 23, 20, 198);
    theme.surface_sidebar.gradient_bottom = rgba(24, 23, 20, 210);
    theme.surface_floating.tint = rgba(33, 32, 28, 232);
    theme.surface_floating.gradient_top = rgba(33, 32, 28, 226);
    theme.surface_floating.gradient_bottom = rgba(33, 32, 28, 238);
    theme
}

/// Aplica a cor de destaque escolhida pelo usuário.
///
/// `Signal` é o turquesa espectral da marca: vivo no escuro (texto escuro sobre o botão, via
/// `PrismTheme::on_accent`) e mais profundo no claro para manter contraste ≥ 4.5:1 com branco.
pub fn apply_accent(theme: &mut PrismTheme, accent: UiAccent) {
    let palette = match accent {
        UiAccent::Signal => None,
        UiAccent::Cobalt => Some(AccentPalette::Cobalt),
        UiAccent::Violet => Some(AccentPalette::Violet),
        UiAccent::Emerald => Some(AccentPalette::Emerald),
        UiAccent::Ruby => Some(AccentPalette::Ruby),
        UiAccent::Amber => Some(AccentPalette::Amber),
        UiAccent::Cyan => Some(AccentPalette::Cyan),
        UiAccent::Magenta => Some(AccentPalette::Magenta),
        UiAccent::Graphite => Some(AccentPalette::Graphite),
    };
    if let Some(palette) = palette {
        theme.set_accent_palette(palette);
        return;
    }
    // Signal: parte do Emerald (classificação de contraste mais próxima) e troca o tom.
    theme.set_accent_palette(AccentPalette::Emerald);
    let dark = theme.mode == ThemeMode::Dark;
    theme.palette.accent = if dark {
        SPEC_TEAL
    } else {
        Color32::from_rgb(10, 122, 108)
    };
    theme.palette.accent_warm = if dark {
        Color32::from_rgb(120, 236, 212)
    } else {
        Color32::from_rgb(20, 170, 150)
    };
}

/// Retorna a cor swatch exibida no seletor de destaque.
pub fn accent_swatch(accent: UiAccent) -> Color32 {
    match accent {
        UiAccent::Signal => SPEC_TEAL,
        UiAccent::Cobalt => AccentPalette::Cobalt.swatch(),
        UiAccent::Violet => AccentPalette::Violet.swatch(),
        UiAccent::Emerald => AccentPalette::Emerald.swatch(),
        UiAccent::Ruby => AccentPalette::Ruby.swatch(),
        UiAccent::Amber => AccentPalette::Amber.swatch(),
        UiAccent::Cyan => AccentPalette::Cyan.swatch(),
        UiAccent::Magenta => AccentPalette::Magenta.swatch(),
        UiAccent::Graphite => AccentPalette::Graphite.swatch(),
    }
}

/// Instancia o tema base de um preset (sem ajustes do usuário).
pub fn base_theme(preset: UiThemePreset) -> PrismTheme {
    match preset {
        UiThemePreset::PrismCarbon => prism_carbon_theme(),
        UiThemePreset::PrismDark => PrismTheme::prism_dark(),
        UiThemePreset::PrismLight => PrismTheme::prism_light(),
        UiThemePreset::PrismGlassDark => PrismTheme::prism_glass_dark(),
        UiThemePreset::PrismGlassLight => PrismTheme::prism_glass_light(),
        UiThemePreset::Onyx => PrismTheme::onyx(),
        UiThemePreset::Glacial => PrismTheme::glacial(),
        UiThemePreset::Frost => PrismTheme::frost(),
        UiThemePreset::MinimalDark => PrismTheme::minimal_dark(),
        UiThemePreset::MinimalLight => PrismTheme::minimal_light(),
    }
}

/// Constrói o `PrismTheme` completo a partir das preferências persistidas do usuário.
pub fn build_prism_theme(appearance: &UiAppearanceSettings) -> PrismTheme {
    let mut theme = base_theme(appearance.preset);
    apply_accent(&mut theme, appearance.accent);
    theme.glass_opacity_scale = appearance.glass_opacity.clamp(0.45, 1.55);
    theme.specular_scale = appearance.specular_intensity.clamp(0.0, 1.8);
    theme.radius_scale = appearance.radius_scale.clamp(0.5, 1.4);
    theme
}

/// Aplica o tema ao contexto `egui` (fontes Inter, visuals e espaçamentos do estúdio).
pub fn apply_prismcast_theme(ctx: &Context, appearance: &UiAppearanceSettings) -> PrismTheme {
    // Idempotente: registra Inter (3 pesos + algarismos tabulares) apenas na primeira chamada.
    egui_prismglass::install_fonts(ctx);

    let theme = build_prism_theme(appearance);
    ctx.set_prism_theme(theme.clone());

    let prism = theme.surface_style.is_prism();
    let n = theme.neutrals;
    ctx.all_styles_mut(|style| {
        // Os painéis pintam a própria superfície; o fundo da janela é o backdrop.
        style.visuals.panel_fill = Color32::TRANSPARENT;
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;
        style.interaction.tooltip_delay = 0.25;
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = vec2(8.0, 6.0);
        if prism {
            style.visuals.extreme_bg_color = match theme.mode {
                ThemeMode::Dark => n.sunken,
                ThemeMode::Light => n.raised,
            };
            style.visuals.text_edit_bg_color = Some(match theme.mode {
                ThemeMode::Dark => n.raised,
                ThemeMode::Light => Color32::WHITE,
            });
        } else {
            style.spacing.button_padding = vec2(11.0, 6.0);
        }
    });

    theme
}

// ------------------------------------------------------------------ Backdrop

/// Pinta o backdrop do tema (Aurora nos presets Prism) e a assinatura espectral de 2 px no topo.
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
    let mut intensity = appearance.backdrop_intensity.clamp(0.0, 1.4);
    // No material Layered o backdrop só aparece nas frestas e no vidro: luz mais contida.
    if theme.surface_style == SurfaceStyle::Layered {
        intensity *= 0.62;
    }
    paint_prism_backdrop(painter, rect, theme, anim_t, intensity);

    let bar_alpha = match theme.surface_style {
        SurfaceStyle::Minimal => 110,
        SurfaceStyle::Layered => 150,
        _ => 185,
    };
    let segments = [
        (0.0, 0.25, SPEC_RED),
        (0.25, 0.50, SPEC_AMBER),
        (0.50, 0.75, SPEC_TEAL),
        (0.75, 1.0, SPEC_BLUE),
    ];
    for (a, b, col) in segments {
        let seg_rect = Rect::from_min_max(
            pos2(rect.left() + rect.width() * a, rect.top()),
            pos2(rect.left() + rect.width() * b, rect.top() + 2.0),
        );
        painter.rect_filled(seg_rect, 0.0, with_alpha(col, bar_alpha));
    }
}

// ------------------------------------------------------------------ Tipografia

/// Inter no peso pedido (cai para a fonte proporcional se Inter não estiver instalada).
pub fn font(ui: &Ui, size: f32, weight: FontWeight) -> FontId {
    prism_font(ui.ctx(), size, weight)
}

/// Inter com algarismos tabulares — para FPS, dB, timers e contadores que não podem "dançar".
pub fn num_font(ui: &Ui, size: f32, weight: FontWeight) -> FontId {
    prism_numeric_font(ui.ctx(), size, weight)
}

/// Texto rico em Inter.
pub fn text(
    ui: &Ui,
    s: impl Into<String>,
    size: f32,
    weight: FontWeight,
    color: Color32,
) -> egui::RichText {
    egui::RichText::new(s)
        .font(font(ui, size, weight))
        .color(color)
}

/// Texto rico com algarismos tabulares.
pub fn num_text(
    ui: &Ui,
    s: impl Into<String>,
    size: f32,
    weight: FontWeight,
    color: Color32,
) -> egui::RichText {
    egui::RichText::new(s)
        .font(num_font(ui, size, weight))
        .color(color)
}

/// Rótulo em caixa-alta com espaçamento de letras (eyebrow), usado nos cabeçalhos de seção.
pub fn eyebrow_job(ui: &Ui, title: &str, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &title.to_uppercase(),
        0.0,
        TextFormat {
            font_id: font(ui, 10.5, FontWeight::Semibold),
            color,
            extra_letter_spacing: 0.7,
            ..Default::default()
        },
    );
    job
}

/// Cabeçalho de seção discreto (caixa-alta, Inter Semibold, tracking +0.7).
pub fn section_heading(ui: &mut Ui, title: &str, theme: &PrismTheme) {
    let job = eyebrow_job(ui, title, theme.palette.text_muted);
    ui.label(job);
}

/// Cabeçalho de página: título 20 pt + subtítulo, com área de ações à direita.
pub fn page_header(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    theme: &PrismTheme,
    actions: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(text(
                ui,
                title,
                21.0,
                FontWeight::Semibold,
                theme.palette.text_primary,
            ));
            ui.label(text(
                ui,
                subtitle,
                12.5,
                FontWeight::Regular,
                theme.palette.text_secondary,
            ));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
    });
    ui.add_space(14.0);
}

/// Pesos Inter para `RichText` (o `.strong()` do egui só muda a cor, não o peso).
pub trait RichTextWeight {
    /// Inter Semibold (600).
    fn semibold(self) -> Self;
    /// Inter Medium (500).
    fn medium(self) -> Self;
}

impl RichTextWeight for egui::RichText {
    fn semibold(self) -> Self {
        self.family(egui::FontFamily::Name("prism-600".into()))
    }
    fn medium(self) -> Self {
        self.family(egui::FontFamily::Name("prism-500".into()))
    }
}

// ------------------------------------------------------------------ Superfícies & Helpers de Cor

/// Overlay de seleção para linhas/cartões (adaptado a cada material).
pub fn selected_card_tint(theme: &PrismTheme) -> Color32 {
    let a = theme.palette.accent;
    match (theme.surface_style, theme.mode) {
        (SurfaceStyle::Minimal, ThemeMode::Light) => Color32::from_rgb(239, 245, 255),
        (SurfaceStyle::Minimal, ThemeMode::Dark) => Color32::from_rgb(31, 37, 48),
        (_, ThemeMode::Light) => with_alpha(a, 26),
        (_, ThemeMode::Dark) => with_alpha(a, 34),
    }
}

/// Fundo para superfícies de mídia (palco e miniaturas).
pub fn media_well_fill(theme: &PrismTheme) -> Color32 {
    match theme.mode {
        ThemeMode::Dark => Color32::from_rgb(0x08, 0x08, 0x08),
        ThemeMode::Light => Color32::from_rgb(0x16, 0x17, 0x1B),
    }
}

/// Pinta um container do tema via `egui-prismglass::paint_glass_surface`.
pub fn paint_elevated_surface(
    painter: &Painter,
    rect: Rect,
    theme: &PrismTheme,
    elev: GlassElevation,
) {
    let tokens = theme.surface(elev);
    paint_glass_surface(painter, rect, &tokens);
}

/// Cor de hover para linhas e botões fantasmas.
pub fn hover_fill(theme: &PrismTheme) -> Color32 {
    match (theme.surface_style.is_prism(), theme.mode) {
        (true, _) => theme.neutrals.hover,
        (false, ThemeMode::Dark) => Color32::from_rgba_unmultiplied(255, 255, 255, 14),
        (false, ThemeMode::Light) => Color32::from_rgba_unmultiplied(0, 0, 0, 10),
    }
}

/// Cor de linha/divisor hairline do tema.
pub fn hairline_color(theme: &PrismTheme) -> Color32 {
    if theme.surface_style.is_prism() {
        theme.neutrals.border
    } else {
        theme.surface_subtle.border
    }
}

/// Divisor horizontal hairline de largura total.
pub fn hairline(ui: &mut Ui, theme: &PrismTheme) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, hairline_color(theme)),
    );
}

/// Divisor vertical hairline (para clusters de botões na barra superior).
pub fn vsep(ui: &mut Ui, theme: &PrismTheme, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, height), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0, hairline_color(theme)),
    );
}

// ------------------------------------------------------------------ Logotipo Óptico

/// Logotipo vetorial nativo do PrismCast adaptado ao tema ativo.
pub fn paint_prism_logo(painter: &Painter, rect: Rect, theme: &PrismTheme) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let ink = theme.palette.text_primary;

    painter.line_segment(
        [
            pos2(rect.left() + s * 0.04, c.y + s * 0.08),
            pos2(c.x - s * 0.14, c.y - s * 0.01),
        ],
        Stroke::new(1.8_f32, ink),
    );
    let rays = [
        (-0.18, SPEC_RED),
        (-0.05, SPEC_AMBER),
        (0.08, SPEC_TEAL),
        (0.21, SPEC_BLUE),
    ];
    for (dy, col) in rays {
        painter.line_segment(
            [
                pos2(c.x + s * 0.12, c.y - s * 0.02 + dy * s * 0.25),
                pos2(rect.right() - s * 0.03, c.y + dy * s),
            ],
            Stroke::new(1.6_f32, col),
        );
    }
    let glass_fill = match theme.mode {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(241, 237, 228, 38),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(18, 19, 23, 22),
    };
    let pts = vec![
        pos2(c.x - s * 0.02, rect.top() + s * 0.12),
        pos2(c.x - s * 0.30, rect.bottom() - s * 0.14),
        pos2(c.x + s * 0.26, rect.bottom() - s * 0.14),
    ];
    painter.add(Shape::convex_polygon(
        pts,
        glass_fill,
        Stroke::new(1.5_f32, ink),
    ));
}

// ------------------------------------------------------------------ Botões

/// Estilo visual dos botões do estúdio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    /// Preenchimento sólido na cor de destaque.
    Accent,
    /// Vermelho Tally de broadcast (Entrar ao vivo, Transição, Encerrar).
    Tally,
    /// Controle elevado neutro.
    Secondary,
    /// Sem fundo até o hover.
    Ghost,
    /// Ação destrutiva (contorno vermelho, preenche no hover).
    Danger,
}

/// Opções de um botão do estúdio.
#[derive(Debug, Clone, Copy)]
pub struct BtnOpts {
    pub style: Btn,
    pub icon: Option<Icon>,
    pub selected: bool,
    pub enabled: bool,
    pub height: f32,
    pub min_width: f32,
    pub pill: bool,
    pub font_size: f32,
}

impl BtnOpts {
    pub fn new(style: Btn) -> Self {
        Self {
            style,
            icon: None,
            selected: false,
            enabled: true,
            height: 30.0,
            min_width: 0.0,
            pill: false,
            font_size: 12.5,
        }
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn pill(mut self, pill: bool) -> Self {
        self.pill = pill;
        self
    }
    pub fn small(mut self) -> Self {
        self.height = 26.0;
        self.font_size = 12.0;
        self
    }
}

/// Resolve (preenchimento, borda, cor do texto) para um estilo/estado.
fn button_colors(
    theme: &PrismTheme,
    o: &BtnOpts,
    hovered: bool,
    pressed: bool,
) -> (Color32, Color32, Color32) {
    let p = &theme.palette;
    let dark = theme.mode == ThemeMode::Dark;
    let n = theme.neutrals;
    let frosted = matches!(
        theme.surface_style,
        SurfaceStyle::Frosted | SurfaceStyle::Glass
    );
    let shade = |c: Color32| {
        if pressed {
            mix(c, Color32::BLACK, 0.14)
        } else if hovered {
            mix(c, Color32::WHITE, 0.09)
        } else {
            c
        }
    };
    let on = |fill: Color32| {
        let [r, g, b, _] = fill.to_srgba_unmultiplied();
        let luma = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
        if luma > 160.0 {
            Color32::from_rgb(14, 15, 16)
        } else {
            Color32::WHITE
        }
    };
    match o.style {
        Btn::Accent => {
            let f = shade(p.accent);
            (f, Color32::TRANSPARENT, on(f))
        }
        Btn::Tally => {
            let f = if pressed {
                TALLY_DARK
            } else if hovered {
                TALLY_HOVER
            } else {
                TALLY
            };
            (f, Color32::TRANSPARENT, Color32::WHITE)
        }
        Btn::Danger => {
            if hovered || pressed {
                (shade(p.danger), Color32::TRANSPARENT, Color32::WHITE)
            } else {
                (Color32::TRANSPARENT, with_alpha(p.danger, 150), p.danger)
            }
        }
        Btn::Secondary | Btn::Ghost => {
            if o.selected {
                let base = if dark { 40 } else { 30 };
                let a = if hovered { base + 12 } else { base };
                return (
                    with_alpha(p.accent, a),
                    with_alpha(p.accent, 120),
                    if dark {
                        mix(p.accent, Color32::WHITE, 0.35)
                    } else {
                        mix(p.accent, Color32::BLACK, 0.18)
                    },
                );
            }
            let ghost = o.style == Btn::Ghost;
            let (fill, border) = if frosted {
                if dark {
                    (
                        rgba(255, 255, 255, if ghost { 0 } else { 16 }),
                        rgba(255, 255, 255, if ghost { 0 } else { 30 }),
                    )
                } else {
                    (
                        rgba(255, 255, 255, if ghost { 0 } else { 150 }),
                        rgba(0, 0, 0, if ghost { 0 } else { 24 }),
                    )
                }
            } else if ghost {
                (Color32::TRANSPARENT, Color32::TRANSPARENT)
            } else if theme.surface_style.is_prism() {
                (n.raised, n.border_strong)
            } else {
                (theme.surface_subtle.tint, theme.surface_subtle.border)
            };
            let fill = if pressed {
                mix(
                    fill,
                    if dark {
                        Color32::BLACK
                    } else {
                        Color32::from_gray(200)
                    },
                    0.25,
                )
                .gamma_multiply(1.0)
            } else if hovered {
                let h = hover_fill(theme);
                if fill.a() == 0 {
                    h
                } else {
                    mix(
                        fill,
                        if dark { Color32::WHITE } else { Color32::BLACK },
                        0.05,
                    )
                }
            } else {
                fill
            };
            let txt = if ghost && !hovered {
                p.text_secondary
            } else {
                p.text_primary
            };
            (fill, border, txt)
        }
    }
}

/// Botão do estúdio com ícone vetorial opcional, Inter Medium e estados hover/pressed/selected.
pub fn button(ui: &mut Ui, label: &str, o: BtnOpts) -> Response {
    let theme = ui.ctx().prism_theme();
    let font_id = font(ui, o.font_size, FontWeight::Medium);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_id, theme.palette.text_primary);
    let icon_sz = (o.height * 0.5).clamp(12.0, 16.0);
    let has_text = !label.is_empty();
    let pad_x = if has_text { 12.0 } else { 0.0 };
    let gap = if o.icon.is_some() && has_text {
        7.0
    } else {
        0.0
    };
    let content_w = galley.size().x + o.icon.map_or(0.0, |_| icon_sz) + gap;
    let w = if has_text {
        (content_w + pad_x * 2.0).max(o.min_width)
    } else {
        o.height.max(o.min_width)
    };
    let sense = if o.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(w, o.height), sense);
    if o.enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = o.enabled && response.hovered();
    let pressed = o.enabled && response.is_pointer_button_down_on();
    let (fill, border, txt) = button_colors(&theme, &o, hovered, pressed);
    let alpha = if o.enabled { 1.0 } else { 0.4 };
    let radius = if o.pill {
        o.height * 0.5
    } else {
        (theme.radius.md * theme.radius_scale.clamp(0.4, 1.6)).min(o.height * 0.5)
    };
    let painter = ui.painter();
    let solid = matches!(o.style, Btn::Accent | Btn::Tally)
        || (o.style == Btn::Danger && (hovered || pressed));
    if solid && !pressed {
        painter.add(
            Shadow {
                offset: [0, 1],
                blur: 3,
                spread: 0,
                color: Color32::from_black_alpha(if theme.mode == ThemeMode::Dark {
                    80
                } else {
                    36
                }),
            }
            .as_shape(rect, CornerRadius::same(radius.round() as u8)),
        );
    }
    let cr = CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8);
    if fill.a() > 0 {
        painter.rect_filled(rect, cr, fill.gamma_multiply(alpha));
    }
    if border.a() > 0 {
        painter.rect_stroke(
            rect,
            cr,
            Stroke::new(1.0, border.gamma_multiply(alpha)),
            StrokeKind::Inside,
        );
    }
    if solid {
        let y = rect.top() + 1.0;
        painter.hline(
            (rect.left() + radius)..=(rect.right() - radius),
            y,
            Stroke::new(1.0, Color32::from_white_alpha((40.0 * alpha) as u8)),
        );
    }
    let txt = txt.gamma_multiply(alpha);
    let dy = if pressed { 0.5 } else { 0.0 };
    let start_x = if has_text {
        rect.center().x - content_w * 0.5
    } else {
        rect.center().x - icon_sz * 0.5
    };
    if let Some(icon) = o.icon {
        let ir = Rect::from_min_size(
            pos2(start_x, rect.center().y - icon_sz * 0.5 + dy),
            Vec2::splat(icon_sz),
        );
        paint_icon(painter, ir, icon, txt);
    }
    if has_text {
        let tx = start_x + o.icon.map_or(0.0, |_| icon_sz + gap);
        painter.galley_with_override_text_color(
            pos2(tx, rect.center().y - galley.size().y * 0.5 + dy),
            galley,
            txt,
        );
    }
    response
}

/// Botão quadrado só com ícone (ações de linha: olho, cadeado, lixeira, setas…).
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    icon_button_ex(ui, icon, tooltip, false, 24.0)
}

/// Botão de ícone com estado ativo e tamanho customizado.
pub fn icon_button_ex(ui: &mut Ui, icon: Icon, tooltip: &str, active: bool, size: f32) -> Response {
    let theme = ui.ctx().prism_theme();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if ui.is_rect_visible(rect) {
        let t = animate_hover(ui.ctx(), resp.id, resp.hovered());
        let cr = CornerRadius::same((theme.radius.sm * theme.radius_scale).clamp(3.0, 10.0) as u8);
        if active {
            ui.painter()
                .rect_filled(rect, cr, with_alpha(theme.palette.accent, 38));
        } else if t > 0.01 {
            let h = hover_fill(&theme);
            ui.painter()
                .rect_filled(rect, cr, h.gamma_multiply(t.clamp(0.0, 1.0) * 1.6));
        }
        let col = if active {
            theme.palette.accent
        } else {
            mix(theme.palette.text_muted, theme.palette.text_primary, t)
        };
        let inner = (size * 0.56).round();
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(inner)),
            icon,
            col,
        );
    }
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

/// Botão primário Tally (compatibilidade com chamadas existentes).
pub fn tally_button(ui: &mut Ui, label: &str) -> Response {
    button(ui, label, BtnOpts::new(Btn::Tally))
}

/// Botão Tally configurável.
pub fn tally_button_ex(ui: &mut Ui, label: &str, pill: bool, enabled: bool) -> Response {
    button(
        ui,
        label,
        BtnOpts::new(Btn::Tally).pill(pill).enabled(enabled),
    )
}

/// Botão Tally compacto para cabeçalhos densos.
pub fn tally_button_compact(ui: &mut Ui, label: &str) -> Response {
    button(ui, label, BtnOpts::new(Btn::Tally).small())
}

/// Botão de alternância compacto (estado selecionado em destaque).
pub fn glass_toggle_button(ui: &mut Ui, label: &str, active: bool) -> Response {
    button(
        ui,
        label,
        BtnOpts::new(Btn::Secondary).selected(active).small(),
    )
}

// ------------------------------------------------------------------ Linhas de lista & chips

/// Linha de lista clicável de largura total (cenas, fontes, canais).
///
/// Pinta hover/seleção com os neutros do tema e um marcador de 3 px na cor `marker` quando
/// selecionada. Widgets adicionados dentro de `add_contents` ficam por cima e mantêm a própria
/// interação (botões de ícone, por exemplo).
pub fn list_row(
    ui: &mut Ui,
    selected: bool,
    height: f32,
    marker: Option<Color32>,
    add_contents: impl FnOnce(&mut Ui),
) -> Response {
    let theme = ui.ctx().prism_theme();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if ui.is_rect_visible(rect) {
        let cr = CornerRadius::same((theme.radius.md * theme.radius_scale).clamp(4.0, 12.0) as u8);
        if selected {
            ui.painter()
                .rect_filled(rect, cr, selected_card_tint(&theme));
            if let Some(m) = marker {
                let bar = Rect::from_min_size(
                    pos2(rect.left() + 1.0, rect.top() + 7.0),
                    vec2(3.0, rect.height() - 14.0),
                );
                ui.painter().rect_filled(bar, 2.0, m);
            }
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, cr, hover_fill(&theme));
        }
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(10.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    add_contents(&mut child);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Pastilha compacta com texto em caixa-alta (tipo de fonte, plataforma, resolução).
pub fn chip(ui: &mut Ui, label: &str, color: Color32) -> Response {
    let theme = ui.ctx().prism_theme();
    let f = font(ui, 9.5, FontWeight::Semibold);
    let galley = ui.painter().layout_no_wrap(label.to_uppercase(), f, color);
    let size = vec2(galley.size().x + 10.0, 17.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let a = if theme.mode == ThemeMode::Dark {
            30
        } else {
            24
        };
        ui.painter().rect_filled(rect, 4.0, with_alpha(color, a));
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, color);
    }
    resp
}

/// Ponto de status com halo (verde OK, âmbar alerta, accent neutro).
pub fn status_dot(ui: &mut Ui, color: Color32, pulse: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    let t = if pulse {
        animate_pulse(ui.ctx(), 0.8)
    } else {
        0.0
    };
    ui.painter().circle_filled(
        rect.center(),
        4.2 + t * 1.6,
        with_alpha(color, (52.0 + t * 30.0) as u8),
    );
    ui.painter().circle_filled(rect.center(), 2.6, color);
}

/// Ícone avulso alinhado ao texto.
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui.painter(), rect, icon, color);
    resp
}

/// Tecla de atalho (`Ctrl`, `Z`, `Del`) desenhada como uma pequena capa de teclado.
pub fn kbd(ui: &mut Ui, key: &str) {
    let theme = ui.ctx().prism_theme();
    let f = font(ui, 10.0, FontWeight::Medium);
    let galley = ui
        .painter()
        .layout_no_wrap(key.to_owned(), f, theme.palette.text_secondary);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 10.0, 17.0), Sense::hover());
    let border = hairline_color(&theme);
    ui.painter().rect_filled(rect, 4.0, hover_fill(&theme));
    ui.painter()
        .rect_stroke(rect, 4.0, Stroke::new(1.0, border), StrokeKind::Inside);
    ui.painter().galley(
        rect.center() - galley.size() * 0.5,
        galley,
        theme.palette.text_secondary,
    );
}

/// Estado vazio centralizado: ícone em círculo, título e descrição.
pub fn empty_state(ui: &mut Ui, ic: Icon, title: &str, body: &str) {
    let theme = ui.ctx().prism_theme();
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 19.0, hover_fill(&theme).gamma_multiply(2.2));
        paint_icon(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(18.0)),
            ic,
            theme.palette.text_muted,
        );
        ui.add_space(8.0);
        ui.label(text(
            ui,
            title,
            13.0,
            FontWeight::Semibold,
            theme.palette.text_primary,
        ));
        ui.add_space(2.0);
        ui.label(text(
            ui,
            body,
            11.5,
            FontWeight::Regular,
            theme.palette.text_muted,
        ));
        ui.add_space(8.0);
    });
}

/// Cabeçalho de dock: eyebrow + contador opcional + ações alinhadas à direita.
pub fn dock_header(
    ui: &mut Ui,
    title: &str,
    count: Option<usize>,
    theme: &PrismTheme,
    actions: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.set_min_height(26.0);
        section_heading(ui, title, theme);
        if let Some(c) = count {
            ui.label(num_text(
                ui,
                c.to_string(),
                10.5,
                FontWeight::Medium,
                theme.palette.text_muted,
            ));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            actions(ui);
        });
    });
    ui.add_space(4.0);
}

// ------------------------------------------------------------------ Componentes Broadcast

/// Pílula Tally com pulso e cronômetro (`AO VIVO 01:23:45`).
pub fn paint_tally_pill(ui: &mut Ui, live: bool, recording: bool, elapsed_sec: Option<u64>) {
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
    let timer = elapsed_sec.filter(|_| live || recording).map(format_hms);

    let label_font = font(ui, 11.0, FontWeight::Semibold);
    let timer_font = num_font(ui, 11.0, FontWeight::Medium);
    let lg = ui
        .painter()
        .layout_no_wrap(label.to_owned(), label_font, Color32::WHITE);
    let tg = timer.as_ref().map(|t| {
        ui.painter()
            .layout_no_wrap(t.clone(), timer_font, Color32::WHITE)
    });
    let w = 30.0 + lg.size().x + tg.as_ref().map_or(0.0, |g| g.size().x + 10.0) + 12.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w.max(96.0), 28.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    let pulse = if live || recording {
        animate_pulse(ui.ctx(), if live { 0.85 } else { 0.55 })
    } else {
        0.0
    };
    let (bg, border, dot, txt) = if live {
        (
            with_alpha(TALLY, (40.0 + pulse * 30.0) as u8),
            with_alpha(TALLY, 200),
            SPEC_RED,
            if theme.mode == ThemeMode::Light {
                TALLY_DARK
            } else {
                Color32::WHITE
            },
        )
    } else if recording {
        (
            with_alpha(METER_WARN, (30.0 + pulse * 24.0) as u8),
            with_alpha(METER_WARN, 180),
            SPEC_AMBER,
            theme.palette.text_primary,
        )
    } else {
        (
            hover_fill(&theme),
            hairline_color(&theme),
            theme.palette.text_muted,
            theme.palette.text_secondary,
        )
    };
    let cr = CornerRadius::same(14);
    p.rect_filled(rect, cr, bg);
    p.rect_stroke(rect, cr, Stroke::new(1.0, border), StrokeKind::Inside);
    let dc = pos2(rect.left() + 14.0, rect.center().y);
    if live || recording {
        p.circle_filled(
            dc,
            5.0 + pulse * 2.2,
            with_alpha(dot, (55.0 + pulse * 45.0) as u8),
        );
    }
    p.circle_filled(dc, 3.4, dot);
    let x = rect.left() + 25.0;
    let lw = lg.size().x;
    p.galley_with_override_text_color(pos2(x, rect.center().y - lg.size().y * 0.5), lg, txt);
    if let Some(g) = tg {
        let tx = x + lw + 10.0;
        p.vline(
            tx - 5.0,
            (rect.top() + 8.0)..=(rect.bottom() - 8.0),
            Stroke::new(1.0, with_alpha(txt, 70)),
        );
        p.galley_with_override_text_color(
            pos2(tx, rect.center().y - g.size().y * 0.5),
            g,
            txt.gamma_multiply(0.9),
        );
    }
}

/// Formata segundos como `HH:MM:SS` (ou `MM:SS` abaixo de 1 h).
pub fn format_hms(total: u64) -> String {
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Converte dBFS (−60..0) em fração 0..1 para medidores.
pub fn db_to_frac(db: f32) -> f32 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Medidor VU broadcast segmentado (verde → âmbar → vermelho) com marcas de −18 e −6 dBFS.
pub fn paint_vu_meter(ui: &mut Ui, peak_db: f32, muted: bool, width: f32) {
    let theme = ui.ctx().prism_theme();
    let height = 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width.max(48.0), height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter();
    let trough = match theme.mode {
        ThemeMode::Dark => Color32::from_rgba_unmultiplied(0, 0, 0, 110),
        ThemeMode::Light => Color32::from_rgba_unmultiplied(18, 19, 23, 22),
    };
    p.rect_filled(rect, 3.0, trough);
    let norm = db_to_frac(peak_db);
    let segments = 40usize;
    let gap = 1.0;
    let seg_w = (rect.width() - gap * (segments as f32 - 1.0)) / segments as f32;
    let lit = (norm * segments as f32).round() as usize;
    for i in 0..segments {
        let x = rect.left() + i as f32 * (seg_w + gap);
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(seg_w.max(1.0), rect.height()));
        let frac = i as f32 / segments as f32;
        let base = if muted {
            theme.palette.text_muted
        } else if frac >= 0.9 {
            METER_CLIP
        } else if frac >= 0.7 {
            METER_WARN
        } else {
            METER_OK
        };
        let col = if i < lit {
            base
        } else {
            with_alpha(
                base,
                if theme.mode == ThemeMode::Light {
                    34
                } else {
                    22
                },
            )
        };
        p.rect_filled(r, 1.0, col);
    }
    // Marcas de referência −18 dBFS (nível de diálogo) e −6 dBFS (headroom).
    for db in [-18.0, -6.0] {
        let x = rect.left() + rect.width() * db_to_frac(db);
        p.vline(
            x,
            (rect.top() - 2.0)..=(rect.bottom() + 2.0),
            Stroke::new(1.0, with_alpha(theme.palette.text_muted, 90)),
        );
    }
}

/// Item de telemetria da barra de status: rótulo discreto + valor tabular.
pub fn stat(ui: &mut Ui, label: &str, value: &str, value_color: Color32) {
    let theme = ui.ctx().prism_theme();
    ui.label(text(
        ui,
        label,
        11.0,
        FontWeight::Regular,
        theme.palette.text_muted,
    ));
    ui.label(num_text(ui, value, 11.0, FontWeight::Medium, value_color));
}

/// Pinta o rótulo flutuante de um viewport (`PROGRAMA`, `PRÉVIA`, `9:16`).
pub fn paint_viewport_tag(
    painter: &Painter,
    ui: &Ui,
    anchor: Pos2,
    label: &str,
    accent: Color32,
) -> Rect {
    let f = font(ui, 10.0, FontWeight::Semibold);
    let galley = painter.layout_no_wrap(label.to_owned(), f, Color32::WHITE);
    let rect = Rect::from_min_size(anchor, vec2(galley.size().x + 24.0, 20.0));
    painter.rect_filled(rect, 10.0, Color32::from_black_alpha(150));
    painter.rect_stroke(
        rect,
        10.0,
        Stroke::new(1.0, Color32::from_white_alpha(22)),
        StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 10.0, rect.center().y), 3.0, accent);
    painter.galley(
        pos2(rect.left() + 17.0, rect.center().y - galley.size().y * 0.5),
        galley,
        Color32::WHITE,
    );
    rect
}

/// Preenche `rect` com a pré-visualização de um tema (usado nos cartões de Aparência).
pub fn paint_theme_preview(painter: &Painter, rect: Rect, theme: &PrismTheme) {
    painter.rect_filled(rect, 8.0, theme.palette.canvas);
    let clip = painter.with_clip_rect(rect);
    paint_prism_backdrop(&clip, rect, theme, 0.0, 0.7);
    let n = theme.neutrals;
    let side = Rect::from_min_size(
        rect.min + vec2(5.0, 5.0),
        vec2(rect.width() * 0.22, rect.height() - 10.0),
    );
    paint_glass_surface(
        &clip,
        side,
        &theme.sidebar_surface().with_corner_radius(5.0),
    );
    let main = Rect::from_min_max(
        pos2(side.right() + 5.0, rect.top() + 5.0),
        rect.max - vec2(5.0, 5.0),
    );
    paint_glass_surface(&clip, main, &theme.panel_surface().with_corner_radius(5.0));
    let bar = Rect::from_min_size(main.min + vec2(7.0, 7.0), vec2(main.width() * 0.45, 5.0));
    clip.rect_filled(bar, 2.0, theme.palette.text_primary.gamma_multiply(0.7));
    let bar2 = Rect::from_min_size(
        bar.left_bottom() + vec2(0.0, 5.0),
        vec2(main.width() * 0.3, 4.0),
    );
    clip.rect_filled(bar2, 2.0, theme.palette.text_muted.gamma_multiply(0.7));
    let btn = Rect::from_min_size(
        pos2(main.right() - 30.0, main.bottom() - 16.0),
        vec2(23.0, 9.0),
    );
    clip.rect_filled(btn, 4.5, theme.palette.accent);
    let card = Rect::from_min_size(
        pos2(main.left() + 7.0, bar2.bottom() + 7.0),
        vec2(
            main.width() * 0.5,
            (main.bottom() - bar2.bottom() - 14.0).max(6.0),
        ),
    );
    let fill = if theme.surface_style.is_prism() {
        n.raised
    } else {
        theme.surface_subtle.tint
    };
    clip.rect_filled(card, 4.0, fill);
    for i in 0..3 {
        let y = side.top() + 10.0 + i as f32 * 9.0;
        let r = Rect::from_min_size(pos2(side.left() + 5.0, y), vec2(side.width() - 10.0, 4.0));
        clip.rect_filled(
            r,
            2.0,
            theme
                .palette
                .text_muted
                .gamma_multiply(if i == 0 { 0.9 } else { 0.45 }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_presets_build_expected_materials() {
        for preset in UiThemePreset::ALL {
            let appearance = UiAppearanceSettings {
                preset,
                ..Default::default()
            };
            let theme = build_prism_theme(&appearance);
            assert!(!theme.name.is_empty());
            assert_eq!(
                theme.mode == ThemeMode::Dark,
                preset.is_dark(),
                "{preset:?}"
            );
            assert_eq!(
                theme.surface_style.is_prism(),
                preset.is_prism(),
                "{preset:?}"
            );
            match preset {
                UiThemePreset::PrismGlassDark | UiThemePreset::PrismGlassLight => {
                    assert_eq!(theme.surface_style, SurfaceStyle::Frosted);
                }
                UiThemePreset::PrismCarbon
                | UiThemePreset::PrismDark
                | UiThemePreset::PrismLight => {
                    assert_eq!(theme.surface_style, SurfaceStyle::Layered);
                }
                UiThemePreset::MinimalDark | UiThemePreset::MinimalLight => {
                    assert_eq!(theme.surface_style, SurfaceStyle::Minimal);
                }
                _ => assert_eq!(theme.surface_style, SurfaceStyle::Glass),
            }
        }
    }

    #[test]
    fn carbon_keeps_content_solid_and_sidebar_glass() {
        let t = prism_carbon_theme();
        assert_eq!(t.panel_surface().tint.a(), 255);
        assert_eq!(t.surface(GlassElevation::Elevated).blur_radius, 0.0);
        assert!(t.sidebar_surface().blur_radius > 0.0);
        assert_eq!(t.neutrals, CARBON_NEUTRALS);
    }

    #[test]
    fn accents_apply_to_every_preset() {
        for preset in UiThemePreset::ALL {
            for accent in UiAccent::ALL {
                let t = build_prism_theme(&UiAppearanceSettings {
                    preset,
                    accent,
                    ..Default::default()
                });
                if accent == UiAccent::Signal {
                    let expected = if preset.is_dark() {
                        SPEC_TEAL
                    } else {
                        Color32::from_rgb(10, 122, 108)
                    };
                    assert_eq!(t.palette.accent, expected);
                }
                assert!(t.palette.accent.a() == 255);
            }
        }
    }

    #[test]
    fn apply_prismcast_theme_installs_theme_and_fonts() {
        let ctx = Context::default();
        let appearance = UiAppearanceSettings {
            preset: UiThemePreset::PrismCarbon,
            glass_opacity: 1.2,
            specular_intensity: 1.35,
            radius_scale: 1.1,
            ..Default::default()
        };
        let applied = apply_prismcast_theme(&ctx, &appearance);
        let read_back = ctx.prism_theme();
        assert_eq!(applied.name, "Prism Carbon");
        assert_eq!(read_back.name, "Prism Carbon");
        assert!(egui_prismglass::fonts_installed(&ctx));
        assert!((read_back.radius_scale - 1.1).abs() < 1e-4);
    }

    #[test]
    fn hms_formatting() {
        assert_eq!(format_hms(0), "00:00");
        assert_eq!(format_hms(65), "01:05");
        assert_eq!(format_hms(3_725), "01:02:05");
    }
}

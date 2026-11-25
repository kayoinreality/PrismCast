//! Seletor nativo de telas e janelas com miniaturas reais ao vivo e superfícies `egui-prismglass`.
//!
//! - Enumera monitores e janelas diretamente pela API nativa do SO em thread dedicada.
//! - Renderiza previews reais (`egui::TextureHandle`) em grade glassmorphism 3 colunas
//!   com elevação 3D, micro-lift com física de mola (`animate_hover`) e badges de resolução.
//! - Selecionar uma janela inicia a captura nativa em Rust imediatamente sem popup do SO.

use crate::capture::{self, NativeSourceEntry};
use crate::native_ui::theme;
use crate::protocol::SourceKind;
use egui::{
    Align2, Color32, ColorImage, Context, CornerRadius, FontId, Frame, Margin, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
};
use egui_prismglass::{
    BadgeVariant, GlassBadge, GlassButton, GlassElevation, PrismContextExt, ThemeMode,
    animate_hover, paint_glass_surface,
};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

#[derive(Debug, Clone)]
pub enum PickerPurpose {
    AddNewSource,
    RebindItem { item_id: String },
}

#[derive(Debug, Clone)]
pub struct PickerSelection {
    pub purpose: PickerPurpose,
    pub source_id: String,
    pub source_name: String,
    pub kind: SourceKind,
}

pub struct SourcePickerModal {
    pub open: bool,
    pub purpose: PickerPurpose,
    pub active_tab: SourceKind,
    pub search: String,
    pub selected_id: Option<String>,
    pub entries: Vec<NativeSourceEntry>,
    pub loading: bool,
    pending_rx: Option<Receiver<Vec<NativeSourceEntry>>>,
    thumb_textures: HashMap<String, TextureHandle>,
}

impl Default for SourcePickerModal {
    fn default() -> Self {
        Self {
            open: false,
            purpose: PickerPurpose::AddNewSource,
            active_tab: SourceKind::Window,
            search: String::new(),
            selected_id: None,
            entries: Vec::new(),
            loading: false,
            pending_rx: None,
            thumb_textures: HashMap::new(),
        }
    }
}

impl SourcePickerModal {
    pub fn open(
        &mut self,
        purpose: PickerPurpose,
        initial_tab: SourceKind,
        current_id: Option<String>,
    ) {
        self.open = true;
        self.purpose = purpose;
        self.active_tab = initial_tab;
        self.search.clear();
        self.selected_id = current_id;
        self.refresh();
    }

    pub fn refresh(&mut self) {
        self.loading = true;
        let (tx, rx) = mpsc::channel();
        self.pending_rx = Some(rx);
        std::thread::Builder::new()
            .name("prismcast-source-enum".to_string())
            .spawn(move || {
                let list = capture::list_native_sources_with_thumbs();
                let _ = tx.send(list);
            })
            .ok();
    }

    fn poll_results(&mut self, ctx: &Context) {
        if let Some(ref rx) = self.pending_rx {
            if let Ok(list) = rx.try_recv() {
                self.loading = false;
                self.pending_rx = None;
                self.thumb_textures.clear();

                for entry in &list {
                    if let Some(ref th) = entry.thumb {
                        if th.width > 0
                            && th.height > 0
                            && th.rgba.len() == (th.width as usize) * (th.height as usize) * 4
                        {
                            let img = ColorImage::from_rgba_unmultiplied(
                                [th.width as usize, th.height as usize],
                                &th.rgba,
                            );
                            let tex = ctx.load_texture(
                                format!("thumb::{}", entry.source.id),
                                img,
                                TextureOptions::LINEAR,
                            );
                            self.thumb_textures.insert(entry.source.id.clone(), tex);
                        }
                    }
                }

                if self.selected_id.is_none() {
                    if let Some(first) = list.iter().find(|e| e.source.kind == self.active_tab) {
                        self.selected_id = Some(first.source.id.clone());
                    }
                }
                self.entries = list;
                ctx.request_repaint();
            }
        }
    }

    /// Renderiza o modal flutuante `egui-prismglass` do seletor e retorna `Some(PickerSelection)` ao confirmar.
    pub fn show(&mut self, ctx: &Context) -> Option<PickerSelection> {
        if !self.open {
            return None;
        }
        self.poll_results(ctx);

        let prism = ctx.prism_theme();
        let mut result: Option<PickerSelection> = None;
        let mut close_now = false;

        // Scrim translúcido atrás do modal
        let screen_rect = ctx.content_rect();
        egui::Area::new(egui::Id::unique("source_picker_backdrop"))
            .fixed_pos(screen_rect.min)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                let scrim = match prism.mode {
                    ThemeMode::Dark => Color32::from_black_alpha(150),
                    ThemeMode::Light => Color32::from_rgba_unmultiplied(15, 18, 26, 86),
                };
                ui.painter().rect_filled(screen_rect, 0.0, scrim);
                if resp.clicked() {
                    close_now = true;
                }
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_now = true;
        }

        let modal_w = (screen_rect.width() - 80.0).clamp(580.0, 940.0);
        let modal_h = (screen_rect.height() - 90.0).clamp(430.0, 680.0);

        egui::Window::new("source_picker_window")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(modal_w, modal_h))
            .order(egui::Order::Foreground)
            .frame(Frame::NONE.inner_margin(Margin::same(20)))
            .show(ctx, |ui| {
                // Superfície 3D floating glass (`egui-prismglass`) no fundo do modal
                let outer_rect = ui.max_rect().expand(20.0);
                paint_glass_surface(
                    ui.painter(),
                    outer_rect,
                    &prism.surface(GlassElevation::Floating),
                );

                // Cabeçalho do modal
                ui.horizontal(|ui| {
                    let title = if self.active_tab == SourceKind::Screen {
                        "Escolha a tela"
                    } else {
                        "Escolha a janela"
                    };
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(title)
                                    .size(19.0)
                                    .strong()
                                    .color(prism.palette.text_primary),
                            );
                            ui.add(
                                GlassBadge::new("Captura Nativa Rust")
                                    .variant(BadgeVariant::Accent)
                                    .dot(true),
                            );
                        });
                        ui.label(
                            RichText::new(
                                "Sem popup de navegador, sem barra de compartilhamento e sem tirar foco do estúdio.",
                            )
                            .size(12.0)
                            .color(prism.palette.text_secondary),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                GlassButton::ghost("✕")
                                    .pill(true)
                                    .min_size(Vec2::new(32.0, 32.0)),
                            )
                            .clicked()
                        {
                            close_now = true;
                        }
                    });
                });

                ui.add_space(12.0);

                // Barra de filtro: abas Telas / Janelas + busca + atualizar
                ui.horizontal(|ui| {
                    let screen_count = self
                        .entries
                        .iter()
                        .filter(|e| e.source.kind == SourceKind::Screen)
                        .count();
                    let window_count = self
                        .entries
                        .iter()
                        .filter(|e| e.source.kind == SourceKind::Window)
                        .count();

                    if ui
                        .add(
                            GlassButton::secondary(format!("Telas ({screen_count})"))
                                .selected(self.active_tab == SourceKind::Screen)
                                .pill(true)
                                .min_size(Vec2::new(0.0, 30.0)),
                        )
                        .clicked()
                    {
                        self.active_tab = SourceKind::Screen;
                    }
                    if ui
                        .add(
                            GlassButton::secondary(format!("Janelas ({window_count})"))
                                .selected(self.active_tab == SourceKind::Window)
                                .pill(true)
                                .min_size(Vec2::new(0.0, 30.0)),
                        )
                        .clicked()
                    {
                        self.active_tab = SourceKind::Window;
                    }

                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .desired_width(230.0)
                            .hint_text("Filtrar por nome…"),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let lbl = if self.loading {
                            "Atualizando…"
                        } else {
                            "↻ Atualizar prévias"
                        };
                        if ui
                            .add(
                                GlassButton::secondary(lbl)
                                    .pill(true)
                                    .enabled(!self.loading)
                                    .min_size(Vec2::new(0.0, 30.0)),
                            )
                            .clicked()
                        {
                            self.refresh();
                        }
                    });
                });

                ui.add_space(12.0);

                let query = self.search.trim().to_lowercase();
                let filtered: Vec<&NativeSourceEntry> = self
                    .entries
                    .iter()
                    .filter(|e| e.source.kind == self.active_tab)
                    .filter(|e| query.is_empty() || e.source.name.to_lowercase().contains(&query))
                    .collect();

                let body_h = (modal_h - 172.0).max(200.0);
                egui::ScrollArea::vertical()
                    .max_height(body_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.loading && self.entries.is_empty() {
                            ui.add_space(64.0);
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new(
                                        "Capturando miniaturas nativas das telas e janelas…",
                                    )
                                    .color(prism.palette.text_secondary),
                                );
                            });
                            return;
                        }

                        if filtered.is_empty() {
                            ui.add_space(56.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("Nenhuma fonte encontrada nesta aba.")
                                        .size(14.5)
                                        .strong()
                                        .color(prism.palette.text_primary),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(
                                        "Abra o jogo ou programa que deseja capturar e clique em Atualizar prévias.",
                                    )
                                    .size(12.0)
                                    .color(prism.palette.text_secondary),
                                );
                            });
                            return;
                        }

                        let cols = 3usize;
                        let gap = 12.0;
                        let card_w = ((ui.available_width() - gap * (cols as f32 - 1.0))
                            / cols as f32)
                            .max(150.0);
                        let thumb_h = (card_w * 9.0 / 16.0).clamp(86.0, 150.0);

                        for row in filtered.chunks(cols) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = gap;
                                for entry in row {
                                    let is_selected = self.selected_id.as_deref()
                                        == Some(entry.source.id.as_str());
                                    let card_h = thumb_h + 46.0;
                                    let (rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(card_w, card_h),
                                        Sense::click(),
                                    );
                                    if resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }

                                    // Micro-elevação com física de mola (`egui-prismglass::animate_hover`)
                                    let hover_t = animate_hover(
                                        ui.ctx(),
                                        ui.make_persistent_id(("picker_card", &entry.source.id)),
                                        resp.hovered(),
                                    );
                                    let card_rect = rect.translate(Vec2::new(0.0, -2.0 * hover_t));

                                    let mut card_tokens = prism
                                        .surface(if is_selected {
                                            GlassElevation::Elevated
                                        } else {
                                            GlassElevation::Subtle
                                        })
                                        .with_corner_radius(13.0);

                                    if is_selected {
                                        card_tokens = card_tokens
                                            .with_tint(theme::selected_card_tint(&prism))
                                            .with_specular_scale(1.4);
                                        card_tokens.border = prism.palette.accent;
                                        card_tokens.border_width = 1.5;
                                    } else if hover_t > 0.01 {
                                        card_tokens = card_tokens
                                            .with_specular_scale(1.0 + 0.4 * hover_t)
                                            .with_opacity_scale(1.0 + 0.08 * hover_t);
                                    }

                                    let p = ui.painter();
                                    paint_glass_surface(p, card_rect, &card_tokens);

                                    // Área da miniatura real 16:9
                                    let thumb_rect = Rect::from_min_size(
                                        card_rect.min + Vec2::new(6.0, 6.0),
                                        Vec2::new(card_w - 12.0, thumb_h - 6.0),
                                    );
                                    p.rect_filled(
                                        thumb_rect,
                                        CornerRadius::same(8),
                                        theme::media_well_fill(&prism),
                                    );

                                    if let Some(tex) = self.thumb_textures.get(&entry.source.id) {
                                        let tex_sz = tex.size_vec2();
                                        let aspect = if tex_sz.y > 0.0 {
                                            tex_sz.x / tex_sz.y
                                        } else {
                                            16.0 / 9.0
                                        };
                                        let box_aspect = thumb_rect.width() / thumb_rect.height();
                                        let fit = if aspect > box_aspect {
                                            let w = thumb_rect.width();
                                            let h = w / aspect;
                                            Rect::from_center_size(
                                                thumb_rect.center(),
                                                Vec2::new(w, h),
                                            )
                                        } else {
                                            let h = thumb_rect.height();
                                            let w = h * aspect;
                                            Rect::from_center_size(
                                                thumb_rect.center(),
                                                Vec2::new(w, h),
                                            )
                                        };
                                        p.image(
                                            tex.id(),
                                            fit,
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    } else {
                                        let tag = if entry.source.kind == SourceKind::Screen {
                                            "MONITOR"
                                        } else {
                                            "JANELA"
                                        };
                                        p.text(
                                            thumb_rect.center(),
                                            Align2::CENTER_CENTER,
                                            tag,
                                            FontId::monospace(11.0),
                                            prism.palette.text_muted,
                                        );
                                    }

                                    // Badge de resolução no canto inferior direito
                                    if let Some((rw, rh)) = entry.resolution {
                                        let badge = format!("{rw}×{rh}");
                                        let b_rect = Rect::from_min_max(
                                            Pos2::new(
                                                thumb_rect.right() - 74.0,
                                                thumb_rect.bottom() - 20.0,
                                            ),
                                            thumb_rect.max - Vec2::new(4.0, 4.0),
                                        );
                                        p.rect_filled(
                                            b_rect,
                                            CornerRadius::same(6),
                                            Color32::from_rgba_unmultiplied(12, 13, 16, 195),
                                        );
                                        p.rect_stroke(
                                            b_rect,
                                            CornerRadius::same(6),
                                            Stroke::new(
                                                1.0,
                                                Color32::from_rgba_unmultiplied(255, 255, 255, 36),
                                            ),
                                            StrokeKind::Inside,
                                        );
                                        p.text(
                                            b_rect.center(),
                                            Align2::CENTER_CENTER,
                                            badge,
                                            FontId::monospace(9.5),
                                            Color32::from_rgb(0xF5, 0xF3, 0xEE),
                                        );
                                    }

                                    // Título da fonte
                                    let title_rect = Rect::from_min_max(
                                        Pos2::new(card_rect.left() + 10.0, thumb_rect.bottom() + 4.0),
                                        Pos2::new(card_rect.right() - 10.0, card_rect.bottom() - 6.0),
                                    );
                                    let mut label = entry.source.name.clone();
                                    if label.chars().count() > 28 {
                                        label = format!(
                                            "{}…",
                                            label.chars().take(27).collect::<String>()
                                        );
                                    }
                                    p.text(
                                        Pos2::new(title_rect.left(), title_rect.center().y),
                                        Align2::LEFT_CENTER,
                                        label,
                                        FontId::proportional(12.5),
                                        if is_selected {
                                            prism.palette.text_primary
                                        } else {
                                            prism.palette.text_secondary
                                        },
                                    );

                                    if resp.clicked() {
                                        self.selected_id = Some(entry.source.id.clone());
                                    }
                                    if resp.double_clicked() {
                                        result = Some(PickerSelection {
                                            purpose: self.purpose.clone(),
                                            source_id: entry.source.id.clone(),
                                            source_name: entry.source.name.clone(),
                                            kind: entry.source.kind,
                                        });
                                        close_now = true;
                                    }
                                }
                            });
                            ui.add_space(gap);
                        }
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // Rodapé do modal
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Duplo clique seleciona e inicia a captura na hora.")
                            .size(11.5)
                            .color(prism.palette.text_muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let selected_entry = self
                            .selected_id
                            .as_deref()
                            .and_then(|id| self.entries.iter().find(|e| e.source.id == id));

                        let confirm_label = if self.active_tab == SourceKind::Screen {
                            "Usar esta tela"
                        } else {
                            "Usar esta janela"
                        };
                        let confirm_resp = theme::tally_button_ex(
                            ui,
                            confirm_label,
                            true,
                            selected_entry.is_some(),
                        );
                        if confirm_resp.clicked() {
                            if let Some(entry) = selected_entry {
                                result = Some(PickerSelection {
                                    purpose: self.purpose.clone(),
                                    source_id: entry.source.id.clone(),
                                    source_name: entry.source.name.clone(),
                                    kind: entry.source.kind,
                                });
                                close_now = true;
                            }
                        }
                        if ui
                            .add(
                                GlassButton::ghost("Cancelar")
                                    .pill(true)
                                    .min_size(Vec2::new(82.0, 32.0)),
                            )
                            .clicked()
                        {
                            close_now = true;
                        }
                    });
                });
            });

        if close_now {
            self.open = false;
        }
        result
    }
}

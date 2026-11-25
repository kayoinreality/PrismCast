//! Seletor nativo de telas e janelas com miniaturas reais ao vivo.
//!
//! Substitui completamente o popup de compartilhamento de navegador do Tauri/WebView2:
//! - Enumera monitores e janelas diretamente pela API nativa do SO em thread dedicada.
//! - Renderiza previews reais (`egui::TextureHandle`) em grade glassmorphism 3 colunas.
//! - Selecionar uma janela inicia a captura nativa em Rust imediatamente sem popup,
//!   sem barra `"tauri.localhost"` e sem tirar o foco do PrismCast Studio.

use crate::capture::{self, NativeSourceEntry};
use crate::native_ui::theme;
use crate::protocol::SourceKind;
use egui::{
    Align2, Color32, ColorImage, Context, CornerRadius, FontId, Frame, Margin, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2,
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
    pub fn open(&mut self, purpose: PickerPurpose, initial_tab: SourceKind, current_id: Option<String>) {
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

                // Se a aba atual estiver vazia mas a outra tiver itens, mantém a seleção limpa
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

    /// Renderiza o modal glassmorphism do seletor e retorna `Some(PickerSelection)` quando confirmado.
    pub fn show(&mut self, ctx: &Context) -> Option<PickerSelection> {
        if !self.open {
            return None;
        }
        self.poll_results(ctx);

        let mut result: Option<PickerSelection> = None;
        let mut close_now = false;

        // Backdrop escuro fosco sobre o estúdio
        let screen_rect = ctx.screen_rect();
        egui::Area::new(egui::Id::new("source_picker_backdrop"))
            .fixed_pos(screen_rect.min)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter()
                    .rect_filled(screen_rect, 0.0, Color32::from_black_alpha(155));
                if resp.clicked() {
                    close_now = true;
                }
            });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_now = true;
        }

        let modal_w = (screen_rect.width() - 80.0).clamp(560.0, 900.0);
        let modal_h = (screen_rect.height() - 90.0).clamp(420.0, 660.0);

        egui::Window::new("source_picker_window")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(Vec2::new(modal_w, modal_h))
            .order(egui::Order::Foreground)
            .frame(
                Frame::new()
                    .fill(theme::GLASS_MODAL)
                    .stroke(Stroke::new(1.0_f32, theme::GLASS_EDGE_STRONG))
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(Margin::same(18))
                    .shadow(egui::Shadow {
                        offset: [0, 18],
                        blur: 40,
                        spread: 0,
                        color: Color32::from_black_alpha(180),
                    }),
            )
            .show(ctx, |ui| {
                theme::paint_glass_top_sheen(ui.painter(), ui.max_rect().expand(18.0));

                // Cabeçalho do modal
                ui.horizontal(|ui| {
                    let title = if self.active_tab == SourceKind::Screen {
                        "Escolha a tela"
                    } else {
                        "Escolha a janela"
                    };
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(title)
                                .size(19.0)
                                .strong()
                                .color(theme::INK),
                        );
                        ui.label(
                            RichText::new(
                                "Captura 100% nativa em Rust — sem popup, sem barra de compartilhamento e sem roubar foco.",
                            )
                            .size(12.0)
                            .color(theme::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("✕").size(15.0)).clicked() {
                            close_now = true;
                        }
                    });
                });

                ui.add_space(10.0);

                // Abas Telas / Janelas + busca + atualizar
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

                    if theme::glass_toggle_button(
                        ui,
                        &format!("Telas ({screen_count})"),
                        self.active_tab == SourceKind::Screen,
                    )
                    .clicked()
                    {
                        self.active_tab = SourceKind::Screen;
                    }
                    if theme::glass_toggle_button(
                        ui,
                        &format!("Janelas ({window_count})"),
                        self.active_tab == SourceKind::Window,
                    )
                    .clicked()
                    {
                        self.active_tab = SourceKind::Window;
                    }

                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .desired_width(220.0)
                            .hint_text("Filtrar por nome…"),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.loading,
                                egui::Button::new(if self.loading {
                                    "Atualizando…"
                                } else {
                                    "↻ Atualizar prévias"
                                }),
                            )
                            .clicked()
                        {
                            self.refresh();
                        }
                    });
                });

                ui.add_space(10.0);

                let query = self.search.trim().to_lowercase();
                let filtered: Vec<&NativeSourceEntry> = self
                    .entries
                    .iter()
                    .filter(|e| e.source.kind == self.active_tab)
                    .filter(|e| query.is_empty() || e.source.name.to_lowercase().contains(&query))
                    .collect();

                let body_h = (modal_h - 156.0).max(200.0);
                egui::ScrollArea::vertical()
                    .max_height(body_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.loading && self.entries.is_empty() {
                            ui.add_space(60.0);
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                                ui.add_space(8.0);
                                ui.label(
                                    RichText::new("Capturando miniaturas nativas das telas e janelas…")
                                        .color(theme::MUTED),
                                );
                            });
                            return;
                        }

                        if filtered.is_empty() {
                            ui.add_space(54.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("Nenhuma fonte encontrada nesta aba.")
                                        .size(14.5)
                                        .strong()
                                        .color(theme::INK),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(
                                        "Abra o jogo ou programa que deseja capturar e clique em Atualizar prévias.",
                                    )
                                    .size(12.0)
                                    .color(theme::MUTED),
                                );
                            });
                            return;
                        }

                        let cols = 3usize;
                        let gap = 12.0;
                        let card_w = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32)
                            .max(150.0);
                        let thumb_h = (card_w * 9.0 / 16.0).clamp(84.0, 146.0);

                        for row in filtered.chunks(cols) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = gap;
                                for entry in row {
                                    let is_selected =
                                        self.selected_id.as_deref() == Some(entry.source.id.as_str());
                                    let card_h = thumb_h + 44.0;
                                    let (rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(card_w, card_h),
                                        Sense::click(),
                                    );
                                    let hovered = resp.hovered();

                                    let bg = if is_selected {
                                        Color32::from_rgba_unmultiplied(54, 48, 39, 235)
                                    } else if hovered {
                                        theme::GLASS_CARD_HOVER
                                    } else {
                                        theme::GLASS_CARD
                                    };
                                    let border = if is_selected {
                                        Stroke::new(1.6_f32, theme::SPEC_AMBER)
                                    } else if hovered {
                                        Stroke::new(1.0_f32, theme::GLASS_EDGE_STRONG)
                                    } else {
                                        Stroke::new(1.0_f32, theme::GLASS_EDGE)
                                    };

                                    let p = ui.painter();
                                    p.rect_filled(rect, CornerRadius::same(12), bg);
                                    p.rect_stroke(
                                        rect,
                                        CornerRadius::same(12),
                                        border,
                                        StrokeKind::Inside,
                                    );

                                    // Área da miniatura real 16:9
                                    let thumb_rect = Rect::from_min_size(
                                        rect.min + Vec2::new(6.0, 6.0),
                                        Vec2::new(card_w - 12.0, thumb_h - 6.0),
                                    );
                                    p.rect_filled(
                                        thumb_rect,
                                        CornerRadius::same(8),
                                        Color32::from_rgb(0x0D, 0x0D, 0x0B),
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
                                        // Fallback estilizado quando janela está minimizada ou em modo headless
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
                                            theme::DIM,
                                        );
                                    }

                                    // Badge de resolução no canto inferior direito da miniatura
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
                                            CornerRadius::same(5),
                                            Color32::from_black_alpha(175),
                                        );
                                        p.text(
                                            b_rect.center(),
                                            Align2::CENTER_CENTER,
                                            badge,
                                            FontId::monospace(9.5),
                                            theme::INK,
                                        );
                                    }

                                    // Título da fonte
                                    let title_rect = Rect::from_min_max(
                                        Pos2::new(rect.left() + 9.0, thumb_rect.bottom() + 4.0),
                                        Pos2::new(rect.right() - 9.0, rect.bottom() - 6.0),
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
                                        if is_selected { theme::INK } else { theme::MUTED },
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
                ui.add_space(6.0);

                // Rodapé do modal
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Duplo clique seleciona e inicia a captura na hora.")
                            .size(11.5)
                            .color(theme::DIM),
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
                        let confirm_resp = ui.add_enabled(
                            selected_entry.is_some(),
                            egui::Button::new(RichText::new(confirm_label).color(theme::INK).strong())
                                .fill(theme::TALLY)
                                .corner_radius(CornerRadius::same(9)),
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
                        if ui.button("Cancelar").clicked() {
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

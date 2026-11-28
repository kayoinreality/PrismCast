//! Telas e painéis do PrismCast Studio na linguagem Prism da `egui-prismglass` 0.3.
//!
//! - **Barra superior** sólida com contexto (aba / cena), Tally com cronômetro e grupos de
//!   ações com ícones vetoriais (modos de visualização · gravação · CTA ao vivo).
//! - **Trilho lateral** em vidro (a camada flutuante do material Layered) com marca, seções
//!   Produção / Sistema, badges vivos e alternador rápido claro/escuro.
//! - **Barra de status** com telemetria tabular (FPS, render p95, quadros perdidos, bitrate,
//!   encoder, backend GPU, áudio e clientes externos).
//! - **Estúdio**: palco 16:9 / 9:16 / Modo Estúdio, docks de Cenas, Fontes e Mixer com linhas
//!   de lista, estados vazios e inspetor com abas segmentadas (Propriedades · Chat).
//! - Telas de `Destinos`, `Widgets`, `Rede Local` e `Ajustes` (com galeria de temas e cores).

use crate::capture::CaptureStatus;
use crate::compositor::capture_key_for_item;
use crate::native_ui::icons::Icon;
use crate::native_ui::picker::PickerPurpose;
use crate::native_ui::stage::{show_stage_viewport, StageAction, StageInteraction};
use crate::native_ui::theme::{self, Btn, BtnOpts, RichTextWeight};
use crate::native_ui::{
    ensure_window_audio_channel, source_type_badge, transition_label, BgUiMsg, NavTab,
    PrismCastNativeApp, RightTab, ToastNotice,
};
use crate::protocol::{
    AudioChannel, AudioChannelKind, AudioFilters, AudioMonitorMode, CanvasOrientation,
    ChatFragment, ChatPlatform, Destination, DiscoveredHost, PlatformId, Scene, SceneItem,
    SourceKind, SourceType, TransitionType, TwitchLoginState, UiAccent, UiAppearanceSettings,
    UiThemePreset, UpdateState,
};
use egui::{
    Align, Align2, Color32, Context, Frame, Layout, Margin, RichText, Ui, Vec2, ViewportCommand,
};
use egui_prismglass::{
    animate_spring, paint_glass_surface, BadgeVariant, FontWeight, GlassBadge, GlassButton,
    GlassCard, GlassElevation, GlassPanel, GlassSegmented, GlassSidebar, GlassSlider, GlassToggle,
    PrismContextExt, SpringConfig, SurfaceStyle, ThemeMode,
};
use serde_json::{json, Value};
use uuid::Uuid;

// ------------------------------------------------------------------ Helpers de layout

/// Margem externa padrão entre a janela e os painéis flutuantes.
const GUTTER: i8 = 8;

fn tab_title(tab: NavTab) -> &'static str {
    match tab {
        NavTab::Studio => "Estúdio",
        NavTab::Widgets => "Widgets",
        NavTab::Destinations => "Destinos",
        NavTab::Lan => "Rede local",
        NavTab::Settings => "Ajustes",
    }
}

fn tab_icon(tab: NavTab) -> Icon {
    match tab {
        NavTab::Studio => Icon::Studio,
        NavTab::Widgets => Icon::Widgets,
        NavTab::Destinations => Icon::Broadcast,
        NavTab::Lan => Icon::Network,
        NavTab::Settings => Icon::Settings,
    }
}

/// Ícone vetorial para cada tipo de fonte.
pub fn source_icon(t: SourceType) -> Icon {
    match t {
        SourceType::ScreenCapture => Icon::Monitor,
        SourceType::WindowCapture => Icon::Window,
        SourceType::ImageSource => Icon::Image,
        SourceType::ColorSource => Icon::Swatch,
        SourceType::TextSource => Icon::Text,
        SourceType::BrowserSource => Icon::Globe,
        SourceType::CameraInput => Icon::Camera,
        SourceType::AudioInput => Icon::Mic,
        SourceType::AudioOutput => Icon::Volume,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    } else {
        s.to_owned()
    }
}

/// Tempo no ar: maior uptime entre os destinos ativos.
fn live_uptime(app: &PrismCastNativeApp) -> Option<u64> {
    if app.multistream.active {
        Some(
            app.multistream
                .destinations
                .iter()
                .map(|d| d.uptime_sec)
                .max()
                .unwrap_or(0),
        )
    } else if app.recorder.recording {
        Some(app.recorder.recordingDurationSec)
    } else {
        None
    }
}

/// Margem das telas de página: centraliza o conteúdo numa coluna de até 1000 pt.
fn page_margin(ui: &Ui) -> Margin {
    let w = ui.available_width();
    let side = ((w - 1000.0) * 0.5).max(18.0).min(400.0);
    Margin {
        left: side as i8,
        right: side as i8,
        top: 14,
        bottom: 12,
    }
}

// ------------------------------------------------------------------ Barra Superior

pub fn show_top_bar(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    egui::Panel::top("prismcast_top_bar")
        .exact_size(60.0)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: GUTTER,
            right: GUTTER,
            top: GUTTER + 2,
            bottom: 2,
        }))
        .show(ui, |ui| {
            GlassPanel::new()
                .padding(Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        // Contexto: aba ativa › cena em edição
                        theme::icon(
                            ui,
                            tab_icon(app.active_tab),
                            16.0,
                            prism.palette.text_secondary,
                        );
                        ui.label(theme::text(
                            ui,
                            tab_title(app.active_tab),
                            14.0,
                            FontWeight::Semibold,
                            prism.palette.text_primary,
                        ));
                        if app.active_tab == NavTab::Studio {
                            theme::icon(ui, Icon::ChevronRight, 12.0, prism.palette.text_muted);
                            let scene = app
                                .program_scene()
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| "Cena principal".into());
                            ui.label(theme::text(
                                ui,
                                truncate(&scene, 26),
                                13.0,
                                FontWeight::Medium,
                                prism.palette.text_secondary,
                            ));
                        }
                        ui.add_space(10.0);
                        theme::paint_tally_pill(
                            ui,
                            app.multistream.active,
                            app.recorder.recording,
                            live_uptime(app),
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;

                            // CTA principal
                            let cta = if app.multistream.active {
                                theme::button(
                                    ui,
                                    "Encerrar live",
                                    BtnOpts::new(Btn::Danger).icon(Icon::Stop).height(32.0),
                                )
                            } else {
                                theme::button(
                                    ui,
                                    "Entrar ao vivo",
                                    BtnOpts::new(Btn::Tally).icon(Icon::Broadcast).height(32.0),
                                )
                            };
                            if cta.clicked() {
                                app.toggle_live();
                            }

                            let rec_label = if app.recorder.recording {
                                theme::format_hms(app.recorder.recordingDurationSec)
                            } else {
                                "Gravar".to_string()
                            };
                            if theme::button(
                                ui,
                                &rec_label,
                                BtnOpts::new(Btn::Secondary)
                                    .icon(if app.recorder.recording {
                                        Icon::Stop
                                    } else {
                                        Icon::Record
                                    })
                                    .selected(app.recorder.recording)
                                    .height(32.0),
                            )
                            .on_hover_text("Gravação local MP4")
                            .clicked()
                            {
                                app.toggle_recording();
                            }

                            if theme::button(
                                ui,
                                "Replay",
                                BtnOpts::new(Btn::Ghost).icon(Icon::Replay).height(32.0),
                            )
                            .on_hover_text("Salvar os últimos segundos do Replay Buffer")
                            .clicked()
                            {
                                app.save_replay();
                            }

                            theme::vsep(ui, &prism, 22.0);

                            // Modos de visualização (alternâncias)
                            let shield = app.settings.studio.privacy_shield_active;
                            if theme::icon_button_ex(
                                ui,
                                Icon::Pause,
                                "Tela de pausa (Volto já)",
                                shield,
                                30.0,
                            )
                            .clicked()
                            {
                                app.settings.studio.privacy_shield_active = !shield;
                                app.persist_studio();
                            }
                            let dual = app.settings.studio.dual_output_enabled;
                            if theme::icon_button_ex(
                                ui,
                                Icon::Vertical,
                                "Saída dupla vertical 9:16 (TikTok / Shorts)",
                                dual,
                                30.0,
                            )
                            .clicked()
                            {
                                app.settings.studio.dual_output_enabled = !dual;
                                if !app.settings.studio.dual_output_enabled {
                                    app.edit_vertical = false;
                                }
                                app.persist_studio();
                            }
                            let sm = app.settings.scenes.studio_mode;
                            if theme::icon_button_ex(
                                ui,
                                Icon::Columns,
                                "Modo estúdio: prévia editável + programa",
                                sm,
                                30.0,
                            )
                            .clicked()
                            {
                                app.settings.scenes.studio_mode = !sm;
                                if app.settings.scenes.studio_mode
                                    && app.settings.scenes.preview_scene_id.is_empty()
                                {
                                    app.settings.scenes.preview_scene_id =
                                        app.settings.scenes.active_scene_id.clone();
                                }
                                app.persist_scenes();
                            }
                        });
                    });
                });
        });
}

// ------------------------------------------------------------------ Barra de Status

pub fn show_status_bar(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    if !app.settings.ui_appearance.show_status_bar {
        return;
    }
    let prism = ui.ctx().prism_theme();
    egui::Panel::bottom("prismcast_status_bar")
        .exact_size(30.0)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: GUTTER + 4,
            right: GUTTER + 4,
            top: 2,
            bottom: 6,
        }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let t = &app.telemetry;
                let fps = if t.compositor_fps > 0.1 {
                    t.compositor_fps
                } else {
                    app.settings.studio.fps as f32
                };
                let fps_ok = t.target_fps == 0 || fps >= t.target_fps as f32 * 0.95;
                theme::status_dot(
                    ui,
                    if fps_ok {
                        theme::METER_OK
                    } else {
                        theme::METER_WARN
                    },
                    false,
                );
                theme::stat(ui, "", &format!("{fps:.0} fps"), prism.palette.text_primary);
                ui.add_space(8.0);
                theme::stat(
                    ui,
                    "Render",
                    &format!("{:.1} ms", t.render_ms_p95),
                    prism.palette.text_secondary,
                );
                ui.add_space(8.0);
                theme::stat(
                    ui,
                    "Perdidos",
                    &t.dropped_frames.to_string(),
                    if t.dropped_frames > 0 {
                        theme::METER_WARN
                    } else {
                        prism.palette.text_secondary
                    },
                );
                if app.multistream.active {
                    ui.add_space(8.0);
                    let kbps = if t.encode_bitrate_kbps > 0 {
                        t.encode_bitrate_kbps
                    } else {
                        app.multistream
                            .destinations
                            .iter()
                            .map(|d| d.bitrate_kbps)
                            .sum()
                    };
                    theme::stat(
                        ui,
                        "Bitrate",
                        &format!("{kbps} kbps"),
                        prism.palette.text_secondary,
                    );
                    if t.abr_active {
                        theme::chip(ui, "ABR", prism.palette.accent);
                    }
                }
                let encoder = if !t.encoder.is_empty() {
                    t.encoder.clone()
                } else {
                    app.multistream.encoder.clone()
                };
                if !encoder.is_empty() {
                    ui.add_space(8.0);
                    theme::stat(
                        ui,
                        "Encoder",
                        &encoder.to_uppercase(),
                        prism.palette.text_secondary,
                    );
                }
                ui.add_space(8.0);
                let backend = if app.gpu_status.compositor_backend.is_empty() {
                    "wgpu".to_string()
                } else {
                    app.gpu_status.compositor_backend.clone()
                };
                theme::stat(ui, "GPU", &backend, prism.palette.text_secondary);
                if app.gpu_status.zero_copy_active || t.zero_copy_active {
                    theme::chip(ui, "ZERO-COPY", theme::METER_OK);
                }
                if t.audio_runtime_active {
                    ui.add_space(8.0);
                    theme::stat(ui, "Áudio", "48 kHz", prism.palette.text_secondary);
                }
                if app.virtual_cam.active {
                    theme::chip(ui, "VCam", prism.palette.accent);
                }
                if t.external_clients > 0 {
                    ui.add_space(8.0);
                    theme::stat(
                        ui,
                        "Stream Deck",
                        &t.external_clients.to_string(),
                        prism.palette.text_secondary,
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(theme::num_text(
                        ui,
                        format!("v{}", env!("CARGO_PKG_VERSION")),
                        11.0,
                        FontWeight::Regular,
                        prism.palette.text_muted,
                    ));
                    if app.lan_hosting {
                        ui.add_space(8.0);
                        theme::chip(ui, "LAN host", theme::SPEC_BLUE);
                    }
                });
            });
        });
}

// ------------------------------------------------------------------ Trilho Lateral (GlassSidebar)

/// Item de navegação com ícone vetorial, rótulo Inter e badge opcional.
fn nav_item(
    ui: &mut Ui,
    ic: Icon,
    label: &str,
    selected: bool,
    badge: Option<&str>,
    collapsed: bool,
) -> egui::Response {
    let prism = ui.ctx().prism_theme();
    let h = 34.0;
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), h), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let cr =
        egui::CornerRadius::same((prism.radius.md * prism.radius_scale).clamp(4.0, 12.0) as u8);
    if selected {
        let fill = if prism.surface_style.is_prism() {
            prism.neutrals.selected
        } else {
            theme::selected_card_tint(&prism)
        };
        p.rect_filled(rect, cr, fill);
    } else if resp.hovered() {
        p.rect_filled(rect, cr, theme::hover_fill(&prism));
    }
    let icon_col = if selected {
        prism.palette.accent
    } else {
        prism.palette.text_secondary
    };
    let icon_x = if collapsed {
        rect.center().x - 8.0
    } else {
        rect.left() + 10.0
    };
    let ir =
        egui::Rect::from_min_size(egui::pos2(icon_x, rect.center().y - 8.0), Vec2::splat(16.0));
    crate::native_ui::icons::paint_icon(p, ir, ic, icon_col);
    if !collapsed {
        let f = theme::font(ui, 13.0, FontWeight::Medium);
        p.text(
            egui::pos2(rect.left() + 36.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            f,
            if selected {
                prism.palette.text_primary
            } else {
                prism.palette.text_secondary
            },
        );
        if let Some(b) = badge {
            let bf = theme::num_font(ui, 10.5, FontWeight::Semibold);
            let g = p.layout_no_wrap(b.to_owned(), bf, prism.palette.text_secondary);
            let br = egui::Rect::from_min_size(
                egui::pos2(rect.right() - g.size().x - 18.0, rect.center().y - 9.0),
                Vec2::new(g.size().x + 10.0, 18.0),
            );
            let live = b == "AR";
            let (bg, fg) = if live {
                (theme::TALLY, Color32::WHITE)
            } else {
                (
                    theme::hover_fill(&prism).gamma_multiply(2.0),
                    prism.palette.text_secondary,
                )
            };
            p.rect_filled(br, 9.0, bg);
            p.galley_with_override_text_color(br.center() - g.size() * 0.5, g, fg);
        }
    } else if let Some(b) = badge {
        let dot = egui::pos2(ir.right() + 2.0, ir.top());
        p.circle_filled(
            dot,
            3.5,
            if b == "AR" {
                theme::TALLY
            } else {
                prism.palette.accent
            },
        );
    }
    if collapsed {
        resp.on_hover_text(label)
    } else {
        resp
    }
}

pub fn show_nav_rail(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();
    let collapsed = app.settings.ui_appearance.sidebar_collapsed;

    let target_w = if collapsed { 60.0 } else { 204.0 };
    let anim_w = animate_spring(
        ui.ctx(),
        egui::Id::unique("prismcast_sidebar_width"),
        target_w,
        SpringConfig::smooth(),
    );
    let compact = collapsed || anim_w < 150.0;

    let enabled_dests = app
        .settings
        .destinations
        .iter()
        .filter(|d| d.enabled)
        .count();
    let widget_count = app
        .loaded_kit
        .as_ref()
        .map(|k| k.widgets.len())
        .unwrap_or(0);
    let has_update = matches!(
        app.updater.state,
        UpdateState::Available | UpdateState::Ready
    );

    let dest_badge = (enabled_dests > 0).then(|| enabled_dests.to_string());
    let widget_badge = (widget_count > 0).then(|| widget_count.to_string());
    let lan_badge = if app.lan_hosting {
        Some("HOST".to_string())
    } else if !app.lan_hosts.is_empty() {
        Some(app.lan_hosts.len().to_string())
    } else {
        None
    };

    egui::Panel::left("prismcast_nav_rail")
        .exact_size(anim_w + f32::from(GUTTER) + 2.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: GUTTER,
            right: 2,
            top: 2,
            bottom: 2,
        }))
        .show(ui, |ui| {
            GlassSidebar::new()
                .width(anim_w)
                .padding(Margin::symmetric(if compact { 8 } else { 10 }, 12))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    // Marca
                    ui.horizontal(|ui| {
                        let (logo_rect, logo_resp) =
                            ui.allocate_exact_size(Vec2::new(30.0, 26.0), egui::Sense::click());
                        theme::paint_prism_logo(ui.painter(), logo_rect, &prism);
                        if compact {
                            if logo_resp.on_hover_text("Expandir barra lateral").clicked() {
                                app.settings.ui_appearance.sidebar_collapsed = false;
                                app.persist_appearance();
                            }
                        } else {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(theme::text(
                                    ui,
                                    "PrismCast",
                                    14.0,
                                    FontWeight::Semibold,
                                    prism.palette.text_primary,
                                ));
                                ui.label(theme::text(
                                    ui,
                                    "Studio",
                                    11.0,
                                    FontWeight::Regular,
                                    prism.palette.text_muted,
                                ));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if theme::icon_button(
                                    ui,
                                    Icon::ChevronLeft,
                                    "Recolher barra lateral",
                                )
                                .clicked()
                                {
                                    app.settings.ui_appearance.sidebar_collapsed = true;
                                    app.persist_appearance();
                                }
                            });
                        }
                    });
                    ui.add_space(14.0);

                    let section = |ui: &mut Ui, title: &str| {
                        if compact {
                            ui.add_space(6.0);
                            theme::hairline(ui, &prism);
                            ui.add_space(6.0);
                        } else {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                theme::section_heading(ui, title, &prism);
                            });
                            ui.add_space(2.0);
                        }
                    };

                    section(ui, "Produção");
                    let items = [
                        (
                            NavTab::Studio,
                            app.multistream.active.then(|| "AR".to_string()),
                        ),
                        (NavTab::Widgets, widget_badge),
                        (NavTab::Destinations, dest_badge),
                    ];
                    for (tab, badge) in items {
                        if nav_item(
                            ui,
                            tab_icon(tab),
                            tab_title(tab),
                            app.active_tab == tab,
                            badge.as_deref(),
                            compact,
                        )
                        .clicked()
                        {
                            app.active_tab = tab;
                        }
                    }
                    ui.add_space(8.0);
                    section(ui, "Sistema");
                    let items = [
                        (NavTab::Lan, lan_badge),
                        (NavTab::Settings, has_update.then(|| "1".to_string())),
                    ];
                    for (tab, badge) in items {
                        if nav_item(
                            ui,
                            tab_icon(tab),
                            tab_title(tab),
                            app.active_tab == tab,
                            badge.as_deref(),
                            compact,
                        )
                        .clicked()
                        {
                            app.active_tab = tab;
                        }
                    }

                    // Rodapé: atualização + alternador claro/escuro
                    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                        let preset = app.settings.ui_appearance.preset;
                        let next = preset.toggled_polarity();
                        let ic = if preset.is_dark() {
                            Icon::Sun
                        } else {
                            Icon::Moon
                        };
                        let tip = format!(
                            "Mudar para {} ({})",
                            next.label(),
                            if next.is_dark() { "escuro" } else { "claro" }
                        );
                        if compact {
                            ui.vertical_centered(|ui| {
                                if theme::icon_button_ex(ui, ic, &tip, false, 32.0).clicked() {
                                    app.settings.ui_appearance.preset = next;
                                    app.persist_appearance();
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                if theme::icon_button_ex(ui, ic, &tip, false, 28.0).clicked() {
                                    app.settings.ui_appearance.preset = next;
                                    app.persist_appearance();
                                }
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.label(theme::text(
                                        ui,
                                        "Tema",
                                        10.5,
                                        FontWeight::Regular,
                                        prism.palette.text_muted,
                                    ));
                                    ui.label(theme::text(
                                        ui,
                                        preset.label(),
                                        12.0,
                                        FontWeight::Medium,
                                        prism.palette.text_secondary,
                                    ));
                                });
                            });
                        }
                        if has_update {
                            ui.add_space(6.0);
                            let resp = if compact {
                                theme::icon_button_ex(
                                    ui,
                                    Icon::Download,
                                    "Atualização disponível",
                                    true,
                                    32.0,
                                )
                            } else {
                                theme::button(
                                    ui,
                                    "Atualização disponível",
                                    BtnOpts::new(Btn::Accent)
                                        .icon(Icon::Download)
                                        .small()
                                        .min_width(ui.available_width()),
                                )
                            };
                            if resp.clicked() {
                                app.active_tab = NavTab::Settings;
                            }
                        }
                    });
                });
        });
}

// ------------------------------------------------------------------ Workspace do Estúdio

pub fn show_studio_workspace(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let prism = ctx.prism_theme();

    // 1. Inspetor à direita (Propriedades / Multichat)
    egui::Panel::right("studio_right_panel")
        .default_size(320.0)
        .size_range(276.0..=440.0)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: 2,
            right: GUTTER,
            top: 2,
            bottom: 2,
        }))
        .show(ui, |ui| {
            let panel_h = ui.available_height();
            GlassPanel::new()
                .padding(Margin::same(12))
                .min_height((panel_h - 24.0).max(180.0))
                .show(ui, |ui| {
                    let mut idx = match app.right_tab {
                        RightTab::Properties => 0usize,
                        RightTab::Chat => 1usize,
                    };
                    let chat_label = if app.chat_messages.is_empty() {
                        "Chat".to_string()
                    } else {
                        format!("Chat · {}", app.chat_messages.len().min(999))
                    };
                    if ui
                        .add(
                            GlassSegmented::new(&mut idx, &["Propriedades", chat_label.as_str()])
                                .fill_width(true)
                                .height(30.0),
                        )
                        .changed()
                    {
                        app.right_tab = if idx == 0 {
                            RightTab::Properties
                        } else {
                            RightTab::Chat
                        };
                    }
                    ui.add_space(12.0);
                    match app.right_tab {
                        RightTab::Properties => show_properties_panel(app, ui),
                        RightTab::Chat => show_chat_panel(app, ui),
                    }
                });
        });

    // 2. Dock inferior (Cenas | Fontes | Mixer de Áudio)
    egui::Panel::bottom("studio_bottom_dock")
        .default_size(262.0)
        .size_range(200.0..=400.0)
        .resizable(true)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: 2,
            right: 2,
            top: 4,
            bottom: 2,
        }))
        .show(ui, |ui| {
            let total_w = ui.available_width();
            let gap = 8.0;
            let col_scenes_w = (total_w * 0.26).clamp(200.0, 300.0);
            let col_sources_w = (total_w * 0.34).clamp(240.0, 400.0);
            let col_mixer_w = (total_w - col_scenes_w - col_sources_w - gap * 2.0).max(240.0);
            let dock_h = ui.available_height();

            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let cols: [(f32, fn(&mut PrismCastNativeApp, &mut Ui)); 3] = [
                    (col_scenes_w, show_scenes_dock),
                    (col_sources_w, show_sources_dock),
                    (col_mixer_w, show_mixer_dock),
                ];
                for (w, f) in cols {
                    ui.allocate_ui_with_layout(
                        Vec2::new(w, dock_h),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            GlassPanel::new()
                                .padding(Margin::symmetric(10, 10))
                                .min_height((dock_h - 20.0).max(140.0))
                                .show(ui, |ui| f(app, ui));
                        },
                    );
                }
            });
        });

    // 3. Palco central
    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(Margin::symmetric(6, 4)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                let scene_name = app
                    .editing_scene()
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "Cena principal".into());
                let studio_mode = app.settings.scenes.studio_mode;
                theme::chip(
                    ui,
                    if studio_mode { "Prévia" } else { "Cena" },
                    if studio_mode {
                        theme::SPEC_TEAL
                    } else {
                        prism.palette.text_secondary
                    },
                );
                ui.label(theme::text(
                    ui,
                    truncate(&scene_name, 32),
                    14.0,
                    FontWeight::Semibold,
                    prism.palette.text_primary,
                ));
                let n_items = app.editing_scene().map(|s| s.items.len()).unwrap_or(0);
                ui.label(theme::num_text(
                    ui,
                    format!("{n_items} fontes"),
                    12.0,
                    FontWeight::Regular,
                    prism.palette.text_muted,
                ));

                if app.settings.studio.dual_output_enabled && !studio_mode {
                    ui.add_space(10.0);
                    let mut idx = usize::from(app.edit_vertical);
                    if ui
                        .add(
                            GlassSegmented::new(&mut idx, &["Editar 16:9", "Editar 9:16"])
                                .height(26.0),
                        )
                        .changed()
                    {
                        app.edit_vertical = idx == 1;
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if studio_mode
                        && theme::button(
                            ui,
                            &format!(
                                "Transição · {}",
                                transition_label(app.settings.scenes.transition_type)
                            ),
                            BtnOpts::new(Btn::Tally).icon(Icon::Swap).small(),
                        )
                        .on_hover_text("Leva a prévia para o programa")
                        .clicked()
                        && !app.settings.scenes.preview_scene_id.is_empty()
                    {
                        app.settings.scenes.active_scene_id =
                            app.settings.scenes.preview_scene_id.clone();
                        app.persist_scenes();
                    }
                    let can_undo = !app.undo_stack.is_empty();
                    if theme::button(
                        ui,
                        "Desfazer",
                        BtnOpts::new(Btn::Ghost)
                            .icon(Icon::Undo)
                            .small()
                            .enabled(can_undo),
                    )
                    .on_hover_text(format!(
                        "Ctrl+Z · {} passos no histórico",
                        app.undo_stack.len()
                    ))
                    .clicked()
                    {
                        app.undo();
                    }
                });
            });

            ui.add_space(4.0);

            let stage_area_h = (ui.available_height() - 26.0).max(160.0);
            let mut stage_actions: Vec<StageAction> = Vec::new();
            let editing_scene = app.editing_scene().cloned();
            let program_scene = app.program_scene().cloned();
            let selected_id = app.selected_item_id.clone();
            let dual = app.settings.studio.dual_output_enabled;
            let studio_mode = app.settings.scenes.studio_mode;
            let live = app.multistream.active;

            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), stage_area_h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if studio_mode {
                        let half_w = (ui.available_width() - 8.0) * 0.5;
                        ui.allocate_ui(Vec2::new(half_w, stage_area_h), |ui| {
                            show_stage_viewport(
                                ui,
                                "stage_preview",
                                "PRÉVIA · EDITÁVEL",
                                theme::SPEC_TEAL,
                                app.tex_studio_preview
                                    .as_ref()
                                    .or(app.tex_program_h.as_ref()),
                                editing_scene.as_ref(),
                                false,
                                true,
                                selected_id.as_deref(),
                                &app.capture_statuses,
                                &mut app.stage_interaction,
                                &mut stage_actions,
                            );
                        });
                        ui.add_space(8.0);
                        ui.allocate_ui(Vec2::new(half_w, stage_area_h), |ui| {
                            let mut dummy = StageInteraction::default();
                            let mut ignored = Vec::new();
                            show_stage_viewport(
                                ui,
                                "stage_program_ro",
                                if live {
                                    "PROGRAMA · NO AR"
                                } else {
                                    "PROGRAMA"
                                },
                                theme::TALLY,
                                app.tex_program_h.as_ref(),
                                program_scene.as_ref(),
                                false,
                                false,
                                None,
                                &app.capture_statuses,
                                &mut dummy,
                                &mut ignored,
                            );
                        });
                    } else if dual {
                        let vert_w = (stage_area_h * 9.0 / 16.0 + 22.0).clamp(184.0, 340.0);
                        let horiz_w = (ui.available_width() - vert_w - 8.0).max(240.0);
                        ui.allocate_ui(Vec2::new(horiz_w, stage_area_h), |ui| {
                            show_stage_viewport(
                                ui,
                                "stage_h",
                                if live {
                                    "NO AR · 16:9"
                                } else {
                                    "16:9 · HORIZONTAL"
                                },
                                if live {
                                    theme::TALLY
                                } else if !app.edit_vertical {
                                    prism.palette.accent
                                } else {
                                    prism.palette.text_muted
                                },
                                app.tex_program_h.as_ref(),
                                editing_scene.as_ref(),
                                false,
                                !app.edit_vertical,
                                selected_id.as_deref(),
                                &app.capture_statuses,
                                &mut app.stage_interaction,
                                &mut stage_actions,
                            );
                        });
                        ui.add_space(8.0);
                        ui.allocate_ui(Vec2::new(vert_w, stage_area_h), |ui| {
                            show_stage_viewport(
                                ui,
                                "stage_v",
                                "9:16 · VERTICAL",
                                if app.edit_vertical {
                                    prism.palette.accent
                                } else {
                                    prism.palette.text_muted
                                },
                                app.tex_program_v.as_ref(),
                                editing_scene.as_ref(),
                                true,
                                app.edit_vertical,
                                selected_id.as_deref(),
                                &app.capture_statuses,
                                &mut app.stage_interaction,
                                &mut stage_actions,
                            );
                        });
                    } else {
                        show_stage_viewport(
                            ui,
                            "stage_main",
                            if live {
                                "NO AR · 16:9"
                            } else {
                                "PALCO · 16:9"
                            },
                            if live {
                                theme::TALLY
                            } else {
                                prism.palette.accent
                            },
                            app.tex_program_h.as_ref(),
                            editing_scene.as_ref(),
                            false,
                            true,
                            selected_id.as_deref(),
                            &app.capture_statuses,
                            &mut app.stage_interaction,
                            &mut stage_actions,
                        );
                    }
                },
            );

            if !stage_actions.is_empty() {
                app.apply_stage_actions(stage_actions);
            }

            // Dicas de atalho em capas de tecla
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let muted = prism.palette.text_muted;
                let hint = |ui: &mut Ui, s: &str| {
                    ui.label(theme::text(ui, s, 11.0, FontWeight::Regular, muted));
                };
                let avail = ui.available_width();
                let start = ((avail - 600.0) * 0.5).max(0.0);
                ui.add_space(start);
                hint(ui, "Arraste para mover ·");
                theme::kbd(ui, "Shift");
                hint(ui, "solta proporção ·");
                theme::kbd(ui, "Alt");
                hint(ui, "recorta ·");
                theme::kbd(ui, "←↑↓→");
                hint(ui, "ajuste fino ·");
                theme::kbd(ui, "Del");
                hint(ui, "remove ·");
                theme::kbd(ui, "Ctrl Z");
                hint(ui, "desfaz");
            });
        });

    if let Some(ch_id) = app.filter_channel_id.clone() {
        show_audio_filters_modal(app, &ctx, &ch_id);
    }
}

// ------------------------------------------------------------------ Dock: Cenas

fn transition_combo(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    egui::ComboBox::from_id_salt("transition_combo")
        .width(104.0)
        .selected_text(transition_label(app.settings.scenes.transition_type))
        .show_ui(ui, |ui| {
            for t in [
                TransitionType::Cut,
                TransitionType::Fade,
                TransitionType::FadeToBlack,
                TransitionType::Slide,
                TransitionType::Stinger,
                TransitionType::LumaWipe,
                TransitionType::CustomShader,
            ] {
                if ui
                    .selectable_value(
                        &mut app.settings.scenes.transition_type,
                        t,
                        transition_label(t),
                    )
                    .clicked()
                {
                    app.persist_scenes();
                }
            }
        });
}

fn show_scenes_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();
    let n = app.settings.scenes.scenes.len();

    theme::dock_header(ui, "Cenas", Some(n), &prism, |ui| {
        if theme::icon_button(ui, Icon::Plus, "Nova cena").clicked() {
            app.adding_scene = true;
            app.new_scene_name = format!("Cena {}", app.settings.scenes.scenes.len() + 1);
        }
    });

    let active_id = app.settings.scenes.active_scene_id.clone();
    let preview_id = app.settings.scenes.preview_scene_id.clone();
    let studio_mode = app.settings.scenes.studio_mode;

    let mut select_scene: Option<String> = None;
    let mut delete_scene: Option<String> = None;
    let mut start_rename: Option<(String, String)> = None;
    let mut commit_rename = false;

    egui::ScrollArea::vertical()
        .id_salt("scenes_scroll")
        .max_height((ui.available_height() - 40.0).max(80.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (idx, scene) in app.settings.scenes.scenes.iter().enumerate() {
                let is_selected = if studio_mode {
                    scene.id == preview_id
                } else {
                    scene.id == active_id
                };
                let is_on_air = scene.id == active_id;
                let renaming = app
                    .renaming_scene
                    .as_ref()
                    .is_some_and(|(id, _)| *id == scene.id);

                let resp =
                    theme::list_row(ui, is_selected, 32.0, Some(prism.palette.accent), |ui| {
                        ui.label(theme::num_text(
                            ui,
                            format!("{:02}", idx + 1),
                            11.0,
                            FontWeight::Medium,
                            if is_on_air && (studio_mode || app.multistream.active) {
                                theme::SPEC_RED
                            } else {
                                prism.palette.text_muted
                            },
                        ));
                        if renaming {
                            if let Some((_, buf)) = app.renaming_scene.as_mut() {
                                let r = ui.add(
                                    egui::TextEdit::singleline(buf)
                                        .desired_width(ui.available_width() - 34.0),
                                );
                                r.request_focus();
                                if r.lost_focus() {
                                    commit_rename = true;
                                }
                            }
                        } else {
                            ui.label(theme::text(
                                ui,
                                truncate(&scene.name, 22),
                                12.5,
                                if is_selected {
                                    FontWeight::Medium
                                } else {
                                    FontWeight::Regular
                                },
                                if is_selected {
                                    prism.palette.text_primary
                                } else {
                                    prism.palette.text_secondary
                                },
                            ));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if n > 1
                                && theme::icon_button(ui, Icon::Trash, "Remover cena").clicked()
                            {
                                delete_scene = Some(scene.id.clone());
                            }
                            if is_on_air && (studio_mode || app.multistream.active) {
                                theme::chip(ui, "No ar", theme::SPEC_RED);
                            }
                        });
                    });
                if resp.clicked() {
                    select_scene = Some(scene.id.clone());
                }
                if resp.double_clicked() {
                    start_rename = Some((scene.id.clone(), scene.name.clone()));
                }
                resp.on_hover_text("Clique para selecionar · duplo clique para renomear");
            }

            if app.adding_scene {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut app.new_scene_name)
                            .desired_width(ui.available_width() - 64.0)
                            .hint_text("Nome da cena"),
                    );
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if (theme::icon_button_ex(ui, Icon::Check, "Criar", true, 26.0).clicked()
                        || enter)
                        && !app.new_scene_name.trim().is_empty()
                    {
                        app.push_undo();
                        let new_id = format!("scene-{}", Uuid::new_v4());
                        app.settings.scenes.scenes.push(Scene {
                            id: new_id.clone(),
                            name: app.new_scene_name.trim().to_string(),
                            items: Vec::new(),
                            created_at: super::now_ms(),
                            transition_override: None,
                        });
                        if studio_mode {
                            app.settings.scenes.preview_scene_id = new_id;
                        } else {
                            app.settings.scenes.active_scene_id = new_id.clone();
                            app.settings.scenes.preview_scene_id = new_id;
                        }
                        app.new_scene_name.clear();
                        app.adding_scene = false;
                        app.persist_scenes();
                    }
                    if theme::icon_button(ui, Icon::Close, "Cancelar").clicked() || esc {
                        app.adding_scene = false;
                        app.new_scene_name.clear();
                    }
                });
            }
        });

    if let Some(r) = start_rename {
        app.renaming_scene = Some(r);
    }
    if commit_rename {
        if let Some((id, name)) = app.renaming_scene.take() {
            let name = name.trim().to_string();
            if !name.is_empty() {
                app.push_undo();
                if let Some(sc) = app.settings.scenes.scenes.iter_mut().find(|s| s.id == id) {
                    sc.name = name;
                }
                app.persist_scenes();
            }
        }
    }

    if let Some(id) = select_scene {
        if studio_mode {
            app.settings.scenes.preview_scene_id = id;
        } else {
            app.settings.scenes.active_scene_id = id.clone();
            app.settings.scenes.preview_scene_id = id;
        }
        app.selected_item_id = app
            .editing_scene()
            .and_then(|s| s.items.first())
            .map(|i| i.id.clone());
        app.persist_scenes();
    }

    if let Some(id) = delete_scene {
        app.push_undo();
        app.settings.scenes.scenes.retain(|s| s.id != id);
        if app.settings.scenes.active_scene_id == id {
            if let Some(first) = app.settings.scenes.scenes.first() {
                app.settings.scenes.active_scene_id = first.id.clone();
            }
        }
        if app.settings.scenes.preview_scene_id == id {
            app.settings.scenes.preview_scene_id = app.settings.scenes.active_scene_id.clone();
        }
        app.persist_scenes();
    }

    // Rodapé: transição + duração
    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            theme::icon(ui, Icon::Swap, 14.0, prism.palette.text_muted);
            transition_combo(app, ui);
            if ui
                .add(
                    egui::DragValue::new(&mut app.settings.scenes.transition_duration_ms)
                        .range(50..=3000)
                        .speed(25)
                        .suffix(" ms"),
                )
                .on_hover_text("Duração da transição")
                .changed()
            {
                app.persist_scenes();
            }
        });
        theme::hairline(ui, &prism);
    });
}

// ------------------------------------------------------------------ Dock: Fontes

fn show_sources_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();
    let count = app.editing_scene().map(|s| s.items.len()).unwrap_or(0);

    let mut open_add_menu = None;
    theme::dock_header(ui, "Fontes", Some(count), &prism, |ui| {
        let r = theme::icon_button(ui, Icon::Plus, "Adicionar fonte");
        open_add_menu = Some(r);
    });
    if let Some(r) = open_add_menu {
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(190.0);
            add_source_menu(app, ui);
        });
    }

    let items_snapshot: Vec<SceneItem> = app
        .editing_scene()
        .map(|sc| {
            let mut v = sc.items.clone();
            v.sort_by_key(|it| std::cmp::Reverse(it.transform.z_index));
            v
        })
        .unwrap_or_default();

    let mut select_id: Option<String> = None;
    let mut toggle_vis: Option<String> = None;
    let mut toggle_lock: Option<String> = None;
    let mut remove_id: Option<String> = None;
    let mut move_delta: Option<(String, i32)> = None;

    egui::ScrollArea::vertical()
        .id_salt("sources_scroll")
        .max_height((ui.available_height() - 40.0).max(80.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if items_snapshot.is_empty() {
                theme::empty_state(
                    ui,
                    Icon::Layers,
                    "Nenhuma fonte nesta cena",
                    "Adicione uma tela, janela, imagem ou texto.",
                );
            }
            for item in &items_snapshot {
                let selected = app.selected_item_id.as_deref() == Some(item.id.as_str());
                let status = capture_key_for_item(item)
                    .and_then(|k| app.capture_statuses.get(&k))
                    .copied();
                let is_capture = matches!(
                    item.source_type,
                    SourceType::ScreenCapture | SourceType::WindowCapture
                );

                let resp = theme::list_row(ui, selected, 32.0, Some(prism.palette.accent), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let icon_col = if !item.visible {
                        prism.palette.text_muted
                    } else if selected {
                        prism.palette.accent
                    } else {
                        prism.palette.text_secondary
                    };
                    theme::icon(ui, source_icon(item.source_type), 15.0, icon_col);
                    ui.label(theme::text(
                        ui,
                        truncate(&item.name, 20),
                        12.5,
                        if selected {
                            FontWeight::Medium
                        } else {
                            FontWeight::Regular
                        },
                        if !item.visible {
                            prism.palette.text_muted
                        } else if selected {
                            prism.palette.text_primary
                        } else {
                            prism.palette.text_secondary
                        },
                    ));
                    if is_capture {
                        let (col, tip) = match status {
                            Some(CaptureStatus::Ok) => (theme::METER_OK, "Capturando"),
                            Some(CaptureStatus::Missing) => (theme::SPEC_AMBER, "Janela fechada"),
                            Some(CaptureStatus::Error) => (theme::METER_CLIP, "Erro de captura"),
                            _ => (prism.palette.text_muted, "Conectando…"),
                        };
                        let (r, resp) =
                            ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.0, col);
                        resp.on_hover_text(tip);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if theme::icon_button(ui, Icon::Trash, "Remover fonte").clicked() {
                            remove_id = Some(item.id.clone());
                        }
                        if theme::icon_button(
                            ui,
                            if item.locked {
                                Icon::Lock
                            } else {
                                Icon::Unlock
                            },
                            "Travar / destravar",
                        )
                        .clicked()
                        {
                            toggle_lock = Some(item.id.clone());
                        }
                        if theme::icon_button(
                            ui,
                            if item.visible {
                                Icon::Eye
                            } else {
                                Icon::EyeOff
                            },
                            "Mostrar / ocultar",
                        )
                        .clicked()
                        {
                            toggle_vis = Some(item.id.clone());
                        }
                        if theme::icon_button(ui, Icon::ChevronDown, "Descer camada").clicked() {
                            move_delta = Some((item.id.clone(), -1));
                        }
                        if theme::icon_button(ui, Icon::ChevronUp, "Subir camada").clicked() {
                            move_delta = Some((item.id.clone(), 1));
                        }
                    });
                });
                if resp.clicked() {
                    select_id = Some(item.id.clone());
                }
                resp.on_hover_text(source_type_badge(item.source_type));
            }
        });

    if let Some(id) = select_id {
        app.selected_item_id = Some(id);
        app.right_tab = RightTab::Properties;
    }
    if let Some(id) = toggle_vis {
        app.push_undo();
        if let Some(sc) = app.editing_scene_mut() {
            if let Some(it) = sc.items.iter_mut().find(|i| i.id == id) {
                it.visible = !it.visible;
            }
        }
        app.persist_scenes();
    }
    if let Some(id) = toggle_lock {
        app.apply_stage_actions(vec![StageAction::ToggleLock(id)]);
    }
    if let Some(id) = remove_id {
        app.apply_stage_actions(vec![StageAction::RemoveItem(id)]);
    }
    if let Some((id, delta)) = move_delta {
        app.push_undo();
        if let Some(sc) = app.editing_scene_mut() {
            if let Some(it) = sc.items.iter_mut().find(|i| i.id == id) {
                it.transform.z_index = (it.transform.z_index + delta).max(0);
                it.vertical_transform.z_index = (it.vertical_transform.z_index + delta).max(0);
            }
        }
        app.persist_scenes();
    }

    // Rodapé: atalhos de captura nativa
    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if theme::button(
                ui,
                "Tela",
                BtnOpts::new(Btn::Accent).icon(Icon::Monitor).small(),
            )
            .clicked()
            {
                app.picker
                    .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
            }
            if theme::button(
                ui,
                "Janela",
                BtnOpts::new(Btn::Secondary).icon(Icon::Window).small(),
            )
            .clicked()
            {
                app.picker
                    .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
            }
            let more = theme::button(
                ui,
                "Mais",
                BtnOpts::new(Btn::Ghost).icon(Icon::Plus).small(),
            );
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(190.0);
                add_source_menu(app, ui);
            });
        });
        theme::hairline(ui, &prism);
    });
}

fn menu_entry(ui: &mut Ui, ic: Icon, label: &str) -> bool {
    let prism = ui.ctx().prism_theme();
    let resp = theme::list_row(ui, false, 28.0, None, |ui| {
        theme::icon(ui, ic, 14.0, prism.palette.text_secondary);
        ui.label(theme::text(
            ui,
            label,
            12.5,
            FontWeight::Regular,
            prism.palette.text_primary,
        ));
    });
    resp.clicked()
}

fn add_source_menu(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let mut close = false;
    if menu_entry(ui, Icon::Monitor, "Captura de tela") {
        app.picker
            .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
        close = true;
    }
    if menu_entry(ui, Icon::Window, "Captura de janela") {
        app.picker
            .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
        close = true;
    }
    if menu_entry(ui, Icon::Image, "Imagem (PNG / JPG / WebP)") {
        app.add_simple_source(SourceType::ImageSource);
        close = true;
    }
    if menu_entry(ui, Icon::Swatch, "Cor sólida") {
        app.add_simple_source(SourceType::ColorSource);
        close = true;
    }
    if menu_entry(ui, Icon::Text, "Texto") {
        app.add_simple_source(SourceType::TextSource);
        close = true;
    }
    if menu_entry(ui, Icon::Globe, "Widget HTML / URL") {
        app.add_simple_source(SourceType::BrowserSource);
        close = true;
    }
    if menu_entry(ui, Icon::Camera, "Câmera") {
        app.add_simple_source(SourceType::CameraInput);
        close = true;
    }
    if close {
        ui.close();
    }
}

// ------------------------------------------------------------------ Dock: Mixer de Áudio

fn show_mixer_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();
    let n = app.settings.audio_mixer.channels.len();

    let mut add_channel = false;
    theme::dock_header(ui, "Mixer", Some(n), &prism, |ui| {
        if theme::button(
            ui,
            "Canal de app",
            BtnOpts::new(Btn::Ghost).icon(Icon::Plus).small(),
        )
        .on_hover_text("Isola o áudio de um único programa (WASAPI / PipeWire)")
        .clicked()
        {
            add_channel = true;
        }
        theme::chip(ui, "48K", prism.palette.text_muted);
    });
    if add_channel {
        app.settings.audio_mixer.channels.push(AudioChannel {
            id: format!("app-{}", Uuid::new_v4()),
            kind: AudioChannelKind::App,
            name: "Aplicativo".to_string(),
            filters: AudioFilters::default_for(AudioChannelKind::App),
            ..Default::default()
        });
        app.persist_audio();
    }

    let mut dirty = false;
    let mut open_filters: Option<String> = None;
    let mut remove_ch: Option<String> = None;
    let avail_w = ui.available_width();

    egui::ScrollArea::vertical()
        .id_salt("mixer_scroll")
        .max_height((ui.available_height() - 4.0).max(110.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.settings.audio_mixer.channels.is_empty() {
                theme::empty_state(
                    ui,
                    Icon::Volume,
                    "Sem canais de áudio",
                    "Adicione um canal de aplicativo.",
                );
            }
            let count = app.settings.audio_mixer.channels.len();
            for (i, ch) in app.settings.audio_mixer.channels.iter_mut().enumerate() {
                let peak = app
                    .audio_levels
                    .get(&ch.id)
                    .map(|l| l.peak_db)
                    .unwrap_or(-60.0);
                let has_dsp = ch.filters.gate.enabled
                    || ch.filters.compressor.enabled
                    || ch.filters.eq_enabled
                    || ch.filters.limiter.enabled
                    || ch.filters.highpass_hz > 1.0;
                let ch_icon = match ch.kind {
                    AudioChannelKind::Mic => Icon::Mic,
                    AudioChannelKind::App => Icon::Window,
                    _ => Icon::Volume,
                };

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    theme::icon(
                        ui,
                        ch_icon,
                        14.0,
                        if ch.muted {
                            prism.palette.text_muted
                        } else {
                            prism.palette.text_secondary
                        },
                    );
                    ui.label(theme::text(
                        ui,
                        truncate(&ch.name, 22),
                        12.5,
                        FontWeight::Medium,
                        if ch.muted {
                            prism.palette.text_muted
                        } else {
                            prism.palette.text_primary
                        },
                    ));
                    if has_dsp {
                        theme::chip(ui, "DSP", prism.palette.accent);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if ch.kind == AudioChannelKind::App
                            && theme::icon_button(ui, Icon::Trash, "Remover canal").clicked()
                        {
                            remove_ch = Some(ch.id.clone());
                        }
                        if theme::icon_button(
                            ui,
                            Icon::Sliders,
                            "Filtros DSP, EQ 3 bandas, compressor e VST3/CLAP",
                        )
                        .clicked()
                        {
                            open_filters = Some(ch.id.clone());
                        }
                        let mute = theme::icon_button_ex(
                            ui,
                            if ch.muted {
                                Icon::VolumeMute
                            } else {
                                Icon::Volume
                            },
                            if ch.muted {
                                "Ativar canal"
                            } else {
                                "Silenciar canal"
                            },
                            ch.muted,
                            24.0,
                        );
                        if mute.clicked() {
                            ch.muted = !ch.muted;
                            dirty = true;
                        }
                        ui.add_space(4.0);
                        ui.label(theme::num_text(
                            ui,
                            format!("{:+.1} dB", ch.volume_db),
                            11.0,
                            FontWeight::Medium,
                            if ch.muted {
                                prism.palette.text_muted
                            } else {
                                prism.palette.text_secondary
                            },
                        ));
                    });
                });
                ui.add_space(2.0);
                theme::paint_vu_meter(ui, peak, ch.muted, avail_w - 22.0);
                let fader_accent = if ch.muted {
                    prism.palette.text_muted
                } else {
                    prism.palette.accent
                };
                if ui
                    .add(GlassSlider::new(&mut ch.volume_db, -60.0..=12.0).accent(fader_accent))
                    .changed()
                {
                    dirty = true;
                }
                if i + 1 < count {
                    ui.add_space(4.0);
                    theme::hairline(ui, &prism);
                    ui.add_space(6.0);
                }
            }
        });

    if let Some(id) = remove_ch {
        app.settings.audio_mixer.channels.retain(|c| c.id != id);
        dirty = true;
    }
    if let Some(id) = open_filters {
        app.filter_channel_id = Some(id);
    }
    if dirty {
        app.persist_audio();
    }
}

fn show_audio_filters_modal(app: &mut PrismCastNativeApp, ctx: &Context, ch_id: &str) {
    let prism = ctx.prism_theme();
    let mut open = true;
    let mut dirty = false;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }

    let screen_rect = ctx.content_rect();
    egui::Area::new(egui::Id::unique("audio_dsp_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let (_, resp) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
            let scrim = match prism.mode {
                ThemeMode::Dark => Color32::from_black_alpha(128),
                ThemeMode::Light => Color32::from_rgba_unmultiplied(15, 18, 26, 76),
            };
            ui.painter().rect_filled(screen_rect, 0.0, scrim);
            if resp.clicked() {
                open = false;
            }
        });

    egui::Window::new("audio_dsp_modal")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .order(egui::Order::Foreground)
        .default_width(430.0)
        .frame(Frame::NONE.inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            paint_glass_surface(
                ui.painter(),
                ui.max_rect().expand(18.0),
                &prism.surface(GlassElevation::Floating),
            );

            let Some(ch) = app
                .settings
                .audio_mixer
                .channels
                .iter_mut()
                .find(|c| c.id == ch_id)
            else {
                ui.label("Canal não encontrado.");
                return;
            };

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&ch.name)
                                .size(16.5)
                                .semibold()
                                .color(prism.palette.text_primary),
                        );
                        ui.add(GlassBadge::new("48 kHz DSP").variant(BadgeVariant::Accent));
                    });
                    ui.label(
                        RichText::new(
                            "Filtros em tempo real · Passa-alta, Gate, Compressor e EQ 3 bandas",
                        )
                        .size(11.0)
                        .color(prism.palette.text_muted),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button_ex(
                        ui,
                        crate::native_ui::icons::Icon::Close,
                        "Fechar (Esc)",
                        false,
                        30.0,
                    )
                    .clicked()
                    {
                        open = false;
                    }
                });
            });
            ui.add_space(10.0);

            egui::ScrollArea::vertical()
                .max_height(460.0)
                .show(ui, |ui| {
                    // Pré-estágio & Roteamento
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(12))
                        .fill_width(true)
                        .show(ui, |ui| {
                            theme::section_heading(ui, "ROTEAMENTO & GANHO", &prism);
                            ui.add_space(6.0);

                            if ch.kind == AudioChannelKind::App {
                                let mut exe = ch.app_exe.clone().unwrap_or_default();
                                ui.horizontal(|ui| {
                                    ui.label("Executável:");
                                    if ui.text_edit_singleline(&mut exe).changed() {
                                        ch.app_exe = Some(exe);
                                        dirty = true;
                                    }
                                });
                                ui.add_space(4.0);
                            }

                            ui.horizontal(|ui| {
                                ui.label("Monitoramento:");
                                egui::ComboBox::from_id_salt("audio_monitor_cb")
                                    .selected_text(match ch.monitor {
                                        AudioMonitorMode::Off => "Desligado",
                                        AudioMonitorMode::Monitor => "Monitorar e enviar",
                                        AudioMonitorMode::MonitorOnly => "Apenas monitorar",
                                    })
                                    .show_ui(ui, |ui| {
                                        for (m, lbl) in [
                                            (AudioMonitorMode::Off, "Desligado"),
                                            (AudioMonitorMode::Monitor, "Monitorar e enviar"),
                                            (AudioMonitorMode::MonitorOnly, "Apenas monitorar"),
                                        ] {
                                            if ui
                                                .selectable_value(&mut ch.monitor, m, lbl)
                                                .clicked()
                                            {
                                                dirty = true;
                                            }
                                        }
                                    });
                            });
                            ui.add_space(6.0);

                            if ui
                                .add(
                                    GlassSlider::new(&mut ch.filters.highpass_hz, 0.0..=300.0)
                                        .label("Passa-alta")
                                        .suffix(" Hz")
                                        .decimals(0),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                            if ui
                                .add(
                                    GlassSlider::new(&mut ch.filters.gain_db, -24.0..=24.0)
                                        .label("Ganho")
                                        .suffix(" dB")
                                        .decimals(1),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                            let mut sync_f = ch.sync_offset_ms as f32;
                            if ui
                                .add(
                                    GlassSlider::new(&mut sync_f, -500.0..=500.0)
                                        .label("Atraso de sincronia")
                                        .suffix(" ms")
                                        .decimals(0),
                                )
                                .changed()
                            {
                                ch.sync_offset_ms = sync_f.round() as i32;
                                dirty = true;
                            }
                        });

                    ui.add_space(8.0);

                    // Dinâmica: Gate, Compressor e Limitador
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(12))
                        .fill_width(true)
                        .show(ui, |ui| {
                            theme::section_heading(ui, "DINÂMICA", &prism);
                            ui.add_space(6.0);

                            if ui
                                .add(
                                    GlassToggle::new(&mut ch.filters.gate.enabled)
                                        .label("Noise Gate"),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                            if ch.filters.gate.enabled
                                && ui
                                    .add(
                                        GlassSlider::new(
                                            &mut ch.filters.gate.threshold_db,
                                            -80.0..=0.0,
                                        )
                                        .label("Limiar Gate")
                                        .suffix(" dB")
                                        .decimals(1),
                                    )
                                    .changed()
                            {
                                dirty = true;
                            }

                            ui.add_space(4.0);
                            if ui
                                .add(
                                    GlassToggle::new(&mut ch.filters.compressor.enabled)
                                        .label("Compressor"),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                            if ch.filters.compressor.enabled {
                                if ui
                                    .add(
                                        GlassSlider::new(
                                            &mut ch.filters.compressor.threshold_db,
                                            -60.0..=0.0,
                                        )
                                        .label("Threshold")
                                        .suffix(" dB")
                                        .decimals(1),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                                if ui
                                    .add(
                                        GlassSlider::new(
                                            &mut ch.filters.compressor.ratio,
                                            1.0..=16.0,
                                        )
                                        .label("Ratio")
                                        .suffix(":1")
                                        .decimals(1),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                            }

                            ui.add_space(4.0);
                            if ui
                                .add(
                                    GlassToggle::new(&mut ch.filters.limiter.enabled)
                                        .label("Limitador de pico"),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                        });

                    ui.add_space(8.0);

                    // Equalizador 3 bandas
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(12))
                        .fill_width(true)
                        .show(ui, |ui| {
                            theme::section_heading(ui, "EQUALIZADOR 3 BANDAS", &prism);
                            ui.add_space(6.0);
                            if ui
                                .add(
                                    GlassToggle::new(&mut ch.filters.eq_enabled)
                                        .label("Ativar EQ paramétrico"),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                            if ch.filters.eq_enabled {
                                ui.add_space(4.0);
                                if ui
                                    .add(
                                        GlassSlider::new(&mut ch.filters.eq_low_db, -12.0..=12.0)
                                            .label("Graves")
                                            .suffix(" dB")
                                            .decimals(1),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                                if ui
                                    .add(
                                        GlassSlider::new(&mut ch.filters.eq_mid_db, -12.0..=12.0)
                                            .label("Médios")
                                            .suffix(" dB")
                                            .decimals(1),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                                if ui
                                    .add(
                                        GlassSlider::new(&mut ch.filters.eq_high_db, -12.0..=12.0)
                                            .label("Agudos")
                                            .suffix(" dB")
                                            .decimals(1),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                            }
                        });
                });
        });

    if dirty {
        app.persist_audio();
    }
    if !open {
        app.filter_channel_id = None;
    }
}

// ------------------------------------------------------------------ Sidebar Direita: Propriedades

fn show_properties_panel(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    let Some(sel_id) = app.selected_item_id.clone() else {
        ui.add_space(24.0);
        theme::empty_state(
            ui,
            Icon::Layers,
            "Nenhuma fonte selecionada",
            "Clique em uma fonte no palco ou na lista de Fontes\npara editar posição, recorte e captura.",
        );
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - 190.0) * 0.5).max(0.0));
                if theme::button(
                    ui,
                    "Tela",
                    BtnOpts::new(Btn::Accent).icon(Icon::Monitor).small(),
                )
                .clicked()
                {
                    app.picker
                        .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
                }
                if theme::button(
                    ui,
                    "Janela",
                    BtnOpts::new(Btn::Secondary).icon(Icon::Window).small(),
                )
                .clicked()
                {
                    app.picker
                        .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
                }
            });
        });
        return;
    };

    let vertical = app.edit_vertical;
    let Some(item_snapshot) = app
        .editing_scene()
        .and_then(|sc| sc.items.iter().find(|i| i.id == sel_id))
        .cloned()
    else {
        return;
    };

    let status = capture_key_for_item(&item_snapshot)
        .and_then(|k| app.capture_statuses.get(&k))
        .copied();

    let mut dirty = false;
    let mut open_picker = false;
    let mut pick_image = false;
    let mut add_window_audio = false;

    egui::ScrollArea::vertical()
        .id_salt("props_scroll")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(
                    ui,
                    source_icon(item_snapshot.source_type),
                    18.0,
                    prism.palette.accent,
                );
                ui.label(theme::text(
                    ui,
                    truncate(&item_snapshot.name, 24),
                    15.0,
                    FontWeight::Semibold,
                    prism.palette.text_primary,
                ));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::chip(
                        ui,
                        &source_type_badge(item_snapshot.source_type).to_uppercase(),
                        prism.palette.text_secondary,
                    );
                });
            });

            ui.add_space(8.0);

            // Seção específica por tipo de fonte
            if matches!(
                item_snapshot.source_type,
                SourceType::ScreenCapture | SourceType::WindowCapture
            ) {
                GlassCard::new()
                    .elevation(GlassElevation::Subtle)
                    .padding(Margin::same(11))
                    .fill_width(true)
                    .show(ui, |ui| {
                        let (badge_txt, badge_var) = match status {
                            Some(CaptureStatus::Ok) => {
                                ("Capturando nativamente", BadgeVariant::Success)
                            }
                            Some(CaptureStatus::Missing) => {
                                ("Janela fechada", BadgeVariant::Warning)
                            }
                            Some(CaptureStatus::Error) => ("Erro na captura", BadgeVariant::Warm),
                            _ => ("Conectando captura…", BadgeVariant::Neutral),
                        };
                        ui.add(GlassBadge::new(badge_txt).variant(badge_var).dot(true));
                        ui.add_space(8.0);
                        if theme::button(
                            ui,
                            "Trocar tela ou janela",
                            BtnOpts::new(Btn::Secondary)
                                .icon(Icon::Monitor)
                                .small()
                                .min_width(ui.available_width()),
                        )
                        .clicked()
                        {
                            open_picker = true;
                        }
                        if item_snapshot.source_type == SourceType::WindowCapture {
                            ui.add_space(6.0);
                            let has_audio = item_snapshot.settings.audio_channel_id.is_some();
                            if theme::button(
                                ui,
                                if has_audio {
                                    "Áudio desta janela vinculado"
                                } else {
                                    "Capturar o áudio desta janela"
                                },
                                BtnOpts::new(Btn::Ghost)
                                    .icon(if has_audio { Icon::Check } else { Icon::Volume })
                                    .selected(has_audio)
                                    .enabled(!has_audio)
                                    .small()
                                    .min_width(ui.available_width()),
                            )
                            .clicked()
                            {
                                add_window_audio = true;
                            }
                        }
                    });
            } else if item_snapshot.source_type == SourceType::ImageSource {
                GlassCard::new()
                    .elevation(GlassElevation::Subtle)
                    .padding(Margin::same(11))
                    .fill_width(true)
                    .show(ui, |ui| {
                        let cur = item_snapshot
                            .settings
                            .path
                            .as_deref()
                            .unwrap_or("Nenhuma imagem");
                        ui.label(
                            RichText::new(cur)
                                .size(11.0)
                                .color(prism.palette.text_secondary),
                        );
                        ui.add_space(6.0);
                        if theme::button(
                            ui,
                            "Trocar arquivo de imagem…",
                            BtnOpts::new(Btn::Secondary).icon(Icon::Folder).small(),
                        )
                        .clicked()
                        {
                            pick_image = true;
                        }
                    });
            }

            ui.add_space(8.0);

            if let Some(sc) = app.editing_scene_mut() {
                if let Some(it) = sc.items.iter_mut().find(|i| i.id == sel_id) {
                    if it.source_type == SourceType::ColorSource {
                        GlassCard::new()
                            .elevation(GlassElevation::Subtle)
                            .padding(Margin::same(10))
                            .fill_width(true)
                            .show(ui, |ui| {
                                let mut hex = it
                                    .settings
                                    .color
                                    .clone()
                                    .unwrap_or_else(|| "#1c1c19".to_string());
                                ui.horizontal(|ui| {
                                    ui.label("Cor HEX:");
                                    if ui.text_edit_singleline(&mut hex).changed() {
                                        it.settings.color = Some(hex);
                                        dirty = true;
                                    }
                                });
                            });
                        ui.add_space(8.0);
                    }

                    if it.source_type == SourceType::BrowserSource {
                        GlassCard::new()
                            .elevation(GlassElevation::Subtle)
                            .padding(Margin::same(10))
                            .fill_width(true)
                            .show(ui, |ui| {
                                let mut url = it.settings.url.clone().unwrap_or_default();
                                ui.label("URL do Widget:");
                                if ui.text_edit_singleline(&mut url).changed() {
                                    it.settings.url = Some(url);
                                    dirty = true;
                                }
                            });
                        ui.add_space(8.0);
                    }

                    let tf = if vertical {
                        &mut it.vertical_transform
                    } else {
                        &mut it.transform
                    };
                    let (cw, ch) = if vertical {
                        (1080.0, 1920.0)
                    } else {
                        (1920.0, 1080.0)
                    };

                    // Cartão de Posição & Dimensões
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(11))
                        .fill_width(true)
                        .show(ui, |ui| {
                            theme::section_heading(
                                ui,
                                if vertical {
                                    "POSIÇÃO NO 9:16"
                                } else {
                                    "POSIÇÃO NO 16:9"
                                },
                                &prism,
                            );
                            ui.add_space(6.0);

                            let mut px_x = (tf.x * cw).round() as i32;
                            let mut px_y = (tf.y * ch).round() as i32;
                            let mut px_w = (tf.width * cw).round() as i32;
                            let mut px_h = (tf.height * ch).round() as i32;

                            egui::Grid::new("tf_grid")
                                .num_columns(4)
                                .spacing(Vec2::new(6.0, 6.0))
                                .show(ui, |ui| {
                                    ui.label("X");
                                    if ui.add(egui::DragValue::new(&mut px_x).speed(2)).changed() {
                                        tf.x = px_x as f32 / cw;
                                        dirty = true;
                                    }
                                    ui.label("Y");
                                    if ui.add(egui::DragValue::new(&mut px_y).speed(2)).changed() {
                                        tf.y = px_y as f32 / ch;
                                        dirty = true;
                                    }
                                    ui.end_row();
                                    ui.label("L");
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut px_w)
                                                .range(32..=7680)
                                                .speed(4),
                                        )
                                        .changed()
                                    {
                                        tf.width = px_w as f32 / cw;
                                        dirty = true;
                                    }
                                    ui.label("A");
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut px_h)
                                                .range(32..=7680)
                                                .speed(4),
                                        )
                                        .changed()
                                    {
                                        tf.height = px_h as f32 / ch;
                                        dirty = true;
                                    }
                                    ui.end_row();
                                });

                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if theme::button(
                                    ui,
                                    "Tela cheia",
                                    BtnOpts::new(Btn::Secondary).small(),
                                )
                                .clicked()
                                {
                                    tf.x = 0.0;
                                    tf.y = 0.0;
                                    tf.width = 1.0;
                                    tf.height = 1.0;
                                    dirty = true;
                                }
                                if theme::button(
                                    ui,
                                    "Centralizar",
                                    BtnOpts::new(Btn::Secondary).small(),
                                )
                                .clicked()
                                {
                                    tf.x = (1.0 - tf.width) * 0.5;
                                    tf.y = (1.0 - tf.height) * 0.5;
                                    dirty = true;
                                }
                            });
                        });

                    ui.add_space(8.0);

                    // Cartão de Recorte
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(11))
                        .fill_width(true)
                        .show(ui, |ui| {
                            theme::section_heading(ui, "RECORTE (%) · ALT + ARRASTAR", &prism);
                            ui.add_space(6.0);
                            let mut ct = (tf.crop_top.unwrap_or(0.0) * 100.0).round();
                            let mut cb = (tf.crop_bottom.unwrap_or(0.0) * 100.0).round();
                            let mut cl = (tf.crop_left.unwrap_or(0.0) * 100.0).round();
                            let mut cr = (tf.crop_right.unwrap_or(0.0) * 100.0).round();

                            egui::Grid::new("crop_grid")
                                .num_columns(4)
                                .spacing(Vec2::new(6.0, 4.0))
                                .show(ui, |ui| {
                                    ui.label("Topo");
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut ct)
                                                .range(0.0..=85.0)
                                                .suffix("%"),
                                        )
                                        .changed()
                                    {
                                        tf.crop_top = Some(ct / 100.0);
                                        dirty = true;
                                    }
                                    ui.label("Base");
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut cb)
                                                .range(0.0..=85.0)
                                                .suffix("%"),
                                        )
                                        .changed()
                                    {
                                        tf.crop_bottom = Some(cb / 100.0);
                                        dirty = true;
                                    }
                                    ui.end_row();
                                    ui.label("Esq");
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut cl)
                                                .range(0.0..=85.0)
                                                .suffix("%"),
                                        )
                                        .changed()
                                    {
                                        tf.crop_left = Some(cl / 100.0);
                                        dirty = true;
                                    }
                                    ui.label("Dir");
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut cr)
                                                .range(0.0..=85.0)
                                                .suffix("%"),
                                        )
                                        .changed()
                                    {
                                        tf.crop_right = Some(cr / 100.0);
                                        dirty = true;
                                    }
                                    ui.end_row();
                                });
                        });

                    ui.add_space(8.0);

                    // Cartão de Estilo Visual da Fonte
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(11))
                        .fill_width(true)
                        .show(ui, |ui| {
                            theme::section_heading(ui, "ESTILO DA CAMADA", &prism);
                            ui.add_space(6.0);
                            let mut op_pct = tf.opacity * 100.0;
                            if ui
                                .add(
                                    GlassSlider::new(&mut op_pct, 0.0..=100.0)
                                        .label("Opacidade")
                                        .suffix("%")
                                        .decimals(0),
                                )
                                .changed()
                            {
                                tf.opacity = (op_pct / 100.0).clamp(0.0, 1.0);
                                dirty = true;
                            }
                            ui.add_space(4.0);
                            if ui
                                .add(GlassToggle::new(&mut tf.flip_x).label("Espelhar horizontal"))
                                .changed()
                            {
                                dirty = true;
                            }
                            ui.add_space(4.0);
                            let mut rounded = it.settings.rounded.unwrap_or(false);
                            if ui
                                .add(GlassToggle::new(&mut rounded).label("Cantos arredondados"))
                                .changed()
                            {
                                it.settings.rounded = Some(rounded);
                                dirty = true;
                            }
                        });
                }
            }
        });

    if open_picker {
        app.apply_stage_actions(vec![StageAction::OpenPickerForItem(sel_id.clone())]);
    }
    if pick_image {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Imagens", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
            .pick_file()
        {
            if let Some(sc) = app.editing_scene_mut() {
                if let Some(it) = sc.items.iter_mut().find(|i| i.id == sel_id) {
                    it.settings.path = Some(path.to_string_lossy().to_string());
                    dirty = true;
                }
            }
        }
    }
    if add_window_audio {
        let ch_id = ensure_window_audio_channel(&mut app.settings, &item_snapshot.name);
        if let Some(sc) = app.editing_scene_mut() {
            if let Some(it) = sc.items.iter_mut().find(|i| i.id == sel_id) {
                it.settings.audio_channel_id = Some(ch_id);
            }
        }
        app.persist_audio();
        dirty = true;
        app.notify("Canal de áudio isolado criado para esta janela.", false);
    }
    if dirty {
        app.persist_scenes();
    }
}

// ------------------------------------------------------------------ Sidebar Direita: Multichat

fn show_chat_panel(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    theme::dock_header(ui, "Multichat ao vivo", None, &prism, |ui| {
        if theme::icon_button(ui, Icon::Refresh, "Reconectar Twitch, YouTube e Kick").clicked() {
            let engine = app.engine.clone();
            app.rt.spawn(async move {
                let _ = engine
                    .handle_command("multichat_reconnect", Value::Null)
                    .await;
            });
        }
    });

    let feed_h = (ui.available_height() - 46.0).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("multichat_feed")
        .stick_to_bottom(true)
        .max_height(feed_h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.chat_messages.is_empty() {
                ui.add_space(24.0);
                theme::empty_state(
                    ui,
                    Icon::Chat,
                    "O chat aparece aqui",
                    "Conecte Twitch, YouTube ou Kick em Destinos\npara acompanhar o chat unificado.",
                );
                ui.vertical_centered(|ui| {
                    if theme::button(ui, "Abrir Destinos", BtnOpts::new(Btn::Secondary).icon(Icon::Broadcast).small())
                        .clicked()
                    {
                        app.active_tab = NavTab::Destinations;
                    }
                });
            }
            for msg in &app.chat_messages {
                let (plat_lbl, plat_col) = match msg.platform {
                    ChatPlatform::Twitch => ("TW", theme::TWITCH),
                    ChatPlatform::Youtube => ("YT", theme::YOUTUBE),
                    ChatPlatform::Kick => ("KK", theme::KICK),
                };
                let text_body: String = msg
                    .fragments
                    .iter()
                    .map(|f| match f {
                        ChatFragment::Text { text } => text.as_str(),
                        ChatFragment::Emote { name, .. } => name.as_str(),
                    })
                    .collect();

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    theme::chip(ui, plat_lbl, plat_col);
                    ui.label(theme::text(ui, &msg.name, 12.5, FontWeight::Semibold, prism.palette.text_primary));
                    ui.label(theme::text(ui, text_body, 12.5, FontWeight::Regular, prism.palette.text_secondary));
                });
                ui.add_space(5.0);
            }
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.chat_input)
                .desired_width((ui.available_width() - 40.0).max(80.0))
                .margin(Margin::symmetric(10, 7))
                .hint_text("Enviar no chat da Twitch…"),
        );
        let send = (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            || theme::button(
                ui,
                "",
                BtnOpts::new(Btn::Accent).icon(Icon::Send).height(32.0),
            )
            .on_hover_text("Enviar (Enter)")
            .clicked();
        if send && !app.chat_input.trim().is_empty() {
            let text = app.chat_input.trim().to_string();
            app.chat_input.clear();
            let engine = app.engine.clone();
            let tx = app.bg_tx.clone();
            app.rt.spawn(async move {
                let res = engine
                    .handle_command("twitch_send_chat", json!({ "message": text }))
                    .await;
                if res.get("ok").and_then(|v| v.as_bool()) == Some(false) {
                    let err = res
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Conecte a Twitch em Destinos para enviar mensagens.");
                    let _ = tx.send(BgUiMsg::Toast {
                        message: err.to_string(),
                        is_error: true,
                    });
                }
            });
        }
    });
}

// ------------------------------------------------------------------ Tela: Destinos

pub fn show_destinations_screen(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(page_margin(ui)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Destinos de Transmissão")
                            .size(21.0)
                            .semibold()
                            .color(prism.palette.text_primary),
                    );
                    let active_count = app
                        .settings
                        .destinations
                        .iter()
                        .filter(|d| d.enabled)
                        .count();
                    ui.add(
                        GlassBadge::new(&format!("{active_count} ativos"))
                            .variant(if active_count > 0 {
                                BadgeVariant::Success
                            } else {
                                BadgeVariant::Neutral
                            })
                            .dot(active_count > 0),
                    );
                });
                ui.label(
                    RichText::new(
                        "Transmita simultaneamente em 16:9 e 9:16 com chaves protegidas no cofre nativo do sistema.",
                    )
                    .size(12.5)
                    .color(prism.palette.text_secondary),
                );
                ui.add_space(14.0);

                // Card OAuth Device Flow da Twitch
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Conta Twitch (OAuth Device Code)")
                                            .size(15.0)
                                            .semibold()
                                            .color(prism.palette.text_primary),
                                    );
                                    if app.twitch_caps.linked {
                                        ui.add(
                                            GlassBadge::new(&format!(
                                                "@{}",
                                                app.twitch_caps
                                                    .login
                                                    .as_deref()
                                                    .unwrap_or("twitch")
                                            ))
                                            .variant(BadgeVariant::Success)
                                            .dot(true),
                                        );
                                    }
                                });
                                ui.label(
                                    RichText::new(if app.twitch_caps.linked {
                                        "Conta vinculada — chat integrado e edição de live ativos."
                                    } else {
                                        "Conecte para atualizar título/categoria e enviar mensagens no Multichat."
                                    })
                                    .size(12.0)
                                    .color(prism.palette.text_secondary),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if !app.twitch_caps.linked
                                    && theme::tally_button(ui, "Entrar com a Twitch").clicked()
                                {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let res = engine
                                            .handle_command(
                                                "dest_twitch_start",
                                                json!({ "destinationId": "twitch" }),
                                            )
                                            .await;
                                        if let Ok(st) =
                                            serde_json::from_value::<TwitchLoginState>(res)
                                        {
                                            let _ = tx.send(BgUiMsg::TwitchLogin(st));
                                        }
                                    });
                                }
                            });
                        });

                        if let TwitchLoginState::Code {
                            user_code,
                            verification_uri,
                            ..
                        } = &app.twitch_login
                        {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.label("Código:");
                                ui.label(
                                    RichText::new(user_code)
                                        .monospace()
                                        .size(16.0)
                                        .semibold()
                                        .color(theme::SPEC_AMBER),
                                );
                                if ui
                                    .add(
                                        GlassButton::accent("Abrir twitch.tv/activate")
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 28.0)),
                                    )
                                    .clicked()
                                {
                                    let _ = open::that(verification_uri);
                                }
                            });
                        }
                    });

                ui.add_space(12.0);

                // Cards dos destinos (Twitch, YouTube, Kick, TikTok, Custom)
                let mut dirty = false;
                let mut save_key_req: Option<(String, String)> = None;

                for dest in &mut app.settings.destinations {
                    GlassCard::new()
                        .elevation(GlassElevation::Elevated)
                        .padding(Margin::same(15))
                        .fill_width(true)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.add(GlassToggle::new(&mut dest.enabled)).changed() {
                                    dirty = true;
                                }
                                platform_mark(ui, dest.platform);
                                ui.label(theme::text(ui, &dest.name, 15.0, FontWeight::Semibold, prism.palette.text_primary));
                                ui.add_space(6.0);
                                let mut orient_idx = usize::from(dest.orientation == CanvasOrientation::Vertical);
                                if ui
                                    .add(GlassSegmented::new(&mut orient_idx, &["16:9", "9:16"]).height(26.0))
                                    .on_hover_text("Canvas enviado para este destino")
                                    .changed()
                                {
                                    dest.orientation = if orient_idx == 1 {
                                        CanvasOrientation::Vertical
                                    } else {
                                        CanvasOrientation::Horizontal
                                    };
                                    dirty = true;
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add(
                                        GlassBadge::new(if dest.has_key {
                                            "Chave no cofre"
                                        } else {
                                            "Sem chave"
                                        })
                                        .variant(if dest.has_key {
                                            BadgeVariant::Success
                                        } else {
                                            BadgeVariant::Neutral
                                        })
                                        .dot(dest.has_key),
                                    );
                                });
                            });

                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label("Servidor:");
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut dest.server)
                                            .desired_width(320.0),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                                ui.add_space(8.0);
                                ui.label("Bitrate:");
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut dest.bitrate_kbps)
                                            .range(1000..=20000)
                                            .speed(250)
                                            .suffix(" kbps"),
                                    )
                                    .changed()
                                {
                                    dirty = true;
                                }
                            });

                            ui.add_space(6.0);
                            let key_buf = app.dest_key_inputs.entry(dest.id.clone()).or_default();
                            ui.horizontal(|ui| {
                                ui.label("Chave RTMP:");
                                ui.add(
                                    egui::TextEdit::singleline(key_buf)
                                        .password(true)
                                        .desired_width(290.0)
                                        .hint_text(if dest.has_key {
                                            "•••••••••••• (substituir)"
                                        } else {
                                            "Cole sua stream key aqui"
                                        }),
                                );
                                if ui
                                    .add(
                                        GlassButton::secondary("Salvar no cofre")
                                            .pill(true)
                                            .enabled(!key_buf.trim().is_empty())
                                            .min_size(Vec2::new(0.0, 28.0)),
                                    )
                                    .clicked()
                                {
                                    save_key_req =
                                        Some((dest.id.clone(), key_buf.trim().to_string()));
                                    key_buf.clear();
                                }
                            });
                        });
                    ui.add_space(10.0);
                }

                // Adicionar RTMP customizado
                GlassCard::new()
                    .elevation(GlassElevation::Subtle)
                    .padding(Margin::same(14))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("+ Novo destino RTMP")
                                    .semibold()
                                    .color(prism.palette.text_primary),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut app.new_dest_name)
                                    .desired_width(150.0)
                                    .hint_text("Nome"),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut app.new_dest_server)
                                    .desired_width(260.0)
                                    .hint_text("rtmp://servidor/app"),
                            );
                            if ui
                                .add(
                                    GlassButton::accent("Adicionar")
                                        .pill(true)
                                        .enabled(!app.new_dest_name.trim().is_empty())
                                        .min_size(Vec2::new(0.0, 28.0)),
                                )
                                .clicked()
                            {
                                app.settings.destinations.push(Destination {
                                    id: format!("custom-{}", Uuid::new_v4()),
                                    platform: PlatformId::Custom,
                                    name: app.new_dest_name.trim().to_string(),
                                    enabled: false,
                                    server: app.new_dest_server.trim().to_string(),
                                    has_key: false,
                                    orientation: CanvasOrientation::Horizontal,
                                    bitrate_kbps: 6000,
                                    account: None,
                                });
                                app.new_dest_name.clear();
                                app.new_dest_server = "rtmp://".to_string();
                                dirty = true;
                            }
                        });
                    });

                if dirty {
                    app.persist_destinations();
                }

                if let Some((dest_id, key)) = save_key_req {
                    let engine = app.engine.clone();
                    let tx = app.bg_tx.clone();
                    app.rt.spawn(async move {
                        let res = engine
                            .handle_command(
                                "dest_set_key",
                                json!({ "destinationId": dest_id, "key": key }),
                            )
                            .await;
                        let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                        let snap = engine.store.snapshot();
                        let _ = tx.send(BgUiMsg::SettingsRefreshed(snap));
                        let _ = tx.send(BgUiMsg::Toast {
                            message: if ok {
                                "Chave salva com segurança no cofre nativo.".to_string()
                            } else {
                                res.get("error")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Chave inválida.")
                                    .to_string()
                            },
                            is_error: !ok,
                        });
                    });
                }
            });
        });
}

// ------------------------------------------------------------------ Tela: Widgets & Kit

pub fn show_widgets_screen(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(page_margin(ui)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(
                    RichText::new("Widgets & Kit de Overlays")
                        .size(21.0)
                        .semibold()
                        .color(prism.palette.text_primary),
                );
                ui.label(
                    RichText::new(
                        "Carregue sua pasta de overlays HTML com parsing estático seguro em Rust (sem eval).",
                    )
                    .size(12.5)
                    .color(prism.palette.text_secondary),
                );
                ui.add_space(14.0);

                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let cur = app
                                .settings
                                .kit_path
                                .as_deref()
                                .unwrap_or("Nenhuma pasta selecionada");
                            ui.vertical(|ui| {
                                theme::section_heading(ui, "PASTA DO KIT", &prism);
                                ui.label(
                                    RichText::new(cur)
                                        .monospace()
                                        .size(12.0)
                                        .color(prism.palette.text_secondary),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if theme::button(ui, "Selecionar pasta do kit…", BtnOpts::new(Btn::Accent).icon(Icon::Folder)).clicked() {
                                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                        let p = folder.to_string_lossy().to_string();
                                        app.settings.kit_path = Some(p.clone());
                                        app.persist_settings(json!({ "kitPath": p }));
                                        app.load_kit_async(p);
                                    }
                                }
                            });
                        });
                    });

                ui.add_space(14.0);
                if let Some(ref kit) = app.loaded_kit {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&kit.name)
                                .size(16.0)
                                .semibold()
                                .color(prism.palette.text_primary),
                        );
                        ui.add(
                            GlassBadge::new(&format!("{} widgets", kit.widgets.len()))
                                .variant(BadgeVariant::Accent),
                        );
                    });
                    ui.add_space(8.0);
                    for w in &kit.widgets {
                        GlassCard::new()
                            .elevation(GlassElevation::Subtle)
                            .padding(Margin::same(13))
                            .fill_width(true)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(&w.name)
                                                .semibold()
                                                .color(prism.palette.text_primary),
                                        );
                                        ui.label(
                                            RichText::new(&w.description)
                                                .size(11.8)
                                                .color(prism.palette.text_secondary),
                                        );
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.add(
                                            GlassBadge::new(&format!("{}×{}", w.width, w.height))
                                                .variant(BadgeVariant::Neutral),
                                        );
                                    });
                                });
                            });
                        ui.add_space(6.0);
                    }
                } else {
                    GlassCard::new()
                        .elevation(GlassElevation::Subtle)
                        .padding(Margin::same(18))
                        .fill_width(true)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Selecione uma pasta de kit para listar overlays, alertas e telas de pausa.")
                                    .color(prism.palette.text_secondary),
                            );
                        });
                }
            });
        });
}

// ------------------------------------------------------------------ Tela: Rede Local (LAN P2P)

pub fn show_lan_screen(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(page_margin(ui)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Rede Local (LAN P2P)")
                            .size(21.0)
                            .semibold()
                            .color(prism.palette.text_primary),
                    );
                    if app.lan_hosting {
                        ui.add(
                            GlassBadge::new("HOST ATIVO")
                                .variant(BadgeVariant::Success)
                                .dot(true),
                        );
                    }
                });
                ui.label(
                    RichText::new(
                        "Compartilhe a saída do estúdio em ultra-baixa latência na rede local com descoberta UDP e PIN.",
                    )
                    .size(12.5)
                    .color(prism.palette.text_secondary),
                );
                ui.add_space(14.0);

                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Transmitir na rede local (Host)")
                                        .size(15.0)
                                        .semibold()
                                        .color(prism.palette.text_primary),
                                );
                                if app.lan_hosting {
                                    ui.label(
                                        RichText::new(format!(
                                            "Ativo em {}:{} · PIN: {}",
                                            app.lan_ip,
                                            app.lan_port,
                                            app.lan_pin.as_deref().unwrap_or("livre")
                                        ))
                                        .monospace()
                                        .color(prism.palette.accent),
                                    );
                                } else {
                                    ui.label(
                                        RichText::new("Parado")
                                            .color(prism.palette.text_secondary),
                                    );
                                }
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if app.lan_hosting {
                                    if ui
                                        .add(
                                            GlassButton::secondary("Parar Host LAN")
                                                .pill(true)
                                                .min_size(Vec2::new(0.0, 32.0)),
                                        )
                                        .clicked()
                                    {
                                        let engine = app.engine.clone();
                                        let tx = app.bg_tx.clone();
                                        app.rt.spawn(async move {
                                            let _ = engine
                                                .handle_command("host_stop", Value::Null)
                                                .await;
                                            let _ = tx.send(BgUiMsg::LanHostStopped);
                                        });
                                    }
                                } else if theme::tally_button(ui, "Iniciar Host LAN").clicked() {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    let req_pin = app.settings.require_pin;
                                    let host_name = app.settings.host_name.clone();
                                    app.rt.spawn(async move {
                                        let res = engine
                                            .handle_command(
                                                "host_start",
                                                json!({
                                                    "hostName": host_name,
                                                    "requirePin": req_pin,
                                                    "sourceName": "PrismCast Studio",
                                                    "sourceKind": "screen",
                                                }),
                                            )
                                            .await;
                                        if res.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                                            let ip = res
                                                .get("ip")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("127.0.0.1")
                                                .to_string();
                                            let port = res
                                                .get("port")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(41235)
                                                as u16;
                                            let pin = res
                                                .get("pin")
                                                .and_then(|v| v.as_str())
                                                .map(str::to_string);
                                            let _ =
                                                tx.send(BgUiMsg::LanHostStarted { ip, port, pin });
                                        }
                                    });
                                }
                            });
                        });

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    GlassToggle::new(&mut app.settings.require_pin)
                                        .label("Exigir código PIN (6 dígitos)"),
                                )
                                .changed()
                            {
                                app.persist_settings(
                                    json!({ "requirePin": app.settings.require_pin }),
                                );
                            }
                            ui.add_space(12.0);
                            if ui
                                .add(
                                    GlassButton::secondary("Autorizar Firewall")
                                        .pill(true)
                                        .min_size(Vec2::new(0.0, 28.0)),
                                )
                                .clicked()
                            {
                                let engine = app.engine.clone();
                                let tx = app.bg_tx.clone();
                                app.rt.spawn(async move {
                                    let res = engine
                                        .handle_command("firewall_authorize", Value::Null)
                                        .await;
                                    let ok =
                                        res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                                    let _ = tx.send(BgUiMsg::FirewallChecked(ok));
                                    let _ = tx.send(BgUiMsg::Toast {
                                        message: if ok {
                                            "Regras de firewall verificadas.".to_string()
                                        } else {
                                            "Não foi possível autorizar o firewall automaticamente."
                                                .to_string()
                                        },
                                        is_error: !ok,
                                    });
                                });
                            }
                        });
                    });

                ui.add_space(12.0);
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Computadores encontrados na LAN")
                                    .size(15.0)
                                    .semibold()
                                    .color(prism.palette.text_primary),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if theme::button(
                                    ui,
                                    "Procurar na rede",
                                    BtnOpts::new(Btn::Secondary).icon(Icon::Refresh).small(),
                                )
                                .clicked()
                                {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let _ = engine
                                            .handle_command("discovery_start", Value::Null)
                                            .await;
                                        let res = engine
                                            .handle_command("discovery_refresh", Value::Null)
                                            .await;
                                        if let Ok(hosts) =
                                            serde_json::from_value::<Vec<DiscoveredHost>>(res)
                                        {
                                            let _ = tx.send(BgUiMsg::DiscoveredHosts(hosts));
                                        }
                                    });
                                }
                            });
                        });
                        ui.add_space(8.0);
                        if app.lan_hosts.is_empty() {
                            ui.label(
                                RichText::new("Nenhum host PrismCast detectado nesta rede.")
                                    .color(prism.palette.text_muted),
                            );
                        } else {
                            for h in &app.lan_hosts {
                                GlassCard::new()
                                    .elevation(GlassElevation::Subtle)
                                    .padding(Margin::symmetric(12, 8))
                                    .fill_width(true)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(&h.name)
                                                    .semibold()
                                                    .color(prism.palette.text_primary),
                                            );
                                            ui.add(
                                                GlassBadge::new(&format!("{}:{}", h.ip, h.port))
                                                    .variant(BadgeVariant::Accent),
                                            );
                                            ui.label(
                                                RichText::new(&h.source_name)
                                                    .color(prism.palette.text_secondary),
                                            );
                                        });
                                    });
                                ui.add_space(5.0);
                            }
                        }
                    });
            });
        });
}

// ------------------------------------------------------------------ Tela: Ajustes

pub fn show_settings_screen(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(page_margin(ui)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(
                    RichText::new("Ajustes do Estúdio")
                        .size(21.0)
                        .semibold()
                        .color(prism.palette.text_primary),
                );
                ui.label(
                    RichText::new(
                        "Personalize a aparência PrismGlass, o pipeline GPU Zero-Copy, a Câmera Virtual e integrações.",
                    )
                    .size(12.5)
                    .color(prism.palette.text_secondary),
                );
                ui.add_space(14.0);

                // 0. Aparência: galeria de temas Prism / clássicos, cor de destaque e material
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        let mut app_dirty = false;
                        ui.horizontal(|ui| {
                            theme::icon(ui, Icon::Swatch, 16.0, prism.palette.accent);
                            ui.label(theme::text(ui, "Aparência", 15.5, FontWeight::Semibold, prism.palette.text_primary));
                            theme::chip(ui, "EGUI-PRISMGLASS 0.3", prism.palette.text_muted);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if theme::button(ui, "Restaurar padrão", BtnOpts::new(Btn::Ghost).icon(Icon::Undo).small())
                                    .clicked()
                                {
                                    let keep_collapsed = app.settings.ui_appearance.sidebar_collapsed;
                                    app.settings.ui_appearance = UiAppearanceSettings {
                                        sidebar_collapsed: keep_collapsed,
                                        ..Default::default()
                                    };
                                    app_dirty = true;
                                }
                            });
                        });
                        ui.label(theme::text(
                            ui,
                            "Linguagem Prism: tipografia Inter, neutros calibrados e vidro reservado ao que flutua.",
                            12.0,
                            FontWeight::Regular,
                            prism.palette.text_secondary,
                        ));
                        ui.add_space(12.0);

                        let mut theme_gallery = |ui: &mut Ui, title: &str, presets: &[UiThemePreset], dirty: &mut bool| {
                            theme::section_heading(ui, title, &prism);
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
                                for &preset in presets {
                                    let active = app.settings.ui_appearance.preset == preset;
                                    if theme_card(ui, preset, app.settings.ui_appearance.accent, active).clicked() {
                                        app.settings.ui_appearance.preset = preset;
                                        *dirty = true;
                                    }
                                }
                            });
                            ui.add_space(12.0);
                        };
                        theme_gallery(ui, "Temas Prism", &UiThemePreset::PRISM, &mut app_dirty);
                        theme_gallery(ui, "Clássicos (v0.2)", &UiThemePreset::CLASSIC, &mut app_dirty);

                        theme::section_heading(ui, "Cor de destaque", &prism);
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                            for accent in UiAccent::ALL {
                                let active = app.settings.ui_appearance.accent == accent;
                                if accent_swatch_button(ui, accent, active).clicked() {
                                    app.settings.ui_appearance.accent = accent;
                                    app_dirty = true;
                                }
                            }
                        });
                        ui.add_space(14.0);

                        theme::section_heading(ui, "Material & fundo", &prism);
                        ui.add_space(6.0);
                        let is_glass = prism.surface_style != SurfaceStyle::Minimal;
                        ui.columns(2, |cols| {
                            if is_glass
                                && cols[0]
                                    .add(
                                        GlassSlider::new(&mut app.settings.ui_appearance.glass_opacity, 0.55..=1.45)
                                            .label("Densidade do vidro")
                                            .suffix("×")
                                            .decimals(2),
                                    )
                                    .changed()
                            {
                                app_dirty = true;
                            }
                            if prism.surface_style == SurfaceStyle::Glass
                                && cols[0]
                                    .add(
                                        GlassSlider::new(&mut app.settings.ui_appearance.specular_intensity, 0.0..=1.8)
                                            .label("Brilho especular")
                                            .suffix("×")
                                            .decimals(2),
                                    )
                                    .changed()
                            {
                                app_dirty = true;
                            }
                            if cols[1]
                                .add(
                                    GlassSlider::new(&mut app.settings.ui_appearance.radius_scale, 0.5..=1.4)
                                        .label("Arredondamento")
                                        .suffix("×")
                                        .decimals(2),
                                )
                                .changed()
                            {
                                app_dirty = true;
                            }
                            if is_glass
                                && cols[1]
                                    .add(
                                        GlassSlider::new(&mut app.settings.ui_appearance.backdrop_intensity, 0.0..=1.4)
                                            .label("Luz do fundo")
                                            .suffix("×")
                                            .decimals(2),
                                    )
                                    .changed()
                            {
                                app_dirty = true;
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 18.0;
                            if is_glass
                                && ui
                                    .add(GlassToggle::new(&mut app.settings.ui_appearance.animate_backdrop).label("Animar a luz do fundo"))
                                    .changed()
                            {
                                app_dirty = true;
                            }
                            if ui
                                .add(GlassToggle::new(&mut app.settings.ui_appearance.show_status_bar).label("Barra de status com telemetria"))
                                .changed()
                            {
                                app_dirty = true;
                            }
                            let mut collapsed = app.settings.ui_appearance.sidebar_collapsed;
                            if ui.add(GlassToggle::new(&mut collapsed).label("Barra lateral compacta")).changed() {
                                app.settings.ui_appearance.sidebar_collapsed = collapsed;
                                app_dirty = true;
                            }
                        });

                        if app_dirty {
                            app.settings.ui_appearance.normalize();
                            app.persist_appearance();
                        }
                    });

                ui.add_space(12.0);

                // 1. Pipeline GPU Zero-Copy & Vídeo
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Vídeo & Pipeline GPU Zero-Copy")
                                    .size(15.0)
                                    .semibold()
                                    .color(prism.palette.text_primary),
                            );
                            ui.add(
                                GlassBadge::new(&format!(
                                    "{} · {}",
                                    app.gpu_status.compositor_backend, app.gpu_status.adapter_name
                                ))
                                .variant(BadgeVariant::Accent),
                            );
                        });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("FPS do estúdio:");
                            for fps in [30u32, 60u32] {
                                if theme::glass_toggle_button(
                                    ui,
                                    &format!("{fps} FPS"),
                                    app.settings.studio.fps == fps,
                                )
                                .clicked()
                                {
                                    app.settings.studio.fps = fps;
                                    app.persist_studio();
                                }
                            }
                        });
                        ui.add_space(8.0);
                        let mut gpu_dirty = false;
                        if ui
                            .add(
                                GlassToggle::new(&mut app.settings.gpu_pipeline.zero_copy_enabled)
                                    .label("Ativar captura GPU Zero-Copy (DXGI / PipeWire)"),
                            )
                            .changed()
                        {
                            gpu_dirty = true;
                        }
                        ui.add_space(4.0);
                        if ui
                            .add(
                                GlassToggle::new(&mut app.settings.gpu_pipeline.prefer_wgpu)
                                    .label("Usar compositor acelerado por GPU (wgpu)"),
                            )
                            .changed()
                        {
                            gpu_dirty = true;
                        }
                        if gpu_dirty {
                            if let Ok(val) = serde_json::to_value(&app.settings.gpu_pipeline) {
                                app.persist_settings(json!({ "gpuPipeline": val }));
                            }
                        }
                    });

                ui.add_space(12.0);

                // 2. Câmera Virtual Nativa
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Câmera Virtual Nativa")
                                            .size(15.0)
                                            .semibold()
                                            .color(prism.palette.text_primary),
                                    );
                                    ui.add(
                                        GlassBadge::new(if app.virtual_cam.active {
                                            "Ativa"
                                        } else {
                                            "Parada"
                                        })
                                        .variant(if app.virtual_cam.active {
                                            BadgeVariant::Success
                                        } else {
                                            BadgeVariant::Neutral
                                        })
                                        .dot(app.virtual_cam.active),
                                    );
                                });
                                ui.label(
                                    RichText::new(format!(
                                        "Frames enviados: {}",
                                        app.virtual_cam.frames_sent
                                    ))
                                    .monospace()
                                    .size(11.0)
                                    .color(prism.palette.text_secondary),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if app.virtual_cam.active {
                                    if ui
                                        .add(
                                            GlassButton::secondary("Parar Câmera Virtual")
                                                .pill(true)
                                                .min_size(Vec2::new(0.0, 32.0)),
                                        )
                                        .clicked()
                                    {
                                        let engine = app.engine.clone();
                                        let tx = app.bg_tx.clone();
                                        app.rt.spawn(async move {
                                            let res = engine
                                                .handle_command("virtual_cam_stop", Value::Null)
                                                .await;
                                            if let Ok(st) = serde_json::from_value(res) {
                                                let _ = tx.send(BgUiMsg::VirtualCamStatus(st));
                                            }
                                        });
                                    }
                                } else if theme::tally_button(ui, "Iniciar Câmera Virtual")
                                    .clicked()
                                {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    let orient = app.settings.virtual_camera.orientation;
                                    app.rt.spawn(async move {
                                        let o = match orient {
                                            CanvasOrientation::Horizontal => "horizontal",
                                            CanvasOrientation::Vertical => "vertical",
                                        };
                                        let res = engine
                                            .handle_command(
                                                "virtual_cam_start",
                                                json!({ "orientation": o }),
                                            )
                                            .await;
                                        if let Ok(st) = serde_json::from_value(res) {
                                            let _ = tx.send(BgUiMsg::VirtualCamStatus(st));
                                        }
                                    });
                                }
                            });
                        });
                    });

                ui.add_space(12.0);

                // 3. Controle Externo (obs-websocket v5 / Stream Deck)
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(
                                    "Controladores Externos (obs-websocket v5 / Stream Deck)",
                                )
                                .size(15.0)
                                .semibold()
                                .color(prism.palette.text_primary),
                            );
                            ui.add(
                                GlassBadge::new(&if app.external_ctrl.obs_ws_running {
                                    format!("Rodando ({} clientes)", app.external_ctrl.obs_ws_clients)
                                } else {
                                    "Parado".to_string()
                                })
                                .variant(if app.external_ctrl.obs_ws_running {
                                    BadgeVariant::Success
                                } else {
                                    BadgeVariant::Neutral
                                })
                                .dot(app.external_ctrl.obs_ws_running),
                            );
                        });
                        ui.add_space(8.0);
                        let mut ext_dirty = false;
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    GlassToggle::new(
                                        &mut app.settings.external_control.obs_ws_enabled,
                                    )
                                    .label("Ativar servidor obs-websocket v5"),
                                )
                                .changed()
                            {
                                ext_dirty = true;
                            }
                            ui.add_space(12.0);
                            ui.label("Porta:");
                            if ui
                                .add(
                                    egui::DragValue::new(
                                        &mut app.settings.external_control.obs_ws_port,
                                    )
                                    .range(1024..=65535),
                                )
                                .changed()
                            {
                                ext_dirty = true;
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Senha obs-websocket:");
                            ui.add(
                                egui::TextEdit::singleline(&mut app.obs_password_input)
                                    .password(true)
                                    .desired_width(210.0)
                                    .hint_text("Definir nova senha"),
                            );
                            if ui
                                .add(
                                    GlassButton::secondary("Salvar senha")
                                        .pill(true)
                                        .enabled(!app.obs_password_input.trim().is_empty())
                                        .min_size(Vec2::new(0.0, 28.0)),
                                )
                                .clicked()
                            {
                                let pwd = app.obs_password_input.trim().to_string();
                                app.obs_password_input.clear();
                                let engine = app.engine.clone();
                                let tx = app.bg_tx.clone();
                                app.rt.spawn(async move {
                                    let _ = engine
                                        .handle_command(
                                            "external_control_set_obs_password",
                                            json!({ "password": pwd }),
                                        )
                                        .await;
                                    let _ = tx.send(BgUiMsg::Toast {
                                        message: "Senha obs-websocket salva no cofre.".to_string(),
                                        is_error: false,
                                    });
                                });
                            }
                        });
                        if ext_dirty {
                            if let Ok(val) = serde_json::to_value(&app.settings.external_control) {
                                app.persist_settings(json!({ "externalControl": val }));
                            }
                        }
                    });

                ui.add_space(12.0);

                // 4. Atualizador Automático (GitHub Releases)
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Atualizações (GitHub Releases)")
                                            .size(15.0)
                                            .semibold()
                                            .color(prism.palette.text_primary),
                                    );
                                    ui.add(
                                        GlassBadge::new(&format!(
                                            "v{}",
                                            app.updater.current_version
                                        ))
                                        .variant(BadgeVariant::Neutral),
                                    );
                                });
                                ui.label(
                                    RichText::new(format!("Modo: {}", app.updater.install_kind))
                                        .monospace()
                                        .size(11.0)
                                        .color(prism.palette.text_secondary),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        GlassButton::secondary("Verificar atualizações")
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 30.0)),
                                    )
                                    .clicked()
                                {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let res = engine
                                            .handle_command("updater_check", Value::Null)
                                            .await;
                                        if let Ok(st) = serde_json::from_value(res) {
                                            let _ = tx.send(BgUiMsg::UpdateStatus(st));
                                        }
                                    });
                                }
                                if app.updater.state == UpdateState::Available {
                                    if theme::button(ui, "Baixar atualização", BtnOpts::new(Btn::Accent).icon(Icon::Download)).clicked() {
                                        let engine = app.engine.clone();
                                        app.rt.spawn(async move {
                                            let _ = engine
                                                .handle_command("updater_download", Value::Null)
                                                .await;
                                        });
                                    }
                                } else if app.updater.state == UpdateState::Ready {
                                    if theme::tally_button(ui, "Instalar e reiniciar").clicked() {
                                        let engine = app.engine.clone();
                                        let tx = app.bg_tx.clone();
                                        app.rt.spawn(async move {
                                            let res = engine
                                                .handle_command("updater_install", Value::Null)
                                                .await;
                                            if res.get("shouldExit").and_then(|v| v.as_bool())
                                                == Some(true)
                                            {
                                                let _ = tx.send(BgUiMsg::ExitRequested);
                                            }
                                        });
                                    }
                                }
                            });
                        });

                        if app.updater.state == UpdateState::Downloading {
                            ui.add_space(8.0);
                            ui.add(
                                egui::ProgressBar::new(
                                    (app.updater.progress_percent / 100.0).clamp(0.0, 1.0),
                                )
                                .text(format!("{:.0}%", app.updater.progress_percent)),
                            );
                        }
                        if let Some(ref err) = app.updater.error {
                            ui.add_space(6.0);
                            ui.label(RichText::new(err).color(theme::SPEC_RED).size(11.5));
                        }
                    });
            });
        });
}

/// Marca quadrada com a cor e as iniciais da plataforma de destino.
fn platform_mark(ui: &mut Ui, platform: PlatformId) {
    let (col, txt) = match platform {
        PlatformId::Twitch => (theme::TWITCH, "TW"),
        PlatformId::Youtube => (theme::YOUTUBE, "YT"),
        PlatformId::Kick => (theme::KICK, "K"),
        PlatformId::Tiktok => (theme::TIKTOK, "TT"),
        _ => (theme::SPEC_BLUE, "RT"),
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 7.0, col.gamma_multiply(0.22));
    ui.painter().rect_stroke(
        rect,
        7.0,
        egui::Stroke::new(1.0, col.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    let f = theme::font(ui, 10.5, FontWeight::Semibold);
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, txt, f, col);
}

/// Cartão clicável de tema com miniatura renderizada pelo próprio tema.
fn theme_card(
    ui: &mut Ui,
    preset: UiThemePreset,
    accent: UiAccent,
    active: bool,
) -> egui::Response {
    let prism = ui.ctx().prism_theme();
    let size = Vec2::new(150.0, 128.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if ui.is_rect_visible(rect) {
        let mut preview = theme::base_theme(preset);
        theme::apply_accent(&mut preview, accent);
        let p = ui.painter();
        let cr = egui::CornerRadius::same(10);
        let fill = if resp.hovered() {
            theme::hover_fill(&prism)
        } else {
            Color32::TRANSPARENT
        };
        p.rect_filled(rect, cr, fill);
        let border = if active {
            egui::Stroke::new(2.0, prism.palette.accent)
        } else {
            egui::Stroke::new(1.0, theme::hairline_color(&prism))
        };
        p.rect_stroke(rect, cr, border, egui::StrokeKind::Inside);
        let thumb = egui::Rect::from_min_size(
            rect.min + Vec2::new(6.0, 6.0),
            Vec2::new(size.x - 12.0, 78.0),
        );
        theme::paint_theme_preview(p, thumb, &preview);
        let tf = theme::font(ui, 12.5, FontWeight::Medium);
        p.text(
            egui::pos2(rect.left() + 10.0, thumb.bottom() + 15.0),
            Align2::LEFT_CENTER,
            preset.label(),
            tf,
            prism.palette.text_primary,
        );
        let cf = theme::font(ui, 10.5, FontWeight::Regular);
        p.text(
            egui::pos2(rect.left() + 10.0, thumb.bottom() + 31.0),
            Align2::LEFT_CENTER,
            preset.caption(),
            cf,
            prism.palette.text_muted,
        );
        if active {
            let c = egui::pos2(thumb.right() - 10.0, thumb.top() + 10.0);
            p.circle_filled(c, 8.0, prism.palette.accent);
            crate::native_ui::icons::paint_icon(
                p,
                egui::Rect::from_center_size(c, Vec2::splat(10.0)),
                Icon::Check,
                prism.on_accent(),
            );
        }
    }
    resp
}

/// Amostra circular de cor de destaque com rótulo.
fn accent_swatch_button(ui: &mut Ui, accent: UiAccent, active: bool) -> egui::Response {
    let prism = ui.ctx().prism_theme();
    let f = theme::font(ui, 12.0, FontWeight::Medium);
    let g = ui
        .painter()
        .layout_no_wrap(accent.label().to_owned(), f, prism.palette.text_primary);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(g.size().x + 40.0, 30.0), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let cr = egui::CornerRadius::same(15);
    if active {
        p.rect_filled(rect, cr, theme::hover_fill(&prism).gamma_multiply(2.0));
        p.rect_stroke(
            rect,
            cr,
            egui::Stroke::new(1.0, theme::hairline_color(&prism)),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, cr, theme::hover_fill(&prism));
    }
    let c = egui::pos2(rect.left() + 16.0, rect.center().y);
    let col = theme::accent_swatch(accent);
    p.circle_filled(c, 8.0, col);
    if active {
        p.circle_stroke(c, 10.5, egui::Stroke::new(1.5, col));
    }
    p.galley_with_override_text_color(
        egui::pos2(rect.left() + 30.0, rect.center().y - g.size().y * 0.5),
        g,
        if active {
            prism.palette.text_primary
        } else {
            prism.palette.text_secondary
        },
    );
    resp
}

// ------------------------------------------------------------------ Modais Auxiliares

pub fn show_confirm_quit_modal(app: &mut PrismCastNativeApp, ctx: &Context) {
    let prism = ctx.prism_theme();

    // Scrim translúcido atrás do modal
    let screen_rect = ctx.content_rect();
    egui::Area::new(egui::Id::unique("confirm_quit_backdrop"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            let _ = ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
            let scrim = match prism.mode {
                ThemeMode::Dark => Color32::from_black_alpha(138),
                ThemeMode::Light => Color32::from_rgba_unmultiplied(15, 18, 26, 82),
            };
            ui.painter().rect_filled(screen_rect, 0.0, scrim);
        });

    egui::Window::new("confirm_quit_modal")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .order(egui::Order::Foreground)
        .default_width(400.0)
        .frame(Frame::NONE.inner_margin(Margin::same(20)))
        .show(ctx, |ui| {
            paint_glass_surface(
                ui.painter(),
                ui.max_rect().expand(20.0),
                &prism.surface(GlassElevation::Floating),
            );
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Encerrar PrismCast Studio?")
                        .size(16.5)
                        .semibold()
                        .color(prism.palette.text_primary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::icon_button_ex(ui, crate::native_ui::icons::Icon::Close, "Fechar (Esc)", false, 30.0).clicked()
                    {
                        app.confirm_quit_open = false;
                    }
                });
            });
            ui.add_space(6.0);
            ui.add(
                GlassBadge::new("TRANSMISSÃO / GRAVAÇÃO ATIVA")
                    .variant(BadgeVariant::Warning)
                    .dot(true),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Você está transmitindo ou gravando no momento. Deseja realmente encerrar o estúdio?",
                )
                .color(prism.palette.text_secondary),
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        GlassButton::secondary("Continuar no ar")
                            .pill(true)
                            .min_size(Vec2::new(0.0, 32.0)),
                    )
                    .clicked()
                {
                    app.confirm_quit_open = false;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::tally_button(ui, "Encerrar agora").clicked() {
                        app.force_close = true;
                        app.confirm_quit_open = false;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
            });
        });
}

pub fn show_toast_overlay(ctx: &Context, toast: &ToastNotice) {
    let prism = ctx.prism_theme();

    egui::Area::new(egui::Id::unique("prismcast_toast"))
        .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -48.0))
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            let tokens = prism
                .surface(GlassElevation::Floating)
                .with_corner_radius(12.0);
            let accent = if toast.is_error {
                prism.palette.danger
            } else {
                theme::METER_OK
            };

            let bg_idx = ui.painter().add(egui::Shape::Noop);
            let resp = Frame::NONE
                .inner_margin(Margin {
                    left: 14,
                    right: 18,
                    top: 10,
                    bottom: 10,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(r.center(), 11.0, accent.gamma_multiply(0.18));
                        crate::native_ui::icons::paint_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(r.center(), Vec2::splat(11.0)),
                            if toast.is_error {
                                Icon::Close
                            } else {
                                Icon::Check
                            },
                            accent,
                        );
                        ui.add_space(2.0);
                        ui.label(theme::text(
                            ui,
                            &toast.message,
                            13.0,
                            FontWeight::Medium,
                            prism.palette.text_primary,
                        ));
                    });
                });

            let mut shapes = egui_prismglass::glass_surface_shapes(resp.response.rect, &tokens);
            let r = resp.response.rect;
            shapes.push(egui::Shape::rect_filled(
                egui::Rect::from_min_size(
                    r.min + Vec2::new(0.0, 10.0),
                    Vec2::new(3.0, r.height() - 20.0),
                ),
                2.0,
                accent,
            ));
            ui.painter().set(bg_idx, egui::Shape::Vec(shapes));
        });
}

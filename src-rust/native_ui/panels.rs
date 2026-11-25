//! Telas e painéis glassmorphism do PrismCast Studio alimentados por `egui-prismglass`.
//!
//! - Barra superior broadcast flutuante (`GlassPanel`) com logotipo prisma óptico,
//!   Tally animado (`AO VIVO / FORA DO AR`), telemetria `wgpu` e ações rápidas (`GlassButton`).
//! - Sidebar flutuante recolhível (`GlassSidebar` + `SidebarItem`) com badges vivos e
//!   alternador rápido de temas (`Prism Carbon`, `Onyx`, `Glacial`, `Frost`, `Minimal`).
//! - Workspace completo do Estúdio: palco 16:9 / 9:16 / Modo Estúdio, docks flutuantes
//!   (`Cenas`, `Fontes`, `Mixer de Áudio` com `GlassSlider` e VU meters broadcast) e
//!   inspetor direito (`Propriedades` com `GlassToggle` / `GlassSlider` + `Multichat`).
//! - Telas completas de `Destinos`, `Widgets`, `Rede Local (LAN P2P)` e `Ajustes`
//!   (incluindo Inspetor de Material `egui-prismglass` em tempo real).

use crate::capture::CaptureStatus;
use crate::compositor::capture_key_for_item;
use crate::native_ui::picker::PickerPurpose;
use crate::native_ui::stage::{StageAction, StageInteraction, show_stage_viewport};
use crate::native_ui::theme;
use crate::native_ui::{
    BgUiMsg, NavTab, PrismCastNativeApp, RightTab, ToastNotice, ensure_window_audio_channel,
    source_type_badge, transition_label,
};
use crate::protocol::{
    AudioChannel, AudioChannelKind, AudioFilters, AudioMonitorMode, CanvasOrientation,
    ChatFragment, ChatPlatform, Destination, DiscoveredHost, PlatformId, Scene, SceneItem,
    SourceKind, SourceType, TransitionType, TwitchLoginState, UiAppearanceSettings, UiThemePreset,
    UpdateState,
};
use egui::{
    Align, Align2, Color32, Context, Frame, Layout, Margin, RichText, Ui, Vec2, ViewportCommand,
};
use egui_prismglass::{
    BadgeVariant, GlassBadge, GlassButton, GlassCard, GlassElevation, GlassPanel, GlassSidebar,
    GlassSlider, GlassToggle, PrismContextExt, SidebarIcon, SidebarItem, SpringConfig,
    SurfaceStyle, ThemeMode, animate_spring, paint_glass_surface,
};
use serde_json::{Value, json};
use uuid::Uuid;

// ------------------------------------------------------------------ Barra Superior Broadcast

pub fn show_top_bar(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    egui::Panel::top("prismcast_top_bar")
        .exact_size(64.0)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: 10,
            right: 10,
            top: 8,
            bottom: 4,
        }))
        .show(ui, |ui| {
            GlassPanel::new()
                .padding(Margin::symmetric(14, 7))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        // Logotipo vetorial óptico + Marca
                        let (logo_rect, _) =
                            ui.allocate_exact_size(Vec2::new(30.0, 26.0), egui::Sense::hover());
                        theme::paint_prism_logo(ui.painter(), logo_rect, &prism);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(
                                RichText::new("PrismCast Studio")
                                    .size(14.5)
                                    .strong()
                                    .color(prism.palette.text_primary),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "100% Rust · {}",
                                    app.gpu_status.compositor_backend.to_uppercase()
                                ))
                                .size(9.8)
                                .monospace()
                                .color(prism.palette.text_muted),
                            );
                        });

                        ui.add_space(12.0);

                        // Pílula Tally broadcast com pulso suave (`animate_pulse`)
                        theme::paint_tally_pill(ui, app.multistream.active, app.recorder.recording);

                        ui.add_space(8.0);

                        // Strip de telemetria em tempo real dentro de cartão sutil
                        GlassCard::new()
                            .elevation(GlassElevation::Subtle)
                            .corner_radius(10.0)
                            .padding(Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    let fps_val = if app.telemetry.compositor_fps > 0.1 {
                                        app.telemetry.compositor_fps
                                    } else {
                                        app.settings.studio.fps as f32
                                    };
                                    ui.label(
                                        RichText::new(format!("{fps_val:.0} FPS"))
                                            .monospace()
                                            .size(11.0)
                                            .strong()
                                            .color(prism.palette.accent),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "{:.1} ms",
                                            app.telemetry.render_ms_p95
                                        ))
                                        .monospace()
                                        .size(11.0)
                                        .color(prism.palette.text_secondary),
                                    );
                                    if app.virtual_cam.active {
                                        ui.add(
                                            GlassBadge::new("VCAM")
                                                .variant(BadgeVariant::Accent)
                                                .dot(true),
                                        );
                                    }
                                });
                            });

                        // Botões de ação à direita
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;

                            // CTA principal: Entrar ao vivo / Encerrar live
                            let live_label = if app.multistream.active {
                                "■ Encerrar live"
                            } else {
                                "● Entrar ao vivo"
                            };
                            if theme::tally_button(ui, live_label).clicked() {
                                app.toggle_live();
                            }

                            // Gravação MP4
                            let rec_label = if app.recorder.recording {
                                format!("■ Rec {}s", app.recorder.recordingDurationSec)
                            } else {
                                "● Gravar".to_string()
                            };
                            if ui
                                .add(
                                    GlassButton::secondary(&rec_label)
                                        .selected(app.recorder.recording)
                                        .pill(true)
                                        .min_size(Vec2::new(0.0, 30.0)),
                                )
                                .clicked()
                            {
                                app.toggle_recording();
                            }

                            // Salvar Replay
                            if ui
                                .add(
                                    GlassButton::ghost("↺ Replay")
                                        .pill(true)
                                        .min_size(Vec2::new(0.0, 30.0)),
                                )
                                .on_hover_text("Salvar últimos segundos do Replay Buffer")
                                .clicked()
                            {
                                app.save_replay();
                            }

                            // Tela de Pausa
                            let shield = app.settings.studio.privacy_shield_active;
                            if ui
                                .add(
                                    GlassButton::secondary("Pausa")
                                        .selected(shield)
                                        .pill(true)
                                        .min_size(Vec2::new(0.0, 30.0)),
                                )
                                .on_hover_text("Ativar tela de pausa (Volto já)")
                                .clicked()
                            {
                                app.settings.studio.privacy_shield_active = !shield;
                                app.persist_studio();
                            }

                            // Saída dupla 9:16
                            let dual = app.settings.studio.dual_output_enabled;
                            if ui
                                .add(
                                    GlassButton::secondary("9:16")
                                        .selected(dual)
                                        .pill(true)
                                        .min_size(Vec2::new(0.0, 30.0)),
                                )
                                .on_hover_text(
                                    "Ativar saída dupla vertical 9:16 (TikTok / Shorts)",
                                )
                                .clicked()
                            {
                                app.settings.studio.dual_output_enabled = !dual;
                                if !app.settings.studio.dual_output_enabled {
                                    app.edit_vertical = false;
                                }
                                app.persist_studio();
                            }

                            // Modo Estúdio (Prévia + Programa)
                            let sm = app.settings.scenes.studio_mode;
                            if ui
                                .add(
                                    GlassButton::secondary("Modo estúdio")
                                        .selected(sm)
                                        .pill(true)
                                        .min_size(Vec2::new(0.0, 30.0)),
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

// ------------------------------------------------------------------ Sidebar Flutuante (GlassSidebar)

pub fn show_nav_rail(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();
    let collapsed = app.settings.ui_appearance.sidebar_collapsed;

    let target_w = if collapsed { 72.0 } else { 194.0 };
    let anim_w = animate_spring(
        ui.ctx(),
        egui::Id::unique("prismcast_sidebar_width"),
        target_w,
        SpringConfig::smooth(),
    );

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
        .exact_size(anim_w + 14.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: 10,
            right: 4,
            top: 4,
            bottom: 10,
        }))
        .show(ui, |ui| {
            GlassSidebar::new()
                .width(anim_w)
                .padding(Margin::symmetric(if collapsed { 8 } else { 10 }, 12))
                .show(ui, |ui| {
                    if !collapsed && anim_w > 130.0 {
                        // Cabeçalho expandido + botão recolher
                        ui.horizontal(|ui| {
                            theme::section_heading(ui, "ESTÚDIO", &prism);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        GlassButton::ghost("«")
                                            .pill(true)
                                            .min_size(Vec2::new(26.0, 24.0)),
                                    )
                                    .on_hover_text("Recolher barra lateral")
                                    .clicked()
                                {
                                    app.settings.ui_appearance.sidebar_collapsed = true;
                                    app.persist_appearance();
                                }
                            });
                        });
                        ui.add_space(6.0);

                        // Itens de navegação nativos `SidebarItem` da `egui-prismglass`
                        let mut item_studio = SidebarItem::new(SidebarIcon::Prism, "Estúdio")
                            .selected(app.active_tab == NavTab::Studio);
                        if app.multistream.active {
                            item_studio = item_studio.badge("AR");
                        }
                        if item_studio.show(ui).clicked() {
                            app.active_tab = NavTab::Studio;
                        }
                        ui.add_space(2.0);

                        let mut item_widgets = SidebarItem::new(SidebarIcon::Grid, "Widgets")
                            .selected(app.active_tab == NavTab::Widgets);
                        if let Some(ref b) = widget_badge {
                            item_widgets = item_widgets.badge(b);
                        }
                        if item_widgets.show(ui).clicked() {
                            app.active_tab = NavTab::Widgets;
                        }
                        ui.add_space(2.0);

                        let mut item_dest = SidebarItem::new(SidebarIcon::Tokens, "Destinos")
                            .selected(app.active_tab == NavTab::Destinations);
                        if let Some(ref b) = dest_badge {
                            item_dest = item_dest.badge(b);
                        }
                        if item_dest.show(ui).clicked() {
                            app.active_tab = NavTab::Destinations;
                        }
                        ui.add_space(2.0);

                        let mut item_lan = SidebarItem::new(SidebarIcon::Prism, "Rede LAN")
                            .custom_glyph("⬡")
                            .selected(app.active_tab == NavTab::Lan);
                        if let Some(ref b) = lan_badge {
                            item_lan = item_lan.badge(b);
                        }
                        if item_lan.show(ui).clicked() {
                            app.active_tab = NavTab::Lan;
                        }
                        ui.add_space(2.0);

                        let mut item_settings = SidebarItem::new(SidebarIcon::Sliders, "Ajustes")
                            .selected(app.active_tab == NavTab::Settings);
                        if has_update {
                            item_settings = item_settings.badge("UPD");
                        }
                        if item_settings.show(ui).clicked() {
                            app.active_tab = NavTab::Settings;
                        }

                        // Rodapé: alternador rápido de tema PrismGlass + versão
                        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("PrismCast v{}", env!("CARGO_PKG_VERSION")))
                                    .monospace()
                                    .size(10.0)
                                    .color(prism.palette.text_muted),
                            );
                            ui.add_space(6.0);

                            let preset = app.settings.ui_appearance.preset;
                            if ui
                                .add(
                                    GlassButton::secondary(format!("◐ {}", preset.label()))
                                        .pill(true)
                                        .min_size(Vec2::new(ui.available_width(), 30.0)),
                                )
                                .on_hover_text(
                                    "Alternar tema PrismGlass (clique para trocar estilo)",
                                )
                                .clicked()
                            {
                                app.settings.ui_appearance.preset = preset.next();
                                app.persist_appearance();
                            }

                            if has_update {
                                ui.add_space(6.0);
                                if ui
                                    .add(
                                        GlassButton::accent("⬆ Atualização")
                                            .pill(true)
                                            .min_size(Vec2::new(ui.available_width(), 28.0)),
                                    )
                                    .clicked()
                                {
                                    app.active_tab = NavTab::Settings;
                                }
                            }
                        });
                    } else {
                        // Modo compacto (recolhido)
                        ui.vertical_centered(|ui| {
                            if ui
                                .add(
                                    GlassButton::ghost("»")
                                        .pill(true)
                                        .min_size(Vec2::new(44.0, 28.0)),
                                )
                                .on_hover_text("Expandir barra lateral")
                                .clicked()
                            {
                                app.settings.ui_appearance.sidebar_collapsed = false;
                                app.persist_appearance();
                            }
                            ui.add_space(8.0);

                            let compact_items = [
                                (NavTab::Studio, "▣", "Estúdio"),
                                (NavTab::Widgets, "◈", "Widgets"),
                                (NavTab::Destinations, "◉", "Destinos"),
                                (NavTab::Lan, "⬡", "Rede LAN"),
                                (NavTab::Settings, "⚙", "Ajustes"),
                            ];
                            for (tab, glyph, tooltip) in compact_items {
                                let active = app.active_tab == tab;
                                if ui
                                    .add(
                                        GlassButton::secondary(glyph)
                                            .selected(active)
                                            .min_size(Vec2::new(46.0, 38.0)),
                                    )
                                    .on_hover_text(tooltip)
                                    .clicked()
                                {
                                    app.active_tab = tab;
                                }
                                ui.add_space(4.0);
                            }
                        });

                        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                    .monospace()
                                    .size(9.5)
                                    .color(prism.palette.text_muted),
                            );
                            ui.add_space(6.0);
                            let preset = app.settings.ui_appearance.preset;
                            if ui
                                .add(
                                    GlassButton::secondary("◐")
                                        .pill(true)
                                        .min_size(Vec2::new(40.0, 30.0)),
                                )
                                .on_hover_text(format!(
                                    "Tema: {} (clique para alternar)",
                                    preset.label()
                                ))
                                .clicked()
                            {
                                app.settings.ui_appearance.preset = preset.next();
                                app.persist_appearance();
                            }
                        });
                    }
                });
        });
}

// ------------------------------------------------------------------ Workspace do Estúdio

pub fn show_studio_workspace(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let prism = ctx.prism_theme();

    // 1. Sidebar direita flutuante (Propriedades / Chat Multichat)
    egui::Panel::right("studio_right_panel")
        .default_size(318.0)
        .size_range(268.0..=430.0)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: 4,
            right: 10,
            top: 4,
            bottom: 10,
        }))
        .show(ui, |ui| {
            let panel_h = ui.available_height();
            GlassPanel::new()
                .padding(Margin::same(12))
                .min_height((panel_h - 24.0).max(180.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let tab_w = ((ui.available_width() - 8.0) * 0.5).max(96.0);
                        if ui
                            .add(
                                GlassButton::secondary("Propriedades")
                                    .selected(app.right_tab == RightTab::Properties)
                                    .pill(true)
                                    .min_size(Vec2::new(tab_w, 30.0)),
                            )
                            .clicked()
                        {
                            app.right_tab = RightTab::Properties;
                        }
                        if ui
                            .add(
                                GlassButton::secondary(format!(
                                    "Chat ({})",
                                    app.chat_messages.len()
                                ))
                                .selected(app.right_tab == RightTab::Chat)
                                .pill(true)
                                .min_size(Vec2::new(tab_w, 30.0)),
                            )
                            .clicked()
                        {
                            app.right_tab = RightTab::Chat;
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    match app.right_tab {
                        RightTab::Properties => show_properties_panel(app, ui),
                        RightTab::Chat => show_chat_panel(app, ui),
                    }
                });
        });

    // 2. Dock inferior flutuante (Cenas | Fontes | Mixer de Áudio)
    egui::Panel::bottom("studio_bottom_dock")
        .default_size(258.0)
        .size_range(200.0..=380.0)
        .resizable(true)
        .show_separator_line(false)
        .frame(Frame::NONE.inner_margin(Margin {
            left: 6,
            right: 6,
            top: 4,
            bottom: 10,
        }))
        .show(ui, |ui| {
            let total_w = ui.available_width();
            let gap = 10.0;
            let col_scenes_w = (total_w * 0.25).clamp(200.0, 290.0);
            let col_sources_w = (total_w * 0.31).clamp(230.0, 360.0);
            let col_mixer_w = (total_w - col_scenes_w - col_sources_w - gap * 2.0).max(240.0);
            let dock_h = ui.available_height();

            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;

                ui.allocate_ui_with_layout(
                    Vec2::new(col_scenes_w, dock_h),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        GlassPanel::new()
                            .padding(Margin::same(11))
                            .min_height((dock_h - 22.0).max(140.0))
                            .show(ui, |ui| {
                                show_scenes_dock(app, ui);
                            });
                    },
                );

                ui.allocate_ui_with_layout(
                    Vec2::new(col_sources_w, dock_h),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        GlassPanel::new()
                            .padding(Margin::same(11))
                            .min_height((dock_h - 22.0).max(140.0))
                            .show(ui, |ui| {
                                show_sources_dock(app, ui);
                            });
                    },
                );

                ui.allocate_ui_with_layout(
                    Vec2::new(col_mixer_w, dock_h),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        GlassPanel::new()
                            .padding(Margin::same(11))
                            .min_height((dock_h - 22.0).max(140.0))
                            .show(ui, |ui| {
                                show_mixer_dock(app, ui);
                            });
                    },
                );
            });
        });

    // 3. Palco Central GPU (16:9, 9:16 Dual Output e Modo Estúdio)
    egui::CentralPanel::default()
        .frame(Frame::NONE.inner_margin(Margin::symmetric(8, 4)))
        .show(ui, |ui| {
            // Cabeçalho do palco
            ui.horizontal(|ui| {
                let scene_name = app
                    .editing_scene()
                    .map(|s| s.name.as_str())
                    .unwrap_or("Cena principal");
                ui.add(GlassBadge::new("CENA").variant(BadgeVariant::Neutral));
                ui.label(
                    RichText::new(scene_name)
                        .size(14.0)
                        .strong()
                        .color(prism.palette.text_primary),
                );

                if app.settings.studio.dual_output_enabled {
                    ui.add_space(10.0);
                    if theme::glass_toggle_button(ui, "Editar 16:9", !app.edit_vertical).clicked() {
                        app.edit_vertical = false;
                    }
                    if theme::glass_toggle_button(ui, "Editar 9:16", app.edit_vertical).clicked() {
                        app.edit_vertical = true;
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if app.settings.scenes.studio_mode {
                        if theme::tally_button(ui, "⇄ Transição").clicked() {
                            if !app.settings.scenes.preview_scene_id.is_empty() {
                                app.settings.scenes.active_scene_id =
                                    app.settings.scenes.preview_scene_id.clone();
                                app.persist_scenes();
                            }
                        }
                    }
                    if !app.undo_stack.is_empty() {
                        if ui
                            .add(
                                GlassButton::ghost(format!("↶ Desfazer ({})", app.undo_stack.len()))
                                    .pill(true)
                                    .min_size(Vec2::new(0.0, 28.0)),
                            )
                            .clicked()
                        {
                            app.undo();
                        }
                    }
                });
            });

            ui.add_space(6.0);

            let stage_area_h = (ui.available_height() - 24.0).max(160.0);
            let mut stage_actions: Vec<StageAction> = Vec::new();
            let editing_scene = app.editing_scene().cloned();
            let program_scene = app.program_scene().cloned();
            let selected_id = app.selected_item_id.clone();
            let dual = app.settings.studio.dual_output_enabled;
            let studio_mode = app.settings.scenes.studio_mode;

            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), stage_area_h),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if studio_mode {
                        let half_w = (ui.available_width() - 10.0) * 0.5;
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
                        ui.add_space(10.0);
                        ui.allocate_ui(Vec2::new(half_w, stage_area_h), |ui| {
                            let mut dummy = StageInteraction::default();
                            let mut ignored = Vec::new();
                            show_stage_viewport(
                                ui,
                                "stage_program_ro",
                                "PROGRAMA · NO AR",
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
                        let vert_w = (stage_area_h * 9.0 / 16.0 + 22.0).clamp(184.0, 330.0);
                        let horiz_w = (ui.available_width() - vert_w - 10.0).max(240.0);

                        ui.allocate_ui(Vec2::new(horiz_w, stage_area_h), |ui| {
                            show_stage_viewport(
                                ui,
                                "stage_h",
                                "16:9 HORIZONTAL",
                                if !app.edit_vertical {
                                    theme::SPEC_AMBER
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
                        ui.add_space(10.0);
                        ui.allocate_ui(Vec2::new(vert_w, stage_area_h), |ui| {
                            show_stage_viewport(
                                ui,
                                "stage_v",
                                "9:16 VERTICAL",
                                if app.edit_vertical {
                                    theme::SPEC_AMBER
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
                            if app.multistream.active {
                                "NO AR · 16:9"
                            } else {
                                "PALCO · 16:9"
                            },
                            if app.multistream.active {
                                theme::TALLY
                            } else {
                                theme::SPEC_TEAL
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

            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(
                        "Arraste para mover · cantos redimensionam (Shift solta proporção · Alt recorta) · botão direito abre opções · Delete remove",
                    )
                    .size(10.5)
                    .color(prism.palette.text_muted),
                );
            });
        });

    // Modal de filtros DSP de áudio quando aberto
    if let Some(ch_id) = app.filter_channel_id.clone() {
        show_audio_filters_modal(app, &ctx, &ch_id);
    }
}

// ------------------------------------------------------------------ Dock: Cenas

fn show_scenes_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    ui.horizontal(|ui| {
        theme::section_heading(ui, "CENAS", &prism);
        ui.add(
            GlassBadge::new(&app.settings.scenes.scenes.len().to_string())
                .variant(BadgeVariant::Neutral),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            egui::ComboBox::from_id_salt("transition_combo")
                .width(96.0)
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
        });
    });

    ui.add_space(6.0);

    let active_id = app.settings.scenes.active_scene_id.clone();
    let preview_id = app.settings.scenes.preview_scene_id.clone();
    let studio_mode = app.settings.scenes.studio_mode;

    let mut select_scene: Option<String> = None;
    let mut delete_scene: Option<String> = None;

    egui::ScrollArea::vertical()
        .id_salt("scenes_scroll")
        .max_height((ui.available_height() - 36.0).max(80.0))
        .show(ui, |ui| {
            for (idx, scene) in app.settings.scenes.scenes.iter().enumerate() {
                let is_selected = if studio_mode {
                    scene.id == preview_id
                } else {
                    scene.id == active_id
                };
                let is_on_air = scene.id == active_id;

                let mut card = GlassCard::new()
                    .elevation(if is_selected {
                        GlassElevation::Elevated
                    } else {
                        GlassElevation::Subtle
                    })
                    .corner_radius(10.0)
                    .padding(Margin::symmetric(10, 6))
                    .fill_width(true);
                if is_selected {
                    card = card.tint(theme::selected_card_tint(&prism));
                }

                card.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let idx_resp = ui.add(
                            egui::Label::new(
                                RichText::new(format!("{:02}", idx + 1))
                                    .monospace()
                                    .size(10.5)
                                    .color(if is_on_air {
                                        theme::SPEC_RED
                                    } else {
                                        prism.palette.text_muted
                                    }),
                            )
                            .sense(egui::Sense::click()),
                        );
                        let name_resp = ui.add(
                            egui::Label::new(
                                RichText::new(&scene.name)
                                    .size(12.5)
                                    .strong()
                                    .color(if is_selected {
                                        prism.palette.text_primary
                                    } else {
                                        prism.palette.text_secondary
                                    }),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if idx_resp.hovered() || name_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if idx_resp.clicked() || name_resp.clicked() {
                            select_scene = Some(scene.id.clone());
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if app.settings.scenes.scenes.len() > 1
                                && ui
                                    .small_button(
                                        RichText::new("🗑").color(prism.palette.text_muted),
                                    )
                                    .on_hover_text("Remover cena")
                                    .clicked()
                            {
                                delete_scene = Some(scene.id.clone());
                            }
                            if studio_mode && is_on_air {
                                ui.add(
                                    GlassBadge::new("AR")
                                        .variant(BadgeVariant::Warning)
                                        .dot(true),
                                );
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });

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

    // Rodapé: adicionar nova cena
    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            if app.adding_scene {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.new_scene_name)
                        .desired_width(115.0)
                        .hint_text("Nome da cena"),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui
                    .add(
                        GlassButton::accent("Criar")
                            .pill(true)
                            .min_size(Vec2::new(0.0, 26.0)),
                    )
                    .clicked()
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
                if ui
                    .add(
                        GlassButton::ghost("✕")
                            .pill(true)
                            .min_size(Vec2::new(26.0, 26.0)),
                    )
                    .clicked()
                {
                    app.adding_scene = false;
                    app.new_scene_name.clear();
                }
            } else {
                if ui
                    .add(
                        GlassButton::secondary("+ Nova cena")
                            .pill(true)
                            .min_size(Vec2::new(0.0, 26.0)),
                    )
                    .clicked()
                {
                    app.adding_scene = true;
                    app.new_scene_name =
                        format!("Cena {}", app.settings.scenes.scenes.len() + 1);
                }
                if ui
                    .add(
                        egui::DragValue::new(&mut app.settings.scenes.transition_duration_ms)
                            .range(50..=3000)
                            .speed(25)
                            .suffix(" ms"),
                    )
                    .changed()
                {
                    app.persist_scenes();
                }
            }
        });
    });
}

// ------------------------------------------------------------------ Dock: Fontes

fn show_sources_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    ui.horizontal(|ui| {
        theme::section_heading(ui, "FONTES", &prism);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let count = app.editing_scene().map(|s| s.items.len()).unwrap_or(0);
            ui.add(GlassBadge::new(&format!("{count} itens")).variant(BadgeVariant::Neutral));
        });
    });

    ui.add_space(6.0);

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
        .max_height((ui.available_height() - 36.0).max(80.0))
        .show(ui, |ui| {
            if items_snapshot.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Nenhuma fonte nesta cena ainda.")
                        .size(12.0)
                        .color(prism.palette.text_muted),
                );
            }
            for item in &items_snapshot {
                let selected = app.selected_item_id.as_deref() == Some(item.id.as_str());
                let status = capture_key_for_item(item)
                    .and_then(|k| app.capture_statuses.get(&k))
                    .copied();

                let mut card = GlassCard::new()
                    .elevation(if selected {
                        GlassElevation::Elevated
                    } else {
                        GlassElevation::Subtle
                    })
                    .corner_radius(10.0)
                    .padding(Margin::symmetric(9, 6))
                    .fill_width(true);
                if selected {
                    card = card.tint(theme::selected_card_tint(&prism));
                }

                card.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let dot_col = match (item.source_type, status) {
                            (
                                SourceType::ScreenCapture | SourceType::WindowCapture,
                                Some(CaptureStatus::Ok),
                            ) => theme::METER_OK,
                            (
                                SourceType::ScreenCapture | SourceType::WindowCapture,
                                Some(CaptureStatus::Missing | CaptureStatus::Error),
                            ) => theme::SPEC_AMBER,
                            _ => prism.palette.accent,
                        };
                        let (drect, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            drect.center(),
                            4.5,
                            Color32::from_rgba_unmultiplied(
                                dot_col.r(),
                                dot_col.g(),
                                dot_col.b(),
                                55,
                            ),
                        );
                        ui.painter().circle_filled(drect.center(), 2.8, dot_col);

                        let badge_resp = ui.add(
                            egui::Label::new(
                                RichText::new(source_type_badge(item.source_type))
                                    .monospace()
                                    .size(9.5)
                                    .color(prism.palette.text_muted),
                            )
                            .sense(egui::Sense::click()),
                        );

                        let mut label = item.name.clone();
                        if label.chars().count() > 16 {
                            label = format!("{}…", label.chars().take(15).collect::<String>());
                        }
                        let name_resp = ui.add(
                            egui::Label::new(
                                RichText::new(label).size(12.2).color(if !item.visible {
                                    prism.palette.text_muted
                                } else if selected {
                                    prism.palette.text_primary
                                } else {
                                    prism.palette.text_secondary
                                }),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if badge_resp.hovered() || name_resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if badge_resp.clicked() || name_resp.clicked() {
                            select_id = Some(item.id.clone());
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if ui
                                .small_button(RichText::new("🗑").color(prism.palette.text_muted))
                                .on_hover_text("Remover fonte")
                                .clicked()
                            {
                                remove_id = Some(item.id.clone());
                            }
                            if ui
                                .small_button(if item.visible { "👁" } else { "⊘" })
                                .on_hover_text("Mostrar / ocultar")
                                .clicked()
                            {
                                toggle_vis = Some(item.id.clone());
                            }
                            if ui
                                .small_button(if item.locked { "🔒" } else { "🔓" })
                                .on_hover_text("Travar / destravar")
                                .clicked()
                            {
                                toggle_lock = Some(item.id.clone());
                            }
                            if ui.small_button("↑").on_hover_text("Subir camada").clicked() {
                                move_delta = Some((item.id.clone(), 1));
                            }
                            if ui.small_button("↓").on_hover_text("Descer camada").clicked() {
                                move_delta = Some((item.id.clone(), -1));
                            }
                        });
                    });
                });
                ui.add_space(4.0);
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

    // Rodapé: adicionar fontes nativas
    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            if ui
                .add(
                    GlassButton::accent("+ Tela")
                        .pill(true)
                        .min_size(Vec2::new(0.0, 26.0)),
                )
                .clicked()
            {
                app.picker
                    .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
            }
            if ui
                .add(
                    GlassButton::secondary("+ Janela")
                        .pill(true)
                        .min_size(Vec2::new(0.0, 26.0)),
                )
                .clicked()
            {
                app.picker
                    .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
            }
            ui.menu_button("+ Outros ▾", |ui| {
                if ui.button("Imagem (PNG / JPG / WebP)").clicked() {
                    app.add_simple_source(SourceType::ImageSource);
                    ui.close();
                }
                if ui.button("Cor sólida").clicked() {
                    app.add_simple_source(SourceType::ColorSource);
                    ui.close();
                }
                if ui.button("Texto").clicked() {
                    app.add_simple_source(SourceType::TextSource);
                    ui.close();
                }
                if ui.button("Widget HTML / URL").clicked() {
                    app.add_simple_source(SourceType::BrowserSource);
                    ui.close();
                }
                if ui.button("Câmera").clicked() {
                    app.add_simple_source(SourceType::CameraInput);
                    ui.close();
                }
            });
        });
    });
}

// ------------------------------------------------------------------ Dock: Mixer de Áudio

fn show_mixer_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    let prism = ui.ctx().prism_theme();

    ui.horizontal(|ui| {
        theme::section_heading(ui, "MIXER · 48 KHZ DSP", &prism);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(
                    GlassButton::secondary("+ Canal App")
                        .pill(true)
                        .min_size(Vec2::new(0.0, 24.0)),
                )
                .clicked()
            {
                let id = format!("app-{}", Uuid::new_v4());
                app.settings.audio_mixer.channels.push(AudioChannel {
                    id,
                    kind: AudioChannelKind::App,
                    name: "Aplicativo".to_string(),
                    filters: AudioFilters::default_for(AudioChannelKind::App),
                    ..Default::default()
                });
                app.persist_audio();
            }
        });
    });

    ui.add_space(6.0);

    let mut dirty = false;
    let mut open_filters: Option<String> = None;
    let mut remove_ch: Option<String> = None;
    let avail_w = ui.available_width();

    egui::ScrollArea::vertical()
        .id_salt("mixer_scroll")
        .max_height((ui.available_height() - 4.0).max(110.0))
        .show(ui, |ui| {
            for ch in &mut app.settings.audio_mixer.channels {
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

                GlassCard::new()
                    .elevation(GlassElevation::Subtle)
                    .corner_radius(10.0)
                    .padding(Margin::symmetric(10, 7))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&ch.name)
                                    .size(12.5)
                                    .strong()
                                    .color(prism.palette.text_primary),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                if ch.kind == AudioChannelKind::App
                                    && ui
                                        .small_button(
                                            RichText::new("🗑").color(prism.palette.text_muted),
                                        )
                                        .on_hover_text("Remover canal")
                                        .clicked()
                                {
                                    remove_ch = Some(ch.id.clone());
                                }
                                if ui
                                    .add(
                                        GlassButton::secondary("DSP")
                                            .selected(has_dsp)
                                            .pill(true)
                                            .min_size(Vec2::new(42.0, 22.0)),
                                    )
                                    .on_hover_text(
                                        "Filtros DSP, EQ 3 bandas, Compressor e VST3/CLAP",
                                    )
                                    .clicked()
                                {
                                    open_filters = Some(ch.id.clone());
                                }
                                let mute_resp = if ch.muted {
                                    theme::tally_button_compact(ui, "Mudo")
                                } else {
                                    ui.add(
                                        GlassButton::ghost("🔊")
                                            .pill(true)
                                            .min_size(Vec2::new(30.0, 22.0)),
                                    )
                                };
                                if mute_resp.clicked() {
                                    ch.muted = !ch.muted;
                                    dirty = true;
                                }
                                ui.label(
                                    RichText::new(format!("{:+.1} dB", ch.volume_db))
                                        .monospace()
                                        .size(10.8)
                                        .color(if ch.muted {
                                            prism.palette.text_muted
                                        } else {
                                            prism.palette.text_secondary
                                        }),
                                );
                            });
                        });
                        ui.add_space(3.0);
                        theme::paint_vu_meter(ui, peak, ch.muted, avail_w - 34.0);
                        let fader_accent = if ch.muted {
                            prism.palette.text_muted
                        } else {
                            prism.palette.accent
                        };
                        if ui
                            .add(
                                GlassSlider::new(&mut ch.volume_db, -60.0..=12.0)
                                    .accent(fader_accent),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                ui.add_space(5.0);
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
                                .strong()
                                .color(prism.palette.text_primary),
                        );
                        ui.add(GlassBadge::new("48 kHz DSP").variant(BadgeVariant::Accent));
                    });
                    ui.label(
                        RichText::new("Filtros em tempo real · Passa-alta, Gate, Compressor e EQ 3 bandas")
                            .size(11.0)
                            .color(prism.palette.text_muted),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            GlassButton::ghost("✕")
                                .pill(true)
                                .min_size(Vec2::new(28.0, 28.0)),
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
                                            if ui.selectable_value(&mut ch.monitor, m, lbl).clicked()
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
                                .add(GlassToggle::new(&mut ch.filters.gate.enabled).label("Noise Gate"))
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
        ui.add_space(12.0);
        GlassCard::new()
            .elevation(GlassElevation::Subtle)
            .padding(Margin::same(16))
            .fill_width(true)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Nenhuma fonte selecionada")
                            .size(13.5)
                            .strong()
                            .color(prism.palette.text_primary),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Clique em uma fonte no palco ou na lista inferior para editar posição, recorte e captura.")
                            .size(11.5)
                            .color(prism.palette.text_secondary),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                GlassButton::accent("+ Tela")
                                    .pill(true)
                                    .min_size(Vec2::new(0.0, 28.0)),
                            )
                            .clicked()
                        {
                            app.picker
                                .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
                        }
                        if ui
                            .add(
                                GlassButton::secondary("+ Janela")
                                    .pill(true)
                                    .min_size(Vec2::new(0.0, 28.0)),
                            )
                            .clicked()
                        {
                            app.picker
                                .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
                        }
                    });
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
                ui.label(
                    RichText::new(&item_snapshot.name)
                        .size(15.0)
                        .strong()
                        .color(prism.palette.text_primary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        GlassBadge::new(source_type_badge(item_snapshot.source_type))
                            .variant(BadgeVariant::Accent),
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
                            Some(CaptureStatus::Error) => {
                                ("Erro na captura", BadgeVariant::Warm)
                            }
                            _ => ("Conectando captura…", BadgeVariant::Neutral),
                        };
                        ui.add(GlassBadge::new(badge_txt).variant(badge_var).dot(true));
                        ui.add_space(8.0);
                        if theme::tally_button(ui, "Escolher tela / janela na lista").clicked() {
                            open_picker = true;
                        }
                        if item_snapshot.source_type == SourceType::WindowCapture {
                            ui.add_space(6.0);
                            let has_audio = item_snapshot.settings.audio_channel_id.is_some();
                            if ui
                                .add(
                                    GlassButton::secondary(if has_audio {
                                        "✓ Áudio desta janela vinculado"
                                    } else {
                                        "+ Capturar o áudio desta janela"
                                    })
                                    .selected(has_audio)
                                    .pill(true)
                                    .enabled(!has_audio)
                                    .min_size(Vec2::new(ui.available_width(), 28.0)),
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
                        if ui
                            .add(
                                GlassButton::secondary("Trocar arquivo de imagem…")
                                    .pill(true)
                                    .min_size(Vec2::new(0.0, 28.0)),
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
                                if ui
                                    .add(
                                        GlassButton::secondary("Tela cheia")
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 26.0)),
                                    )
                                    .clicked()
                                {
                                    tf.x = 0.0;
                                    tf.y = 0.0;
                                    tf.width = 1.0;
                                    tf.height = 1.0;
                                    dirty = true;
                                }
                                if ui
                                    .add(
                                        GlassButton::secondary("Centralizar")
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 26.0)),
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

    ui.horizontal(|ui| {
        theme::section_heading(ui, "MULTICHAT AO VIVO", &prism);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(
                    GlassButton::ghost("↻ Reconectar")
                        .pill(true)
                        .min_size(Vec2::new(0.0, 24.0)),
                )
                .clicked()
            {
                let engine = app.engine.clone();
                app.rt.spawn(async move {
                    let _ = engine
                        .handle_command("multichat_reconnect", Value::Null)
                        .await;
                });
            }
        });
    });

    ui.add_space(6.0);

    let feed_h = (ui.available_height() - 44.0).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("multichat_feed")
        .stick_to_bottom(true)
        .max_height(feed_h)
        .show(ui, |ui| {
            if app.chat_messages.is_empty() {
                ui.add_space(16.0);
                GlassCard::new()
                    .elevation(GlassElevation::Subtle)
                    .padding(Margin::same(14))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(
                                "Conecte Twitch, YouTube ou Kick em Destinos para acompanhar o chat unificado aqui.",
                            )
                            .size(12.0)
                            .color(prism.palette.text_secondary),
                        );
                    });
            }
            for msg in &app.chat_messages {
                let (plat_lbl, plat_col) = match msg.platform {
                    ChatPlatform::Twitch => ("TW", Color32::from_rgb(169, 112, 255)),
                    ChatPlatform::Youtube => ("YT", theme::SPEC_RED),
                    ChatPlatform::Kick => ("KK", theme::METER_OK),
                };
                let text_body: String = msg
                    .fragments
                    .iter()
                    .map(|f| match f {
                        ChatFragment::Text { text } => text.as_str(),
                        ChatFragment::Emote { name, .. } => name.as_str(),
                    })
                    .collect();

                GlassCard::new()
                    .elevation(GlassElevation::Flat)
                    .corner_radius(9.0)
                    .padding(Margin::symmetric(8, 5))
                    .fill_width(true)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(plat_lbl)
                                    .monospace()
                                    .size(10.0)
                                    .strong()
                                    .color(plat_col),
                            );
                            ui.label(
                                RichText::new(format!("{}:", msg.name))
                                    .size(12.0)
                                    .strong()
                                    .color(prism.palette.text_primary),
                            );
                            ui.label(
                                RichText::new(text_body)
                                    .size(12.0)
                                    .color(prism.palette.text_secondary),
                            );
                        });
                    });
                ui.add_space(3.0);
            }
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.chat_input)
                .desired_width((ui.available_width() - 78.0).max(80.0))
                .hint_text("Enviar no chat Twitch…"),
        );
        let send = (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            || ui
                .add(
                    GlassButton::accent("Enviar")
                        .pill(true)
                        .min_size(Vec2::new(68.0, 28.0)),
                )
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
        .frame(Frame::NONE.inner_margin(Margin::symmetric(18, 12)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Destinos de Transmissão")
                            .size(22.0)
                            .strong()
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
                                            .strong()
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
                                        .strong()
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
                                ui.label(
                                    RichText::new(&dest.name)
                                        .size(15.0)
                                        .strong()
                                        .color(prism.palette.text_primary),
                                );
                                let orient_lbl = match dest.orientation {
                                    CanvasOrientation::Horizontal => "16:9 Horizontal",
                                    CanvasOrientation::Vertical => "9:16 Vertical",
                                };
                                if ui
                                    .add(
                                        GlassButton::secondary(orient_lbl)
                                            .selected(
                                                dest.orientation == CanvasOrientation::Vertical,
                                            )
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 24.0)),
                                    )
                                    .clicked()
                                {
                                    dest.orientation = match dest.orientation {
                                        CanvasOrientation::Horizontal => {
                                            CanvasOrientation::Vertical
                                        }
                                        CanvasOrientation::Vertical => {
                                            CanvasOrientation::Horizontal
                                        }
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
                                    .strong()
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
        .frame(Frame::NONE.inner_margin(Margin::symmetric(18, 12)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(
                    RichText::new("Widgets & Kit de Overlays")
                        .size(22.0)
                        .strong()
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
                                if theme::tally_button(ui, "Selecionar pasta do kit…").clicked() {
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
                                .strong()
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
                                                .strong()
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
        .frame(Frame::NONE.inner_margin(Margin::symmetric(18, 12)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Rede Local (LAN P2P)")
                            .size(22.0)
                            .strong()
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
                                        .strong()
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
                                    .strong()
                                    .color(prism.palette.text_primary),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        GlassButton::secondary("↻ Procurar na rede")
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 28.0)),
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
                                                    .strong()
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
        .frame(Frame::NONE.inner_margin(Margin::symmetric(18, 12)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(
                    RichText::new("Ajustes do Estúdio")
                        .size(22.0)
                        .strong()
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

                // 0. Aparência & Material (`egui-prismglass`)
                GlassCard::new()
                    .elevation(GlassElevation::Elevated)
                    .padding(Margin::same(16))
                    .fill_width(true)
                    .show(ui, |ui| {
                        let mut app_dirty = false;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Aparência & Material (egui-prismglass)")
                                    .size(15.5)
                                    .strong()
                                    .color(prism.palette.text_primary),
                            );
                            ui.add(
                                GlassBadge::new(if prism.surface_style == SurfaceStyle::Glass {
                                    "Glass 3D"
                                } else {
                                    "Minimal Solid"
                                })
                                .variant(BadgeVariant::Accent)
                                .dot(true),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        GlassButton::ghost("Restaurar padrão")
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 26.0)),
                                    )
                                    .clicked()
                                {
                                    let keep_collapsed =
                                        app.settings.ui_appearance.sidebar_collapsed;
                                    app.settings.ui_appearance = UiAppearanceSettings {
                                        sidebar_collapsed: keep_collapsed,
                                        ..Default::default()
                                    };
                                    app_dirty = true;
                                }
                            });
                        });

                        ui.add_space(10.0);
                        theme::section_heading(ui, "TEMA VISUAL", &prism);
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            for preset in UiThemePreset::ALL {
                                let active = app.settings.ui_appearance.preset == preset;
                                if ui
                                    .add(
                                        GlassButton::secondary(preset.label())
                                            .selected(active)
                                            .pill(true)
                                            .min_size(Vec2::new(0.0, 30.0)),
                                    )
                                    .clicked()
                                {
                                    app.settings.ui_appearance.preset = preset;
                                    app_dirty = true;
                                }
                            }
                        });

                        ui.add_space(12.0);
                        theme::section_heading(ui, "INSPETOR DE MATERIAL", &prism);
                        ui.add_space(6.0);

                        let is_glass = prism.surface_style == SurfaceStyle::Glass;
                        ui.columns(2, |cols| {
                            if is_glass {
                                if cols[0]
                                    .add(
                                        GlassSlider::new(
                                            &mut app.settings.ui_appearance.glass_opacity,
                                            0.55..=1.45,
                                        )
                                        .label("Densidade do vidro")
                                        .suffix("×")
                                        .decimals(2),
                                    )
                                    .changed()
                                {
                                    app_dirty = true;
                                }
                                if cols[0]
                                    .add(
                                        GlassSlider::new(
                                            &mut app.settings.ui_appearance.specular_intensity,
                                            0.0..=1.8,
                                        )
                                        .label("Brilho especular da borda")
                                        .suffix("×")
                                        .decimals(2),
                                    )
                                    .changed()
                                {
                                    app_dirty = true;
                                }
                            }

                            if cols[1]
                                .add(
                                    GlassSlider::new(
                                        &mut app.settings.ui_appearance.radius_scale,
                                        0.5..=1.4,
                                    )
                                    .label("Arredondamento")
                                    .suffix("×")
                                    .decimals(2),
                                )
                                .changed()
                            {
                                app_dirty = true;
                            }
                            if is_glass {
                                if cols[1]
                                    .add(
                                        GlassSlider::new(
                                            &mut app.settings.ui_appearance.backdrop_intensity,
                                            0.0..=1.4,
                                        )
                                        .label("Luminância do fundo 3D")
                                        .suffix("×")
                                        .decimals(2),
                                    )
                                    .changed()
                                {
                                    app_dirty = true;
                                }
                            }
                        });

                        if is_glass {
                            ui.add_space(6.0);
                            if ui
                                .add(
                                    GlassToggle::new(
                                        &mut app.settings.ui_appearance.animate_backdrop,
                                    )
                                    .label("Animar ondas de luz volumétricas no fundo"),
                                )
                                .changed()
                            {
                                app_dirty = true;
                            }
                        }

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
                                    .strong()
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
                                            .strong()
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
                                .strong()
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
                                            .strong()
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
                                    if theme::tally_button(ui, "Baixar atualização").clicked() {
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
                        .strong()
                        .color(prism.palette.text_primary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            GlassButton::ghost("✕")
                                .pill(true)
                                .min_size(Vec2::new(28.0, 28.0)),
                        )
                        .clicked()
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
        .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -24.0))
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            let mut tokens = prism
                .surface(GlassElevation::Floating)
                .with_corner_radius(14.0);
            let accent = if toast.is_error {
                theme::TALLY
            } else {
                prism.palette.accent
            };
            tokens.border = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 185);
            tokens.border_width = 1.2;

            let bg_idx = ui.painter().add(egui::Shape::Noop);
            let resp = Frame::NONE
                .inner_margin(Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (dot_r, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            dot_r.center(),
                            4.5,
                            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 64),
                        );
                        ui.painter().circle_filled(dot_r.center(), 2.8, accent);
                        ui.label(
                            RichText::new(&toast.message)
                                .size(12.5)
                                .color(prism.palette.text_primary),
                        );
                    });
                });

            let shapes = egui_prismglass::glass_surface_shapes(resp.response.rect, &tokens);
            ui.painter().set(bg_idx, egui::Shape::Vec(shapes));
        });
}

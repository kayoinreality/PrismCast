//! Telas e painéis glassmorphism do PrismCast Studio 100% nativo em Rust.
//!
//! - Barra superior broadcast com logotipo prisma óptico, Tally `AO VIVO / FORA DO AR`,
//!   telemetria `wgpu` e ações rápidas.
//! - Rail lateral esquerdo (`Estúdio`, `Widgets`, `Destinos`, `Rede Local`, `Ajustes`).
//! - Workspace completo do Estúdio: palco 16:9 / 9:16 / Modo Estúdio, barra inferior
//!   (`Cenas`, `Fontes`, `Mixer de Áudio` com VU meters e filtros DSP) e sidebar direita
//!   (`Propriedades` + `Chat Multichat`).
//! - Telas completas de `Destinos`, `Widgets`, `Rede Local (LAN P2P)` e `Ajustes`
//!   (GPU Zero-Copy, ABR, Câmera Virtual, obs-websocket v5, Atalhos e Atualizador).

use crate::capture::CaptureStatus;
use crate::compositor::capture_key_for_item;
use crate::native_ui::picker::PickerPurpose;
use crate::native_ui::stage::{show_stage_viewport, StageAction, StageInteraction};
use crate::native_ui::{
    ensure_window_audio_channel, source_type_badge, transition_label, BgUiMsg, NavTab,
    PrismCastNativeApp, RightTab, ToastNotice,
};
use crate::native_ui::theme;
use crate::protocol::{
    AudioChannel, AudioChannelKind, AudioFilters, AudioMonitorMode, CanvasOrientation,
    ChatFragment, ChatPlatform, Destination, DiscoveredHost, PlatformId, Scene, SceneItem,
    SourceKind, SourceType, TransitionType, TwitchLoginState, UpdateState,
};
use egui::{
    Align, Align2, Color32, Context, CornerRadius, FontId, Frame, Layout, Margin, RichText,
    Stroke, StrokeKind, Ui, Vec2, ViewportCommand,
};
use serde_json::{json, Value};
use uuid::Uuid;

// ------------------------------------------------------------------ Barra Superior Broadcast

pub fn show_top_bar(app: &mut PrismCastNativeApp, ctx: &Context, time_sec: f64) {
    egui::TopBottomPanel::top("prismcast_top_bar")
        .exact_height(56.0)
        .frame(
            Frame::new()
                .fill(theme::GLASS_PANEL)
                .stroke(Stroke::new(1.0_f32, theme::GLASS_EDGE))
                .inner_margin(Margin::symmetric(14, 8)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // Logotipo vetorial óptico + Marca
                let (logo_rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 26.0), egui::Sense::hover());
                theme::paint_prism_logo(ui.painter(), logo_rect);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        RichText::new("PrismCast Studio")
                            .size(14.5)
                            .strong()
                            .color(theme::INK),
                    );
                    ui.label(
                        RichText::new(format!(
                            "100% Rust Nativo · {}",
                            app.gpu_status.compositor_backend.to_uppercase()
                        ))
                        .size(10.0)
                        .monospace()
                        .color(theme::MUTED),
                    );
                });

                ui.add_space(16.0);

                // Pílula Tally broadcast (AO VIVO / GRAVANDO / FORA DO AR)
                theme::paint_tally_pill(ui, app.multistream.active, app.recorder.recording, time_sec);

                ui.add_space(10.0);

                // Strip de telemetria em tempo real
                Frame::new()
                    .fill(theme::GLASS_CARD)
                    .stroke(Stroke::new(1.0_f32, theme::GLASS_EDGE))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 4))
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
                                    .color(theme::SPEC_TEAL),
                            );
                            ui.label(
                                RichText::new(format!("{:.1} ms", app.telemetry.render_ms_p95))
                                    .monospace()
                                    .size(11.0)
                                    .color(theme::MUTED),
                            );
                            if app.virtual_cam.active {
                                ui.label(
                                    RichText::new("VCAM")
                                        .monospace()
                                        .size(10.5)
                                        .color(theme::SPEC_BLUE),
                                );
                            }
                        });
                    });

                // Botões de ação à direita
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;

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
                    if theme::glass_toggle_button(ui, &rec_label, app.recorder.recording).clicked() {
                        app.toggle_recording();
                    }

                    // Salvar Replay
                    if ui
                        .button(RichText::new("↺ Replay").size(12.0))
                        .on_hover_text("Salvar últimos segundos do Replay Buffer")
                        .clicked()
                    {
                        app.save_replay();
                    }

                    // Tela de Pausa
                    let shield = app.settings.studio.privacy_shield_active;
                    if theme::glass_toggle_button(ui, "Pausa", shield)
                        .on_hover_text("Ativar tela de pausa (Volto já)")
                        .clicked()
                    {
                        app.settings.studio.privacy_shield_active = !shield;
                        app.persist_studio();
                    }

                    // Saída dupla 9:16
                    let dual = app.settings.studio.dual_output_enabled;
                    if theme::glass_toggle_button(ui, "9:16", dual)
                        .on_hover_text("Ativar saída dupla vertical 9:16 (TikTok / Shorts)")
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
                    if theme::glass_toggle_button(ui, "Modo estúdio", sm).clicked() {
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
}

// ------------------------------------------------------------------ Rail de Navegação Esquerdo

pub fn show_nav_rail(app: &mut PrismCastNativeApp, ctx: &Context) {
    egui::SidePanel::left("prismcast_nav_rail")
        .exact_width(86.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(theme::GLASS_PANEL)
                .stroke(Stroke::new(1.0_f32, theme::GLASS_EDGE))
                .inner_margin(Margin::symmetric(8, 12)),
        )
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                let items = [
                    (NavTab::Studio, "▣", "Estúdio"),
                    (NavTab::Widgets, "◈", "Widgets"),
                    (NavTab::Destinations, "◉", "Destinos"),
                    (NavTab::Lan, "⬡", "Rede LAN"),
                    (NavTab::Settings, "⚙", "Ajustes"),
                ];

                for (tab, icon, label) in items {
                    let active = app.active_tab == tab;
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(68.0, 56.0), egui::Sense::click());
                    let p = ui.painter();

                    let bg = if active {
                        Color32::from_rgba_unmultiplied(214, 54, 38, 42)
                    } else if resp.hovered() {
                        theme::GLASS_CARD_HOVER
                    } else {
                        Color32::TRANSPARENT
                    };
                    let stroke = if active {
                        Stroke::new(1.2_f32, theme::TALLY)
                    } else if resp.hovered() {
                        Stroke::new(1.0_f32, theme::GLASS_EDGE)
                    } else {
                        Stroke::NONE
                    };
                    p.rect_filled(rect, CornerRadius::same(12), bg);
                    p.rect_stroke(rect, CornerRadius::same(12), stroke, StrokeKind::Inside);

                    p.text(
                        egui::Pos2::new(rect.center().x, rect.top() + 20.0),
                        Align2::CENTER_CENTER,
                        icon,
                        FontId::proportional(17.0),
                        if active { theme::INK } else { theme::MUTED },
                    );
                    p.text(
                        egui::Pos2::new(rect.center().x, rect.bottom() - 13.0),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.0),
                        if active { theme::INK } else { theme::MUTED },
                    );

                    if resp.clicked() {
                        app.active_tab = tab;
                    }
                }
            });

            // Rodapé com versão e indicador de atualização
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                if app.updater.state == UpdateState::Available
                    || app.updater.state == UpdateState::Ready
                {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("⬆ Update")
                                    .size(10.5)
                                    .color(theme::INK),
                            )
                            .fill(Color32::from_rgba_unmultiplied(24, 220, 184, 50))
                            .stroke(Stroke::new(1.0_f32, theme::SPEC_TEAL))
                            .corner_radius(CornerRadius::same(8)),
                        )
                        .clicked()
                    {
                        app.active_tab = NavTab::Settings;
                    }
                    ui.add_space(4.0);
                }
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .monospace()
                        .size(10.5)
                        .color(theme::DIM),
                );
            });
        });
}

// ------------------------------------------------------------------ Workspace do Estúdio

pub fn show_studio_workspace(app: &mut PrismCastNativeApp, ctx: &Context) {
    // 1. Sidebar direita (Propriedades / Chat Multichat)
    egui::SidePanel::right("studio_right_panel")
        .default_width(310.0)
        .width_range(260.0..=420.0)
        .frame(
            Frame::new()
                .fill(theme::GLASS_PANEL)
                .stroke(Stroke::new(1.0_f32, theme::GLASS_EDGE))
                .inner_margin(Margin::same(12)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if theme::glass_toggle_button(
                    ui,
                    "Propriedades",
                    app.right_tab == RightTab::Properties,
                )
                .clicked()
                {
                    app.right_tab = RightTab::Properties;
                }
                if theme::glass_toggle_button(
                    ui,
                    &format!("Chat ({})", app.chat_messages.len()),
                    app.right_tab == RightTab::Chat,
                )
                .clicked()
                {
                    app.right_tab = RightTab::Chat;
                }
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            match app.right_tab {
                RightTab::Properties => show_properties_panel(app, ui),
                RightTab::Chat => show_chat_panel(app, ui),
            }
        });

    // 2. Dock inferior (Cenas | Fontes | Mixer de Áudio)
    egui::TopBottomPanel::bottom("studio_bottom_dock")
        .default_height(248.0)
        .height_range(190.0..=360.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(10, 8)),
        )
        .show(ctx, |ui| {
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
                        theme::glass_panel_frame().show(ui, |ui| {
                            ui.set_min_height(dock_h - 22.0);
                            show_scenes_dock(app, ui);
                        });
                    },
                );

                ui.allocate_ui_with_layout(
                    Vec2::new(col_sources_w, dock_h),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        theme::glass_panel_frame().show(ui, |ui| {
                            ui.set_min_height(dock_h - 22.0);
                            show_sources_dock(app, ui);
                        });
                    },
                );

                ui.allocate_ui_with_layout(
                    Vec2::new(col_mixer_w, dock_h),
                    Layout::top_down(Align::LEFT),
                    |ui| {
                        theme::glass_panel_frame().show(ui, |ui| {
                            ui.set_min_height(dock_h - 22.0);
                            show_mixer_dock(app, ui);
                        });
                    },
                );
            });
        });

    // 3. Palco Central GPU (16:9, 9:16 Dual Output e Modo Estúdio)
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(Color32::TRANSPARENT)
                .inner_margin(Margin::symmetric(12, 8)),
        )
        .show(ctx, |ui| {
            // Cabeçalho do palco
            ui.horizontal(|ui| {
                let scene_name = app
                    .editing_scene()
                    .map(|s| s.name.as_str())
                    .unwrap_or("Cena principal");
                ui.label(
                    RichText::new("CENA")
                        .monospace()
                        .size(10.5)
                        .color(theme::DIM),
                );
                ui.label(
                    RichText::new(scene_name)
                        .size(13.5)
                        .strong()
                        .color(theme::INK),
                );

                if app.settings.studio.dual_output_enabled {
                    ui.add_space(12.0);
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
                    if !app.undo_stack.is_empty() && ui.button("↶ Desfazer").clicked() {
                        app.undo();
                    }
                });
            });

            ui.add_space(6.0);

            let stage_area_h = (ui.available_height() - 22.0).max(160.0);
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
                        let vert_w = (stage_area_h * 9.0 / 16.0 + 18.0).clamp(180.0, 320.0);
                        let horiz_w = (ui.available_width() - vert_w - 10.0).max(240.0);

                        ui.allocate_ui(Vec2::new(horiz_w, stage_area_h), |ui| {
                            show_stage_viewport(
                                ui,
                                "stage_h",
                                "16:9 HORIZONTAL",
                                if !app.edit_vertical {
                                    theme::SPEC_AMBER
                                } else {
                                    theme::GLASS_EDGE_STRONG
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
                                    theme::GLASS_EDGE_STRONG
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
                    .color(theme::DIM),
                );
            });
        });

    // Modal de filtros DSP de áudio quando aberto
    if let Some(ch_id) = app.filter_channel_id.clone() {
        show_audio_filters_modal(app, ctx, &ch_id);
    }
}

// ------------------------------------------------------------------ Dock: Cenas

fn show_scenes_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("CENAS")
                .monospace()
                .size(11.0)
                .strong()
                .color(theme::MUTED),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Seletor de transição compacto
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
        .max_height((ui.available_height() - 34.0).max(80.0))
        .show(ui, |ui| {
            for (idx, scene) in app.settings.scenes.scenes.iter().enumerate() {
                let is_selected = if studio_mode {
                    scene.id == preview_id
                } else {
                    scene.id == active_id
                };
                let is_on_air = scene.id == active_id;

                theme::glass_card_frame(is_selected).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let tag = if studio_mode && is_on_air {
                            format!("{} · AR", idx + 1)
                        } else {
                            format!("{}", idx + 1)
                        };
                        ui.label(
                            RichText::new(tag)
                                .monospace()
                                .size(10.5)
                                .color(if is_on_air { theme::SPEC_RED } else { theme::DIM }),
                        );
                        if ui
                            .add(
                                egui::Label::new(
                                    RichText::new(&scene.name)
                                        .size(12.5)
                                        .color(if is_selected { theme::INK } else { theme::MUTED }),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .clicked()
                        {
                            select_scene = Some(scene.id.clone());
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if app.settings.scenes.scenes.len() > 1
                                && ui
                                    .small_button(RichText::new("🗑").color(theme::DIM))
                                    .clicked()
                            {
                                delete_scene = Some(scene.id.clone());
                            }
                        });
                    });
                });
                ui.add_space(3.0);
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
                        .desired_width(120.0)
                        .hint_text("Nome da cena"),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.small_button("Criar").clicked() || enter)
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
                if ui.small_button("✕").clicked() {
                    app.adding_scene = false;
                    app.new_scene_name.clear();
                }
            } else {
                if ui.small_button("+ Nova cena").clicked() {
                    app.adding_scene = true;
                    app.new_scene_name =
                        format!("Cena {}", app.settings.scenes.scenes.len() + 1);
                }
                ui.add(
                    egui::DragValue::new(&mut app.settings.scenes.transition_duration_ms)
                        .range(50..=3000)
                        .speed(25)
                        .suffix(" ms"),
                );
            }
        });
    });
}

// ------------------------------------------------------------------ Dock: Fontes

fn show_sources_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("FONTES")
                .monospace()
                .size(11.0)
                .strong()
                .color(theme::MUTED),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let count = app.editing_scene().map(|s| s.items.len()).unwrap_or(0);
            ui.label(
                RichText::new(format!("{count} itens"))
                    .monospace()
                    .size(10.5)
                    .color(theme::DIM),
            );
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

    let mut toggle_vis: Option<String> = None;
    let mut toggle_lock: Option<String> = None;
    let mut remove_id: Option<String> = None;
    let mut move_delta: Option<(String, i32)> = None;

    egui::ScrollArea::vertical()
        .id_salt("sources_scroll")
        .max_height((ui.available_height() - 34.0).max(80.0))
        .show(ui, |ui| {
            if items_snapshot.is_empty() {
                ui.label(
                    RichText::new("Nenhuma fonte nesta cena ainda.")
                        .size(12.0)
                        .color(theme::DIM),
                );
            }
            for item in &items_snapshot {
                let selected = app.selected_item_id.as_deref() == Some(item.id.as_str());
                let status = capture_key_for_item(item)
                    .and_then(|k| app.capture_statuses.get(&k))
                    .copied();

                theme::glass_card_frame(selected).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Ponto de status nativo (verde = live, âmbar = fechada, cinza = estática)
                        let dot_col = match (item.source_type, status) {
                            (
                                SourceType::ScreenCapture | SourceType::WindowCapture,
                                Some(CaptureStatus::Ok),
                            ) => theme::METER_OK,
                            (
                                SourceType::ScreenCapture | SourceType::WindowCapture,
                                Some(CaptureStatus::Missing | CaptureStatus::Error),
                            ) => theme::SPEC_AMBER,
                            _ => theme::SPEC_TEAL,
                        };
                        let (drect, _) =
                            ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                        ui.painter().circle_filled(drect.center(), 3.2, dot_col);

                        ui.label(
                            RichText::new(source_type_badge(item.source_type))
                                .monospace()
                                .size(9.5)
                                .color(theme::DIM),
                        );

                        let mut label = item.name.clone();
                        if label.chars().count() > 18 {
                            label = format!("{}…", label.chars().take(17).collect::<String>());
                        }
                        if ui
                            .add(
                                egui::Label::new(
                                    RichText::new(label)
                                        .size(12.2)
                                        .color(if !item.visible {
                                            theme::DIM
                                        } else if selected {
                                            theme::INK
                                        } else {
                                            theme::MUTED
                                        }),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .clicked()
                        {
                            app.selected_item_id = Some(item.id.clone());
                            app.right_tab = RightTab::Properties;
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            if ui
                                .small_button(RichText::new("🗑").color(theme::DIM))
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
                            if ui.small_button("↑").clicked() {
                                move_delta = Some((item.id.clone(), 1));
                            }
                            if ui.small_button("↓").clicked() {
                                move_delta = Some((item.id.clone(), -1));
                            }
                        });
                    });
                });
                ui.add_space(3.0);
            }
        });

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
            if ui.small_button("+ Tela").clicked() {
                app.picker
                    .open(PickerPurpose::AddNewSource, SourceKind::Screen, None);
            }
            if ui.small_button("+ Janela").clicked() {
                app.picker
                    .open(PickerPurpose::AddNewSource, SourceKind::Window, None);
            }
            ui.menu_button("+ Outros ▾", |ui| {
                if ui.button("Imagem (PNG / JPG / WebP)").clicked() {
                    app.add_simple_source(SourceType::ImageSource);
                    ui.close_menu();
                }
                if ui.button("Cor sólida").clicked() {
                    app.add_simple_source(SourceType::ColorSource);
                    ui.close_menu();
                }
                if ui.button("Texto").clicked() {
                    app.add_simple_source(SourceType::TextSource);
                    ui.close_menu();
                }
                if ui.button("Widget HTML / URL").clicked() {
                    app.add_simple_source(SourceType::BrowserSource);
                    ui.close_menu();
                }
                if ui.button("Câmera").clicked() {
                    app.add_simple_source(SourceType::CameraInput);
                    ui.close_menu();
                }
            });
        });
    });
}

// ------------------------------------------------------------------ Dock: Mixer de Áudio

fn show_mixer_dock(app: &mut PrismCastNativeApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("MIXER DE ÁUDIO · 48 KHZ DSP")
                .monospace()
                .size(11.0)
                .strong()
                .color(theme::MUTED),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("+ Canal App").clicked() {
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
        .max_height((ui.available_height() - 6.0).max(110.0))
        .show(ui, |ui| {
            for ch in &mut app.settings.audio_mixer.channels {
                let peak = app
                    .audio_levels
                    .get(&ch.id)
                    .map(|l| l.peak_db)
                    .unwrap_or(-60.0);

                theme::glass_card_frame(false).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&ch.name).size(12.5).strong().color(theme::INK));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ch.kind == AudioChannelKind::App
                                && ui
                                    .small_button(RichText::new("🗑").color(theme::DIM))
                                    .clicked()
                            {
                                remove_ch = Some(ch.id.clone());
                            }
                            if ui
                                .small_button("DSP")
                                .on_hover_text("Filtros DSP, EQ 3 bandas, Compressor e VST3/CLAP")
                                .clicked()
                            {
                                open_filters = Some(ch.id.clone());
                            }
                            let mute_btn = if ch.muted {
                                egui::Button::new(
                                    RichText::new("Mudo").size(11.0).color(theme::INK),
                                )
                                .fill(theme::TALLY)
                            } else {
                                egui::Button::new(
                                    RichText::new("🔊").size(11.0).color(theme::MUTED),
                                )
                            };
                            if ui.add(mute_btn).clicked() {
                                ch.muted = !ch.muted;
                                dirty = true;
                            }
                            ui.label(
                                RichText::new(format!("{:+.1} dB", ch.volume_db))
                                    .monospace()
                                    .size(11.0)
                                    .color(theme::MUTED),
                            );
                        });
                    });
                    theme::paint_vu_meter(ui, peak, ch.muted, avail_w - 36.0);
                    let sl = ui.add(
                        egui::Slider::new(&mut ch.volume_db, -60.0..=12.0)
                            .show_value(false)
                            .trailing_fill(true),
                    );
                    if sl.changed() {
                        dirty = true;
                    }
                });
                ui.add_space(4.0);
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
    let mut open = true;
    let mut dirty = false;

    egui::Window::new("Filtros DSP do Canal")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_width(380.0)
        .show(ctx, |ui| {
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

            ui.label(RichText::new(&ch.name).size(15.0).strong().color(theme::INK));
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
                            if ui.selectable_value(&mut ch.monitor, m, lbl).clicked() {
                                dirty = true;
                            }
                        }
                    });
            });

            if ui
                .add(egui::Slider::new(&mut ch.filters.highpass_hz, 0.0..=300.0).text("Passa-alta (Hz)"))
                .changed()
            {
                dirty = true;
            }
            if ui
                .add(egui::Slider::new(&mut ch.filters.gain_db, -24.0..=24.0).text("Ganho (dB)"))
                .changed()
            {
                dirty = true;
            }

            ui.separator();
            if ui
                .checkbox(&mut ch.filters.gate.enabled, "Noise Gate")
                .changed()
            {
                dirty = true;
            }
            if ch.filters.gate.enabled
                && ui
                    .add(
                        egui::Slider::new(&mut ch.filters.gate.threshold_db, -80.0..=0.0)
                            .text("Limiar Gate (dB)"),
                    )
                    .changed()
            {
                dirty = true;
            }

            if ui
                .checkbox(&mut ch.filters.compressor.enabled, "Compressor")
                .changed()
            {
                dirty = true;
            }
            if ch.filters.compressor.enabled {
                if ui
                    .add(
                        egui::Slider::new(&mut ch.filters.compressor.threshold_db, -60.0..=0.0)
                            .text("Threshold (dB)"),
                    )
                    .changed()
                {
                    dirty = true;
                }
                if ui
                    .add(
                        egui::Slider::new(&mut ch.filters.compressor.ratio, 1.0..=16.0)
                            .text("Ratio"),
                    )
                    .changed()
                {
                    dirty = true;
                }
            }

            if ui
                .checkbox(&mut ch.filters.eq_enabled, "Equalizador 3 bandas")
                .changed()
            {
                dirty = true;
            }
            if ch.filters.eq_enabled {
                if ui
                    .add(egui::Slider::new(&mut ch.filters.eq_low_db, -12.0..=12.0).text("Graves"))
                    .changed()
                {
                    dirty = true;
                }
                if ui
                    .add(egui::Slider::new(&mut ch.filters.eq_mid_db, -12.0..=12.0).text("Médios"))
                    .changed()
                {
                    dirty = true;
                }
                if ui
                    .add(egui::Slider::new(&mut ch.filters.eq_high_db, -12.0..=12.0).text("Agudos"))
                    .changed()
                {
                    dirty = true;
                }
            }

            if ui
                .checkbox(&mut ch.filters.limiter.enabled, "Limitador")
                .changed()
            {
                dirty = true;
            }

            if ui
                .add(
                    egui::Slider::new(&mut ch.sync_offset_ms, -500..=500)
                        .text("Atraso de sincronia (ms)"),
                )
                .changed()
            {
                dirty = true;
            }
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
    let Some(sel_id) = app.selected_item_id.clone() else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Nenhuma fonte selecionada")
                    .size(13.5)
                    .strong()
                    .color(theme::MUTED),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Clique em uma fonte no palco ou na lista inferior para editar posição, recorte e captura.")
                    .size(11.5)
                    .color(theme::DIM),
            );
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
                        .color(theme::INK),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(source_type_badge(item_snapshot.source_type))
                            .monospace()
                            .size(10.0)
                            .color(theme::SPEC_TEAL),
                    );
                });
            });

            ui.add_space(6.0);

            // Seção específica por tipo de fonte
            if matches!(
                item_snapshot.source_type,
                SourceType::ScreenCapture | SourceType::WindowCapture
            ) {
                theme::glass_card_frame(false).show(ui, |ui| {
                    let status_text = match status {
                        Some(CaptureStatus::Ok) => ("Capturando nativamente", theme::METER_OK),
                        Some(CaptureStatus::Missing) => {
                            ("Janela fechada — reabra ou escolha outra", theme::SPEC_AMBER)
                        }
                        Some(CaptureStatus::Error) => ("Erro na captura", theme::SPEC_RED),
                        _ => ("Conectando captura nativa…", theme::MUTED),
                    };
                    ui.label(RichText::new(status_text.0).size(11.5).color(status_text.1));
                    ui.add_space(6.0);
                    if theme::tally_button(ui, "Escolher tela / janela na lista").clicked() {
                        open_picker = true;
                    }
                    if item_snapshot.source_type == SourceType::WindowCapture {
                        ui.add_space(6.0);
                        let has_audio = item_snapshot.settings.audio_channel_id.is_some();
                        if ui
                            .button(if has_audio {
                                "✓ Áudio desta janela vinculado"
                            } else {
                                "+ Capturar o áudio desta janela"
                            })
                            .clicked()
                            && !has_audio
                        {
                            add_window_audio = true;
                        }
                    }
                });
            } else if item_snapshot.source_type == SourceType::ImageSource {
                theme::glass_card_frame(false).show(ui, |ui| {
                    let cur = item_snapshot
                        .settings
                        .path
                        .as_deref()
                        .unwrap_or("Nenhuma imagem");
                    ui.label(RichText::new(cur).size(11.0).color(theme::MUTED));
                    ui.add_space(4.0);
                    if ui.button("Trocar arquivo de imagem…").clicked() {
                        pick_image = true;
                    }
                });
            }

            ui.add_space(8.0);

            // Edição direta do item na cena
            if let Some(sc) = app.editing_scene_mut() {
                if let Some(it) = sc.items.iter_mut().find(|i| i.id == sel_id) {
                    if it.source_type == SourceType::ColorSource {
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
                        ui.add_space(6.0);
                    }

                    if it.source_type == SourceType::BrowserSource {
                        let mut url = it.settings.url.clone().unwrap_or_default();
                        ui.label("URL do Widget:");
                        if ui.text_edit_singleline(&mut url).changed() {
                            it.settings.url = Some(url);
                            dirty = true;
                        }
                        ui.add_space(6.0);
                    }

                    let tf = if vertical {
                        &mut it.vertical_transform
                    } else {
                        &mut it.transform
                    };
                    let (cw, ch) = if vertical { (1080.0, 1920.0) } else { (1920.0, 1080.0) };

                    ui.label(
                        RichText::new(if vertical {
                            "POSIÇÃO NO 9:16"
                        } else {
                            "POSIÇÃO NO 16:9"
                        })
                        .monospace()
                        .size(10.5)
                        .color(theme::MUTED),
                    );
                    ui.add_space(4.0);

                    let mut px_x = (tf.x * cw).round() as i32;
                    let mut px_y = (tf.y * ch).round() as i32;
                    let mut px_w = (tf.width * cw).round() as i32;
                    let mut px_h = (tf.height * ch).round() as i32;

                    egui::Grid::new("tf_grid")
                        .num_columns(2)
                        .spacing(Vec2::new(8.0, 6.0))
                        .show(ui, |ui| {
                            ui.label("X");
                            if ui.add(egui::DragValue::new(&mut px_x).speed(2)).changed() {
                                tf.x = px_x as f32 / cw;
                                dirty = true;
                            }
                            ui.end_row();
                            ui.label("Y");
                            if ui.add(egui::DragValue::new(&mut px_y).speed(2)).changed() {
                                tf.y = px_y as f32 / ch;
                                dirty = true;
                            }
                            ui.end_row();
                            ui.label("Largura");
                            if ui
                                .add(egui::DragValue::new(&mut px_w).range(32..=7680).speed(4))
                                .changed()
                            {
                                tf.width = px_w as f32 / cw;
                                dirty = true;
                            }
                            ui.end_row();
                            ui.label("Altura");
                            if ui
                                .add(egui::DragValue::new(&mut px_h).range(32..=7680).speed(4))
                                .changed()
                            {
                                tf.height = px_h as f32 / ch;
                                dirty = true;
                            }
                            ui.end_row();
                        });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Tela cheia").clicked() {
                            tf.x = 0.0;
                            tf.y = 0.0;
                            tf.width = 1.0;
                            tf.height = 1.0;
                            dirty = true;
                        }
                        if ui.button("Centralizar").clicked() {
                            tf.x = (1.0 - tf.width) * 0.5;
                            tf.y = (1.0 - tf.height) * 0.5;
                            dirty = true;
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("RECORTE (%) · ALT + ARRASTAR NO PALCO")
                            .monospace()
                            .size(10.5)
                            .color(theme::MUTED),
                    );
                    let mut ct = (tf.crop_top.unwrap_or(0.0) * 100.0).round();
                    let mut cb = (tf.crop_bottom.unwrap_or(0.0) * 100.0).round();
                    let mut cl = (tf.crop_left.unwrap_or(0.0) * 100.0).round();
                    let mut cr = (tf.crop_right.unwrap_or(0.0) * 100.0).round();

                    egui::Grid::new("crop_grid")
                        .num_columns(4)
                        .spacing(Vec2::new(6.0, 4.0))
                        .show(ui, |ui| {
                            ui.label("Topo");
                            if ui.add(egui::DragValue::new(&mut ct).range(0.0..=85.0).suffix("%")).changed() {
                                tf.crop_top = Some(ct / 100.0);
                                dirty = true;
                            }
                            ui.label("Base");
                            if ui.add(egui::DragValue::new(&mut cb).range(0.0..=85.0).suffix("%")).changed() {
                                tf.crop_bottom = Some(cb / 100.0);
                                dirty = true;
                            }
                            ui.end_row();
                            ui.label("Esq");
                            if ui.add(egui::DragValue::new(&mut cl).range(0.0..=85.0).suffix("%")).changed() {
                                tf.crop_left = Some(cl / 100.0);
                                dirty = true;
                            }
                            ui.label("Dir");
                            if ui.add(egui::DragValue::new(&mut cr).range(0.0..=85.0).suffix("%")).changed() {
                                tf.crop_right = Some(cr / 100.0);
                                dirty = true;
                            }
                            ui.end_row();
                        });

                    ui.add_space(8.0);
                    if ui
                        .add(egui::Slider::new(&mut tf.opacity, 0.0..=1.0).text("Opacidade"))
                        .changed()
                    {
                        dirty = true;
                    }
                    if ui.checkbox(&mut tf.flip_x, "Espelhar horizontal").changed() {
                        dirty = true;
                    }
                    let mut rounded = it.settings.rounded.unwrap_or(false);
                    if ui.checkbox(&mut rounded, "Cantos arredondados").changed() {
                        it.settings.rounded = Some(rounded);
                        dirty = true;
                    }
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
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("MULTICHAT AO VIVO")
                .monospace()
                .size(10.5)
                .color(theme::MUTED),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("↻ Reconectar").clicked() {
                let engine = app.engine.clone();
                app.rt.spawn(async move {
                    let _ = engine.handle_command("multichat_reconnect", Value::Null).await;
                });
            }
        });
    });

    ui.add_space(6.0);

    let feed_h = (ui.available_height() - 42.0).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("multichat_feed")
        .stick_to_bottom(true)
        .max_height(feed_h)
        .show(ui, |ui| {
            if app.chat_messages.is_empty() {
                ui.add_space(20.0);
                ui.label(
                    RichText::new(
                        "Conecte Twitch, YouTube ou Kick em Destinos/Ajustes para ver o chat unificado aqui.",
                    )
                    .size(12.0)
                    .color(theme::DIM),
                );
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
                            .color(theme::INK),
                    );
                    ui.label(RichText::new(text_body).size(12.0).color(theme::MUTED));
                });
                ui.add_space(3.0);
            }
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.chat_input)
                .desired_width(ui.available_width() - 68.0)
                .hint_text("Enviar no chat Twitch…"),
        );
        let send = (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            || ui.button("Enviar").clicked();
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

pub fn show_destinations_screen(app: &mut PrismCastNativeApp, ctx: &Context) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new("Destinos de Transmissão").size(22.0).strong().color(theme::INK));
                ui.label(
                    RichText::new(
                        "Transmita simultaneamente em 16:9 e 9:16 com chaves protegidas no cofre nativo do sistema.",
                    )
                    .size(12.5)
                    .color(theme::MUTED),
                );
                ui.add_space(14.0);

                // Card OAuth Device Flow da Twitch
                theme::glass_panel_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Conta Twitch (OAuth Device Code)").size(14.5).strong());
                            if app.twitch_caps.linked {
                                ui.label(
                                    RichText::new(format!(
                                        "Conectado como @{}",
                                        app.twitch_caps.login.as_deref().unwrap_or("twitch")
                                    ))
                                    .color(theme::METER_OK),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Conecte para atualizar título/categoria e usar o chat.")
                                        .size(12.0)
                                        .color(theme::MUTED),
                                );
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !app.twitch_caps.linked {
                                if theme::tally_button(ui, "Entrar com a Twitch").clicked() {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let res = engine
                                            .handle_command(
                                                "dest_twitch_start",
                                                json!({ "destinationId": "twitch" }),
                                            )
                                            .await;
                                        if let Ok(st) = serde_json::from_value::<TwitchLoginState>(res) {
                                            let _ = tx.send(BgUiMsg::TwitchLogin(st));
                                        }
                                    });
                                }
                            }
                        });
                    });

                    if let TwitchLoginState::Code {
                        user_code,
                        verification_uri,
                        ..
                    } = &app.twitch_login
                    {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Código:");
                            ui.label(
                                RichText::new(user_code)
                                    .monospace()
                                    .size(16.0)
                                    .strong()
                                    .color(theme::SPEC_AMBER),
                            );
                            if ui.button("Abrir twitch.tv/activate").clicked() {
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
                    theme::glass_panel_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut dest.enabled, "").changed() {
                                dirty = true;
                            }
                            ui.label(RichText::new(&dest.name).size(15.0).strong().color(theme::INK));
                            let orient_lbl = match dest.orientation {
                                CanvasOrientation::Horizontal => "16:9",
                                CanvasOrientation::Vertical => "9:16",
                            };
                            if ui.small_button(orient_lbl).clicked() {
                                dest.orientation = match dest.orientation {
                                    CanvasOrientation::Horizontal => CanvasOrientation::Vertical,
                                    CanvasOrientation::Vertical => CanvasOrientation::Horizontal,
                                };
                                dirty = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(if dest.has_key {
                                        "✓ Chave salva no cofre"
                                    } else {
                                        "Sem chave"
                                    })
                                    .size(11.5)
                                    .color(if dest.has_key {
                                        theme::METER_OK
                                    } else {
                                        theme::DIM
                                    }),
                                );
                            });
                        });

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Servidor:");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut dest.server)
                                        .desired_width(300.0),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
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

                        ui.add_space(4.0);
                        let key_buf = app.dest_key_inputs.entry(dest.id.clone()).or_default();
                        ui.horizontal(|ui| {
                            ui.label("Chave RTMP:");
                            ui.add(
                                egui::TextEdit::singleline(key_buf)
                                    .password(true)
                                    .desired_width(280.0)
                                    .hint_text(if dest.has_key {
                                        "•••••••••••• (substituir)"
                                    } else {
                                        "Cole sua stream key aqui"
                                    }),
                            );
                            if ui.button("Salvar no cofre").clicked() && !key_buf.trim().is_empty() {
                                save_key_req = Some((dest.id.clone(), key_buf.trim().to_string()));
                                key_buf.clear();
                            }
                        });
                    });
                    ui.add_space(8.0);
                }

                // Adicionar RTMP customizado
                theme::glass_card_frame(false).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("+ Novo destino RTMP:").strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut app.new_dest_name)
                                .desired_width(140.0)
                                .hint_text("Nome"),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut app.new_dest_server)
                                .desired_width(240.0)
                                .hint_text("rtmp://servidor/app"),
                        );
                        if ui.button("Adicionar").clicked() && !app.new_dest_name.trim().is_empty() {
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

pub fn show_widgets_screen(app: &mut PrismCastNativeApp, ctx: &Context) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new("Widgets & Kit de Overlays").size(22.0).strong().color(theme::INK));
                ui.label(
                    RichText::new(
                        "Carregue sua pasta de overlays HTML com parsing estático seguro em Rust (sem eval).",
                    )
                    .size(12.5)
                    .color(theme::MUTED),
                );
                ui.add_space(14.0);

                theme::glass_panel_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let cur = app
                            .settings
                            .kit_path
                            .as_deref()
                            .unwrap_or("Nenhuma pasta selecionada");
                        ui.label(RichText::new("Pasta do kit:").strong());
                        ui.label(RichText::new(cur).monospace().size(11.5).color(theme::MUTED));
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

                ui.add_space(12.0);
                if let Some(ref kit) = app.loaded_kit {
                    ui.label(
                        RichText::new(format!("{} ({} widgets)", kit.name, kit.widgets.len()))
                            .size(15.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    for w in &kit.widgets {
                        theme::glass_card_frame(false).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&w.name).strong().color(theme::INK));
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · {}×{}",
                                            w.description, w.width, w.height
                                        ))
                                        .size(11.5)
                                        .color(theme::MUTED),
                                    );
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                } else {
                    theme::glass_card_frame(false).show(ui, |ui| {
                        ui.label(
                            RichText::new("Selecione uma pasta de kit para listar overlays, alertas e telas de pausa.")
                                .color(theme::MUTED),
                        );
                    });
                }
            });
        });
}

// ------------------------------------------------------------------ Tela: Rede Local (LAN P2P)

pub fn show_lan_screen(app: &mut PrismCastNativeApp, ctx: &Context) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new("Rede Local (LAN P2P)").size(22.0).strong().color(theme::INK));
                ui.label(
                    RichText::new(
                        "Compartilhe a saída do estúdio em ultra-baixa latência na rede local com descoberta UDP e PIN.",
                    )
                    .size(12.5)
                    .color(theme::MUTED),
                );
                ui.add_space(14.0);

                theme::glass_panel_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Transmitir na rede local (Host)").size(15.0).strong());
                            if app.lan_hosting {
                                ui.label(
                                    RichText::new(format!(
                                        "Ativo em {}:{} · PIN: {}",
                                        app.lan_ip,
                                        app.lan_port,
                                        app.lan_pin.as_deref().unwrap_or("livre")
                                    ))
                                    .monospace()
                                    .color(theme::SPEC_TEAL),
                                );
                            } else {
                                ui.label(RichText::new("Parado").color(theme::MUTED));
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if app.lan_hosting {
                                if ui.button("Parar Host LAN").clicked() {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let _ = engine.handle_command("host_stop", Value::Null).await;
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
                                        let ip = res.get("ip").and_then(|v| v.as_str()).unwrap_or("127.0.0.1").to_string();
                                        let port = res.get("port").and_then(|v| v.as_u64()).unwrap_or(41235) as u16;
                                        let pin = res.get("pin").and_then(|v| v.as_str()).map(str::to_string);
                                        let _ = tx.send(BgUiMsg::LanHostStarted { ip, port, pin });
                                    }
                                });
                            }
                        });
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(&mut app.settings.require_pin, "Exigir código PIN (6 dígitos)")
                            .changed()
                        {
                            app.persist_settings(json!({ "requirePin": app.settings.require_pin }));
                        }
                        if ui.button("Autorizar Firewall").clicked() {
                            let engine = app.engine.clone();
                            let tx = app.bg_tx.clone();
                            app.rt.spawn(async move {
                                let res = engine.handle_command("firewall_authorize", Value::Null).await;
                                let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                                let _ = tx.send(BgUiMsg::FirewallChecked(ok));
                                let _ = tx.send(BgUiMsg::Toast {
                                    message: if ok {
                                        "Regras de firewall verificadas.".to_string()
                                    } else {
                                        "Não foi possível autorizar o firewall automaticamente.".to_string()
                                    },
                                    is_error: !ok,
                                });
                            });
                        }
                    });
                });

                ui.add_space(12.0);
                theme::glass_panel_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Computadores encontrados na LAN").size(14.5).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("↻ Procurar na rede").clicked() {
                                let engine = app.engine.clone();
                                let tx = app.bg_tx.clone();
                                app.rt.spawn(async move {
                                    let _ = engine.handle_command("discovery_start", Value::Null).await;
                                    let res = engine.handle_command("discovery_refresh", Value::Null).await;
                                    if let Ok(hosts) = serde_json::from_value::<Vec<DiscoveredHost>>(res) {
                                        let _ = tx.send(BgUiMsg::DiscoveredHosts(hosts));
                                    }
                                });
                            }
                        });
                    });
                    ui.add_space(6.0);
                    if app.lan_hosts.is_empty() {
                        ui.label(RichText::new("Nenhum host PrismCast detectado nesta rede.").color(theme::DIM));
                    } else {
                        for h in &app.lan_hosts {
                            theme::glass_card_frame(false).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&h.name).strong());
                                    ui.label(
                                        RichText::new(format!("{}:{}", h.ip, h.port))
                                            .monospace()
                                            .color(theme::SPEC_TEAL),
                                    );
                                    ui.label(RichText::new(&h.source_name).color(theme::MUTED));
                                });
                            });
                            ui.add_space(4.0);
                        }
                    }
                });
            });
        });
}

// ------------------------------------------------------------------ Tela: Ajustes

pub fn show_settings_screen(app: &mut PrismCastNativeApp, ctx: &Context) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(Color32::TRANSPARENT).inner_margin(Margin::same(18)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new("Ajustes do Estúdio").size(22.0).strong().color(theme::INK));
                ui.add_space(12.0);

                // 1. Pipeline GPU Zero-Copy & Vídeo
                theme::glass_panel_frame().show(ui, |ui| {
                    ui.label(RichText::new("Vídeo & Pipeline GPU Zero-Copy").size(15.0).strong());
                    ui.add_space(6.0);
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
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(format!(
                                "Backend: {} · Adaptador: {}",
                                app.gpu_status.compositor_backend, app.gpu_status.adapter_name
                            ))
                            .monospace()
                            .size(11.0)
                            .color(theme::SPEC_TEAL),
                        );
                    });
                    ui.add_space(6.0);
                    let mut gpu_dirty = false;
                    if ui
                        .checkbox(
                            &mut app.settings.gpu_pipeline.zero_copy_enabled,
                            "Ativar captura GPU Zero-Copy (DXGI / PipeWire)",
                        )
                        .changed()
                    {
                        gpu_dirty = true;
                    }
                    if ui
                        .checkbox(
                            &mut app.settings.gpu_pipeline.prefer_wgpu,
                            "Usar compositor acelerado por GPU (wgpu 24)",
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

                ui.add_space(10.0);

                // 2. Câmera Virtual Nativa
                theme::glass_panel_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Câmera Virtual Nativa").size(15.0).strong());
                            ui.label(
                                RichText::new(format!(
                                    "Status: {} · Frames: {}",
                                    if app.virtual_cam.active { "Ativa" } else { "Parada" },
                                    app.virtual_cam.frames_sent
                                ))
                                .monospace()
                                .size(11.0)
                                .color(theme::MUTED),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if app.virtual_cam.active {
                                if ui.button("Parar Câmera Virtual").clicked() {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let res = engine.handle_command("virtual_cam_stop", Value::Null).await;
                                        if let Ok(st) = serde_json::from_value(res) {
                                            let _ = tx.send(BgUiMsg::VirtualCamStatus(st));
                                        }
                                    });
                                }
                            } else if theme::tally_button(ui, "Iniciar Câmera Virtual").clicked() {
                                let engine = app.engine.clone();
                                let tx = app.bg_tx.clone();
                                let orient = app.settings.virtual_camera.orientation;
                                app.rt.spawn(async move {
                                    let o = match orient {
                                        CanvasOrientation::Horizontal => "horizontal",
                                        CanvasOrientation::Vertical => "vertical",
                                    };
                                    let res = engine
                                        .handle_command("virtual_cam_start", json!({ "orientation": o }))
                                        .await;
                                    if let Ok(st) = serde_json::from_value(res) {
                                        let _ = tx.send(BgUiMsg::VirtualCamStatus(st));
                                    }
                                });
                            }
                        });
                    });
                });

                ui.add_space(10.0);

                // 3. Controle Externo (obs-websocket v5 / Stream Deck)
                theme::glass_panel_frame().show(ui, |ui| {
                    ui.label(RichText::new("Controladores Externos (obs-websocket v5 / Stream Deck)").size(15.0).strong());
                    ui.add_space(6.0);
                    let mut ext_dirty = false;
                    ui.horizontal(|ui| {
                        if ui
                            .checkbox(
                                &mut app.settings.external_control.obs_ws_enabled,
                                "Ativar servidor obs-websocket v5",
                            )
                            .changed()
                        {
                            ext_dirty = true;
                        }
                        ui.label("Porta:");
                        if ui
                            .add(
                                egui::DragValue::new(&mut app.settings.external_control.obs_ws_port)
                                    .range(1024..=65535),
                            )
                            .changed()
                        {
                            ext_dirty = true;
                        }
                        ui.label(
                            RichText::new(if app.external_ctrl.obs_ws_running {
                                format!("● Rodando ({} clientes)", app.external_ctrl.obs_ws_clients)
                            } else {
                                "○ Parado".to_string()
                            })
                            .monospace()
                            .size(11.0)
                            .color(if app.external_ctrl.obs_ws_running {
                                theme::METER_OK
                            } else {
                                theme::DIM
                            }),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Senha obs-websocket:");
                        ui.add(
                            egui::TextEdit::singleline(&mut app.obs_password_input)
                                .password(true)
                                .desired_width(200.0)
                                .hint_text("Definir nova senha"),
                        );
                        if ui.button("Salvar senha").clicked() && !app.obs_password_input.trim().is_empty() {
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

                ui.add_space(10.0);

                // 4. Atualizador Automático (GitHub Releases)
                theme::glass_panel_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Atualizações (GitHub Releases)").size(15.0).strong());
                            ui.label(
                                RichText::new(format!(
                                    "Versão instalada: v{} · Modo: {}",
                                    app.updater.current_version, app.updater.install_kind
                                ))
                                .monospace()
                                .size(11.0)
                                .color(theme::MUTED),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Verificar atualizações").clicked() {
                                let engine = app.engine.clone();
                                let tx = app.bg_tx.clone();
                                app.rt.spawn(async move {
                                    let res = engine.handle_command("updater_check", Value::Null).await;
                                    if let Ok(st) = serde_json::from_value(res) {
                                        let _ = tx.send(BgUiMsg::UpdateStatus(st));
                                    }
                                });
                            }
                            if app.updater.state == UpdateState::Available {
                                if theme::tally_button(ui, "Baixar atualização").clicked() {
                                    let engine = app.engine.clone();
                                    app.rt.spawn(async move {
                                        let _ = engine.handle_command("updater_download", Value::Null).await;
                                    });
                                }
                            } else if app.updater.state == UpdateState::Ready {
                                if theme::tally_button(ui, "Instalar e reiniciar").clicked() {
                                    let engine = app.engine.clone();
                                    let tx = app.bg_tx.clone();
                                    app.rt.spawn(async move {
                                        let res = engine.handle_command("updater_install", Value::Null).await;
                                        if res.get("shouldExit").and_then(|v| v.as_bool()) == Some(true) {
                                            let _ = tx.send(BgUiMsg::ExitRequested);
                                        }
                                    });
                                }
                            }
                        });
                    });

                    if app.updater.state == UpdateState::Downloading {
                        ui.add_space(6.0);
                        ui.add(
                            egui::ProgressBar::new((app.updater.progress_percent / 100.0).clamp(0.0, 1.0))
                                .text(format!("{:.0}%", app.updater.progress_percent)),
                        );
                    }
                    if let Some(ref err) = app.updater.error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(err).color(theme::SPEC_RED).size(11.5));
                    }
                });
            });
        });
}

// ------------------------------------------------------------------ Modais Auxiliares

pub fn show_confirm_quit_modal(app: &mut PrismCastNativeApp, ctx: &Context) {
    egui::Window::new("Encerrar PrismCast Studio?")
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(
                    "Você está transmitindo ou gravando no momento. Deseja realmente encerrar o estúdio?",
                )
                .color(theme::INK),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Continuar no ar").clicked() {
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
    egui::Area::new(egui::Id::new("prismcast_toast"))
        .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -24.0))
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            Frame::new()
                .fill(theme::GLASS_MODAL)
                .stroke(Stroke::new(
                    1.2_f32,
                    if toast.is_error {
                        theme::TALLY
                    } else {
                        theme::SPEC_TEAL
                    },
                ))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.label(RichText::new(&toast.message).size(12.5).color(theme::INK));
                });
        });
}

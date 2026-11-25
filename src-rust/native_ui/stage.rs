//! Palco interativo nativo (`16:9`, `9:16` Dual Output e Modo Estúdio) estilizado com `egui-prismglass`.
//!
//! - Exibe a textura GPU ao vivo produzida pelo compositor Rust (`wgpu` / software)
//!   dentro de um monitor broadcast com moldura 3D glassmorphism.
//! - Seleção por clique (ordem Z), arrastar com snap magnético nas bordas/centro,
//!   4 alças de redimensionamento (`Shift` solta proporção, `Alt` recorta),
//!   menu de contexto no botão direito e atalhos de teclado (setas + Delete).
//! - Nunca desenha botão flutuante sobre fontes ativas no palco.

use crate::capture::CaptureStatus;
use crate::compositor::capture_key_for_item;
use crate::native_ui::theme;
use crate::protocol::{Scene, SceneItem, SourceTransform, SourceType};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, Ui, Vec2,
};
use egui_prismglass::{
    GlassButton, GlassCard, GlassElevation, PrismContextExt, paint_glass_surface,
};
use std::collections::HashMap;

const SNAP_THRESHOLD: f32 = 0.012;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CornerHandle {
    Nw,
    Ne,
    Sw,
    Se,
}

#[derive(Debug, Clone)]
enum DragOp {
    Move {
        item_id: String,
        start_rel: Pos2,
        orig: SourceTransform,
    },
    Resize {
        item_id: String,
        corner: CornerHandle,
        start_rel: Pos2,
        orig: SourceTransform,
        crop_mode: bool,
    },
}

#[derive(Debug, Clone)]
pub enum StageAction {
    Select(Option<String>),
    UpdateTransform {
        item_id: String,
        vertical: bool,
        transform: SourceTransform,
        commit_undo: bool,
    },
    BringForward(String),
    SendBackward(String),
    FillCanvas {
        item_id: String,
        vertical: bool,
    },
    CenterCanvas {
        item_id: String,
        vertical: bool,
    },
    ToggleLock(String),
    RemoveItem(String),
    OpenPickerForNewScreen,
    OpenPickerForNewWindow,
    OpenPickerForItem(String),
}

#[derive(Default)]
pub struct StageInteraction {
    drag: Option<DragOp>,
    snap_x: Option<f32>,
    snap_y: Option<f32>,
    hover_id: Option<String>,
}

fn snap_axis(val: f32, targets: &[(f32, f32)]) -> (f32, Option<f32>) {
    for &(pos, guide) in targets {
        if (val - pos).abs() < SNAP_THRESHOLD {
            return (pos, Some(guide));
        }
    }
    (val, None)
}

fn tf_of(item: &SceneItem, vertical: bool) -> &SourceTransform {
    if vertical {
        &item.vertical_transform
    } else {
        &item.transform
    }
}

fn item_rect_in_stage(stage: Rect, tf: &SourceTransform) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            stage.left() + tf.x * stage.width(),
            stage.top() + tf.y * stage.height(),
        ),
        Vec2::new(
            (tf.width * stage.width()).max(8.0),
            (tf.height * stage.height()).max(8.0),
        ),
    )
}

fn corner_pos(r: Rect, c: CornerHandle) -> Pos2 {
    match c {
        CornerHandle::Nw => r.left_top(),
        CornerHandle::Ne => r.right_top(),
        CornerHandle::Sw => r.left_bottom(),
        CornerHandle::Se => r.right_bottom(),
    }
}

fn fit_aspect_rect(container: Rect, aspect: f32) -> Rect {
    let cw = container.width().max(1.0);
    let ch = container.height().max(1.0);
    let c_aspect = cw / ch;
    let (w, h) = if c_aspect > aspect {
        (ch * aspect, ch)
    } else {
        (cw, cw / aspect)
    };
    Rect::from_center_size(container.center(), Vec2::new(w, h))
}

#[allow(clippy::too_many_arguments)]
pub fn show_stage_viewport(
    ui: &mut Ui,
    id_salt: &str,
    label_badge: &str,
    badge_accent: Color32,
    texture: Option<&TextureHandle>,
    scene: Option<&Scene>,
    vertical: bool,
    editable: bool,
    selected_id: Option<&str>,
    capture_statuses: &HashMap<String, CaptureStatus>,
    interaction: &mut StageInteraction,
    actions: &mut Vec<StageAction>,
) {
    let prism = ui.ctx().prism_theme();
    let avail = ui.available_rect_before_wrap();
    let aspect = if vertical { 9.0 / 16.0 } else { 16.0 / 9.0 };
    let stage_rect = fit_aspect_rect(avail.shrink2(Vec2::new(10.0, 10.0)), aspect);

    let resp = ui.allocate_rect(
        avail,
        if editable {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let painter = ui.painter().clone();

    // Moldura 3D glassmorphism (`egui-prismglass`) ao redor do monitor broadcast
    let bezel_rect = stage_rect.expand(5.0);
    let bezel_tokens = prism
        .surface(GlassElevation::Elevated)
        .with_corner_radius((prism.radius.lg * prism.radius_scale).clamp(6.0, 20.0));
    paint_glass_surface(&painter, bezel_rect, &bezel_tokens);

    // Fundo escuro de referência do monitor de vídeo
    let well_cr = CornerRadius::same(((bezel_tokens.corner_radius - 4.0).max(4.0)) as u8);
    painter.rect_filled(stage_rect, well_cr, theme::media_well_fill(&prism));

    // Textura GPU ao vivo do compositor Rust
    if let Some(tex) = texture {
        painter.image(
            tex.id(),
            stage_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // Linha hairline sutil enquadrando a área útil de vídeo
    painter.rect_stroke(
        stage_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18)),
        StrokeKind::Inside,
    );

    let mut visible_items: Vec<&SceneItem> = scene
        .map(|sc| {
            let mut list: Vec<&SceneItem> = sc
                .items
                .iter()
                .filter(|it| {
                    it.visible
                        && it.source_type != SourceType::AudioInput
                        && it.source_type != SourceType::AudioOutput
                        && if vertical {
                            it.show_on_vertical
                        } else {
                            it.show_on_horizontal
                        }
                })
                .collect();
            list.sort_by_key(|it| tf_of(it, vertical).z_index);
            list
        })
        .unwrap_or_default();

    let has_any_visual_source = visible_items.iter().any(|it| match it.source_type {
        SourceType::ColorSource | SourceType::TextSource | SourceType::BrowserSource => true,
        _ => {
            capture_key_for_item(it)
                .and_then(|k| capture_statuses.get(&k))
                .copied()
                == Some(CaptureStatus::Ok)
        }
    });

    // Só exibe prompt vazio se a CENA INTEIRA não tiver nenhuma fonte visual ativa
    if editable && !has_any_visual_source && visible_items.is_empty() {
        let card_w = (stage_rect.width() - 32.0).clamp(240.0, 350.0);
        let card_rect = Rect::from_center_size(stage_rect.center(), Vec2::new(card_w, 134.0));
        let mut child_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(card_rect)
                .id_salt((id_salt, "empty_stage")),
        );
        GlassCard::new()
            .elevation(GlassElevation::Floating)
            .padding(egui::Margin::symmetric(18, 14))
            .fill_width(true)
            .show(&mut child_ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Cena vazia")
                            .size(15.5)
                            .strong()
                            .color(prism.palette.text_primary),
                    );
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("Adicione uma tela ou janela nativa para começar")
                            .size(11.8)
                            .color(prism.palette.text_secondary),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let btn_group_w = 248.0;
                        let pad = ((ui.available_width() - btn_group_w) * 0.5).max(0.0);
                        ui.add_space(pad);
                        if theme::tally_button(ui, "+ Captura de tela").clicked() {
                            actions.push(StageAction::OpenPickerForNewScreen);
                        }
                        if ui
                            .add(
                                GlassButton::secondary("+ Janela")
                                    .pill(true)
                                    .min_size(Vec2::new(88.0, 32.0)),
                            )
                            .clicked()
                        {
                            actions.push(StageAction::OpenPickerForNewWindow);
                        }
                    });
                });
            });
    }

    // Tag pill translúcida de identificação do viewport (PROGRAMA / PRÉVIA / 9:16)
    let badge_font = FontId::monospace(10.0);
    let badge_galley =
        painter.layout_no_wrap(label_badge.to_owned(), badge_font.clone(), Color32::WHITE);
    let badge_pos = Pos2::new(stage_rect.left() + 10.0, stage_rect.top() + 10.0);
    let badge_w = (badge_galley.size().x + 28.0).max(74.0);
    let badge_rect = Rect::from_min_size(badge_pos, Vec2::new(badge_w, 22.0));
    painter.rect_filled(
        badge_rect,
        CornerRadius::same(11),
        Color32::from_rgba_unmultiplied(12, 13, 16, 198),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(11),
        Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(
                badge_accent.r(),
                badge_accent.g(),
                badge_accent.b(),
                185,
            ),
        ),
        StrokeKind::Inside,
    );
    let dot_pos = Pos2::new(badge_rect.left() + 10.0, badge_rect.center().y);
    painter.circle_filled(
        dot_pos,
        4.6,
        Color32::from_rgba_unmultiplied(badge_accent.r(), badge_accent.g(), badge_accent.b(), 65),
    );
    painter.circle_filled(dot_pos, 2.6, badge_accent);
    painter.text(
        Pos2::new(badge_rect.left() + 19.0, badge_rect.center().y),
        Align2::LEFT_CENTER,
        label_badge,
        badge_font,
        Color32::from_rgb(0xF5, 0xF3, 0xEE),
    );

    if !editable {
        return;
    }

    let to_rel = |p: Pos2| -> Pos2 {
        Pos2::new(
            (p.x - stage_rect.left()) / stage_rect.width().max(1.0),
            (p.y - stage_rect.top()) / stage_rect.height().max(1.0),
        )
    };

    let hit_test = |pos: Pos2, items: &[&SceneItem]| -> Option<String> {
        for it in items.iter().rev() {
            let r = item_rect_in_stage(stage_rect, tf_of(it, vertical));
            if r.contains(pos) {
                return Some(it.id.clone());
            }
        }
        None
    };

    if interaction.drag.is_none() {
        interaction.hover_id = resp
            .hover_pos()
            .filter(|p| stage_rect.contains(*p))
            .and_then(|p| hit_test(p, &visible_items));
    }

    // Início de arrasto (alça de redimensionamento ou corpo da fonte)
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(pointer) =
            ui.input(|i| i.pointer.press_origin().or_else(|| i.pointer.hover_pos()))
        {
            let mut started_handle = false;
            if let Some(sel_id) = selected_id {
                if let Some(sel_item) = visible_items.iter().find(|it| it.id == sel_id) {
                    if !sel_item.locked {
                        let r = item_rect_in_stage(stage_rect, tf_of(sel_item, vertical));
                        let corners = [
                            CornerHandle::Nw,
                            CornerHandle::Ne,
                            CornerHandle::Sw,
                            CornerHandle::Se,
                        ];
                        for corner in corners {
                            if corner_pos(r, corner).distance(pointer) <= 12.0 {
                                let alt = ui.input(|i| i.modifiers.alt);
                                interaction.drag = Some(DragOp::Resize {
                                    item_id: sel_item.id.clone(),
                                    corner,
                                    start_rel: to_rel(pointer),
                                    orig: tf_of(sel_item, vertical).clone(),
                                    crop_mode: alt,
                                });
                                started_handle = true;
                                break;
                            }
                        }
                    }
                }
            }
            if !started_handle && stage_rect.contains(pointer) {
                if let Some(hit_id) = hit_test(pointer, &visible_items) {
                    actions.push(StageAction::Select(Some(hit_id.clone())));
                    if let Some(item) = visible_items.iter().find(|it| it.id == hit_id) {
                        if !item.locked {
                            interaction.drag = Some(DragOp::Move {
                                item_id: item.id.clone(),
                                start_rel: to_rel(pointer),
                                orig: tf_of(item, vertical).clone(),
                            });
                        }
                    }
                } else {
                    actions.push(StageAction::Select(None));
                }
            }
        }
    } else if resp.clicked() {
        if let Some(pointer) = resp.interact_pointer_pos() {
            if stage_rect.contains(pointer) {
                let hit = hit_test(pointer, &visible_items);
                actions.push(StageAction::Select(hit));
            }
        }
    }

    // Atualização durante arrasto
    if resp.dragged_by(egui::PointerButton::Primary) {
        if let (Some(drag), Some(pointer)) = (&interaction.drag, resp.interact_pointer_pos()) {
            let cur_rel = to_rel(pointer);
            let shift = ui.input(|i| i.modifiers.shift);
            match drag {
                DragOp::Move {
                    item_id,
                    start_rel,
                    orig,
                } => {
                    let dx = cur_rel.x - start_rel.x;
                    let dy = cur_rel.y - start_rel.y;
                    let raw_x = orig.x + dx;
                    let raw_y = orig.y + dy;
                    let x_targets = [
                        (0.0, 0.0),
                        (1.0 - orig.width, 1.0),
                        (0.5 - orig.width * 0.5, 0.5),
                    ];
                    let y_targets = [
                        (0.0, 0.0),
                        (1.0 - orig.height, 1.0),
                        (0.5 - orig.height * 0.5, 0.5),
                    ];
                    let (nx, gx) = snap_axis(raw_x, &x_targets);
                    let (ny, gy) = snap_axis(raw_y, &y_targets);
                    interaction.snap_x = gx;
                    interaction.snap_y = gy;
                    let mut next = orig.clone();
                    next.x = nx;
                    next.y = ny;
                    actions.push(StageAction::UpdateTransform {
                        item_id: item_id.clone(),
                        vertical,
                        transform: next,
                        commit_undo: false,
                    });
                }
                DragOp::Resize {
                    item_id,
                    corner,
                    start_rel,
                    orig,
                    crop_mode,
                } => {
                    let dx = cur_rel.x - start_rel.x;
                    let dy = cur_rel.y - start_rel.y;
                    let mut next = orig.clone();
                    if *crop_mode {
                        let ow = orig.width.max(0.05);
                        let oh = orig.height.max(0.05);
                        match corner {
                            CornerHandle::Nw => {
                                next.crop_left = Some(
                                    (orig.crop_left.unwrap_or(0.0) + dx / ow).clamp(0.0, 0.85),
                                );
                                next.crop_top =
                                    Some((orig.crop_top.unwrap_or(0.0) + dy / oh).clamp(0.0, 0.85));
                            }
                            CornerHandle::Ne => {
                                next.crop_right = Some(
                                    (orig.crop_right.unwrap_or(0.0) - dx / ow).clamp(0.0, 0.85),
                                );
                                next.crop_top =
                                    Some((orig.crop_top.unwrap_or(0.0) + dy / oh).clamp(0.0, 0.85));
                            }
                            CornerHandle::Sw => {
                                next.crop_left = Some(
                                    (orig.crop_left.unwrap_or(0.0) + dx / ow).clamp(0.0, 0.85),
                                );
                                next.crop_bottom = Some(
                                    (orig.crop_bottom.unwrap_or(0.0) - dy / oh).clamp(0.0, 0.85),
                                );
                            }
                            CornerHandle::Se => {
                                next.crop_right = Some(
                                    (orig.crop_right.unwrap_or(0.0) - dx / ow).clamp(0.0, 0.85),
                                );
                                next.crop_bottom = Some(
                                    (orig.crop_bottom.unwrap_or(0.0) - dy / oh).clamp(0.0, 0.85),
                                );
                            }
                        }
                    } else {
                        let aspect_orig = (orig.width / orig.height.max(0.02)).max(0.05);
                        let (mut nx, mut ny) = (orig.x, orig.y);
                        let (nw, nh);
                        match corner {
                            CornerHandle::Se => {
                                nw = (orig.width + dx).max(0.05);
                                nh = if shift {
                                    (orig.height + dy).max(0.05)
                                } else {
                                    nw / aspect_orig
                                };
                            }
                            CornerHandle::Sw => {
                                nw = (orig.width - dx).max(0.05);
                                nh = if shift {
                                    (orig.height + dy).max(0.05)
                                } else {
                                    nw / aspect_orig
                                };
                                nx = orig.x + (orig.width - nw);
                            }
                            CornerHandle::Ne => {
                                nw = (orig.width + dx).max(0.05);
                                nh = if shift {
                                    (orig.height - dy).max(0.05)
                                } else {
                                    nw / aspect_orig
                                };
                                ny = orig.y + (orig.height - nh);
                            }
                            CornerHandle::Nw => {
                                nw = (orig.width - dx).max(0.05);
                                nh = if shift {
                                    (orig.height - dy).max(0.05)
                                } else {
                                    nw / aspect_orig
                                };
                                nx = orig.x + (orig.width - nw);
                                ny = orig.y + (orig.height - nh);
                            }
                        }
                        next.x = nx;
                        next.y = ny;
                        next.width = nw;
                        next.height = nh;
                    }
                    actions.push(StageAction::UpdateTransform {
                        item_id: item_id.clone(),
                        vertical,
                        transform: next,
                        commit_undo: false,
                    });
                }
            }
        }
    }

    if resp.drag_stopped() {
        if let Some(drag) = interaction.drag.take() {
            let item_id = match drag {
                DragOp::Move { item_id, .. } | DragOp::Resize { item_id, .. } => item_id,
            };
            if let Some(it) = visible_items.iter().find(|i| i.id == item_id) {
                actions.push(StageAction::UpdateTransform {
                    item_id,
                    vertical,
                    transform: tf_of(it, vertical).clone(),
                    commit_undo: true,
                });
            }
        }
        interaction.snap_x = None;
        interaction.snap_y = None;
    }

    // Menu de contexto (botão direito)
    resp.context_menu(|ui| {
        let target_id = interaction
            .hover_id
            .clone()
            .or_else(|| selected_id.map(str::to_string));
        if let Some(tid) = target_id {
            if let Some(item) = visible_items.iter().find(|it| it.id == tid) {
                ui.label(
                    RichText::new(&item.name)
                        .strong()
                        .color(prism.palette.text_primary),
                );
                ui.separator();
                if matches!(
                    item.source_type,
                    SourceType::ScreenCapture | SourceType::WindowCapture
                ) && ui.button("Trocar tela / janela…").clicked()
                {
                    actions.push(StageAction::OpenPickerForItem(tid.clone()));
                    ui.close();
                }
                if ui.button("Tela cheia (100%)").clicked() {
                    actions.push(StageAction::FillCanvas {
                        item_id: tid.clone(),
                        vertical,
                    });
                    ui.close();
                }
                if ui.button("Centralizar no palco").clicked() {
                    actions.push(StageAction::CenterCanvas {
                        item_id: tid.clone(),
                        vertical,
                    });
                    ui.close();
                }
                ui.separator();
                if ui.button("Trazer para frente").clicked() {
                    actions.push(StageAction::BringForward(tid.clone()));
                    ui.close();
                }
                if ui.button("Enviar para trás").clicked() {
                    actions.push(StageAction::SendBackward(tid.clone()));
                    ui.close();
                }
                let lock_txt = if item.locked {
                    "Destravar posição"
                } else {
                    "Travar posição"
                };
                if ui.button(lock_txt).clicked() {
                    actions.push(StageAction::ToggleLock(tid.clone()));
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(RichText::new("Remover fonte").color(theme::SPEC_RED))
                    .clicked()
                {
                    actions.push(StageAction::RemoveItem(tid));
                    ui.close();
                }
            }
        } else {
            if ui.button("+ Adicionar captura de tela…").clicked() {
                actions.push(StageAction::OpenPickerForNewScreen);
                ui.close();
            }
            if ui.button("+ Adicionar captura de janela…").clicked() {
                actions.push(StageAction::OpenPickerForNewWindow);
                ui.close();
            }
        }
    });

    // Guias magnéticas de snap (accent do tema / turquesa espectral)
    if let Some(gx) = interaction.snap_x {
        let x = stage_rect.left() + gx * stage_rect.width();
        painter.line_segment(
            [
                Pos2::new(x, stage_rect.top()),
                Pos2::new(x, stage_rect.bottom()),
            ],
            Stroke::new(1.3_f32, theme::SPEC_TEAL),
        );
    }
    if let Some(gy) = interaction.snap_y {
        let y = stage_rect.top() + gy * stage_rect.height();
        painter.line_segment(
            [
                Pos2::new(stage_rect.left(), y),
                Pos2::new(stage_rect.right(), y),
            ],
            Stroke::new(1.3_f32, theme::SPEC_TEAL),
        );
    }

    // Contorno de hover
    if let Some(ref hid) = interaction.hover_id {
        if Some(hid.as_str()) != selected_id {
            if let Some(it) = visible_items.iter().find(|i| &i.id == hid) {
                let r = item_rect_in_stage(stage_rect, tf_of(it, vertical));
                painter.rect_stroke(
                    r,
                    CornerRadius::same(4),
                    Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(24, 220, 184, 170)),
                    StrokeKind::Inside,
                );
            }
        }
    }

    // Seleção ativa + 4 alças de redimensionamento
    if let Some(sel_id) = selected_id {
        if let Some(sel_item) = visible_items.iter().find(|it| it.id == sel_id) {
            let tf = tf_of(sel_item, vertical);
            let r = item_rect_in_stage(stage_rect, tf);
            let is_missing = capture_key_for_item(sel_item)
                .and_then(|k| capture_statuses.get(&k))
                .copied()
                == Some(CaptureStatus::Missing);
            let border_col = if sel_item.locked {
                prism.palette.text_secondary
            } else if is_missing {
                theme::SPEC_RED
            } else {
                theme::SPEC_AMBER
            };

            painter.rect_stroke(
                r,
                CornerRadius::same(4),
                Stroke::new(1.8_f32, border_col),
                StrokeKind::Inside,
            );

            let mut tag = sel_item.name.clone();
            if sel_item.locked {
                tag.push_str(" · travada");
            } else if is_missing {
                tag.push_str(" · janela fechada");
            }
            let tag_font = FontId::proportional(11.0);
            let tag_galley = painter.layout_no_wrap(tag.clone(), tag_font.clone(), Color32::WHITE);
            let tag_w = (tag_galley.size().x + 16.0).clamp(56.0, 260.0);
            let tag_y = if r.top() - 24.0 > stage_rect.top() {
                r.top() - 24.0
            } else {
                r.top() + 5.0
            };
            let tag_rect =
                Rect::from_min_size(Pos2::new(r.left() + 3.0, tag_y), Vec2::new(tag_w, 20.0));
            painter.rect_filled(tag_rect, CornerRadius::same(10), theme::TALLY);
            painter.rect_stroke(
                tag_rect,
                CornerRadius::same(10),
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 55)),
                StrokeKind::Inside,
            );
            painter.text(
                tag_rect.center(),
                Align2::CENTER_CENTER,
                tag,
                tag_font,
                Color32::WHITE,
            );

            if !sel_item.locked {
                for corner in [
                    CornerHandle::Nw,
                    CornerHandle::Ne,
                    CornerHandle::Sw,
                    CornerHandle::Se,
                ] {
                    let cp = corner_pos(r, corner);
                    let hr = Rect::from_center_size(cp, Vec2::splat(9.5));
                    painter.rect_filled(hr, CornerRadius::same(3), Color32::WHITE);
                    painter.rect_stroke(
                        hr,
                        CornerRadius::same(3),
                        Stroke::new(1.4_f32, theme::TALLY),
                        StrokeKind::Outside,
                    );
                }
            }
        }
    }

    visible_items.clear();
}

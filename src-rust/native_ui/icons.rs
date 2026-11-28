//! Ícones vetoriais nativos do PrismCast Studio.
//!
//! Desenhados diretamente com o `Painter` do `egui` (traço de 1.5 px numa grade de 16 pt),
//! substituem os emojis/glyphs antigos — que dependiam de fontes de fallback e ficavam
//! desalinhados com a tipografia Inter da `egui-prismglass` 0.3. Todos escalam com o DPI e
//! herdam a cor do tema ativo.

use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Studio,
    Widgets,
    Broadcast,
    Network,
    Settings,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Trash,
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Plus,
    Close,
    Check,
    Volume,
    VolumeMute,
    Sliders,
    Monitor,
    Window,
    Image,
    Text,
    Camera,
    Globe,
    Swatch,
    Record,
    Stop,
    Replay,
    Pause,
    Vertical,
    Columns,
    Swap,
    Undo,
    Refresh,
    Send,
    Sun,
    Moon,
    Chat,
    Layers,
    Download,
    Key,
    Mic,
    Bolt,
    Folder,
}

/// Pinta `icon` centrado em `rect` com a cor `color`.
pub fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    if s <= 1.0 {
        return;
    }
    let c = rect.center();
    // Coordenadas normalizadas: (-1..1) dentro de uma caixa de 16 pt com 1.5 pt de respiro.
    let u = s / 2.0 * 0.82;
    let p = |x: f32, y: f32| pos2(c.x + x * u, c.y + y * u);
    let w = (s / 16.0 * 1.5).clamp(1.1, 2.4);
    let st = Stroke::new(w, color);

    let line = |pts: Vec<Pos2>| {
        painter.add(Shape::line(pts, st));
    };
    let closed = |pts: Vec<Pos2>| {
        painter.add(Shape::closed_line(pts, st));
    };
    let seg = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], st);
    };
    let rrect = |r: Rect, radius: f32| {
        painter.rect_stroke(r, radius, st, egui::StrokeKind::Middle);
    };
    let box_r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));

    match icon {
        Icon::Studio => {
            rrect(box_r(-0.95, -0.75, 0.95, 0.55), u * 0.18);
            seg(p(-0.4, 0.9), p(0.4, 0.9));
            painter.add(Shape::convex_polygon(
                vec![p(-0.22, -0.42), p(0.36, -0.1), p(-0.22, 0.22)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Widgets => {
            for (x, y) in [(-0.9, -0.9), (0.1, -0.9), (-0.9, 0.1), (0.1, 0.1)] {
                rrect(box_r(x, y, x + 0.8, y + 0.8), u * 0.16);
            }
        }
        Icon::Broadcast => {
            painter.circle_filled(p(0.0, 0.0), u * 0.17, color);
            line(arc(p(0.0, 0.0), u * 0.5, -PI * 0.28, PI * 0.28, 10));
            line(arc(p(0.0, 0.0), u * 0.5, PI * 0.72, PI * 1.28, 10));
            line(arc(p(0.0, 0.0), u * 0.9, -PI * 0.25, PI * 0.25, 12));
            line(arc(p(0.0, 0.0), u * 0.9, PI * 0.75, PI * 1.25, 12));
        }
        Icon::Network => {
            let nodes = [p(0.0, -0.7), p(-0.72, 0.6), p(0.72, 0.6)];
            seg(nodes[0], nodes[1]);
            seg(nodes[0], nodes[2]);
            seg(nodes[1], nodes[2]);
            for n in nodes {
                painter.circle_filled(n, u * 0.26, color);
            }
        }
        Icon::Settings => {
            painter.circle_stroke(p(0.0, 0.0), u * 0.3, st);
            for i in 0..8 {
                let a = i as f32 / 8.0 * TAU;
                let d = vec2(a.cos(), a.sin());
                seg(c + d * u * 0.58, c + d * u * 0.92);
            }
            painter.circle_stroke(p(0.0, 0.0), u * 0.62, st);
        }
        Icon::Eye | Icon::EyeOff => {
            let mut pts = arc(p(0.0, 0.9), u * 1.25, -PI * 0.78, -PI * 0.22, 14);
            pts.extend(arc(p(0.0, -0.9), u * 1.25, PI * 0.22, PI * 0.78, 14));
            closed(pts);
            painter.circle_stroke(p(0.0, 0.0), u * 0.28, st);
            if icon == Icon::EyeOff {
                seg(p(-0.85, -0.85), p(0.85, 0.85));
            }
        }
        Icon::Lock | Icon::Unlock => {
            rrect(box_r(-0.7, -0.1, 0.7, 0.9), u * 0.14);
            let arc_pts = if icon == Icon::Lock {
                let mut a = vec![p(-0.42, -0.1)];
                a.extend(arc(p(0.0, -0.45), u * 0.42, PI, TAU, 10));
                a.push(p(0.42, -0.1));
                a
            } else {
                let mut a = vec![p(-0.42, -0.1)];
                a.extend(arc(p(0.0, -0.55), u * 0.42, PI, TAU * 0.93, 10));
                a
            };
            line(arc_pts);
            painter.circle_filled(p(0.0, 0.38), u * 0.1, color);
        }
        Icon::Trash => {
            seg(p(-0.85, -0.6), p(0.85, -0.6));
            line(vec![
                p(-0.3, -0.6),
                p(-0.25, -0.9),
                p(0.25, -0.9),
                p(0.3, -0.6),
            ]);
            line(vec![
                p(-0.62, -0.6),
                p(-0.5, 0.9),
                p(0.5, 0.9),
                p(0.62, -0.6),
            ]);
            seg(p(-0.15, -0.25), p(-0.12, 0.55));
            seg(p(0.15, -0.25), p(0.12, 0.55));
        }
        Icon::ChevronUp => line(vec![p(-0.6, 0.3), p(0.0, -0.3), p(0.6, 0.3)]),
        Icon::ChevronDown => line(vec![p(-0.6, -0.3), p(0.0, 0.3), p(0.6, -0.3)]),
        Icon::ChevronLeft => line(vec![p(0.3, -0.6), p(-0.3, 0.0), p(0.3, 0.6)]),
        Icon::ChevronRight => line(vec![p(-0.3, -0.6), p(0.3, 0.0), p(-0.3, 0.6)]),
        Icon::Plus => {
            seg(p(-0.7, 0.0), p(0.7, 0.0));
            seg(p(0.0, -0.7), p(0.0, 0.7));
        }
        Icon::Close => {
            seg(p(-0.58, -0.58), p(0.58, 0.58));
            seg(p(0.58, -0.58), p(-0.58, 0.58));
        }
        Icon::Check => line(vec![p(-0.7, 0.05), p(-0.2, 0.55), p(0.72, -0.5)]),
        Icon::Volume | Icon::VolumeMute => {
            closed(vec![
                p(-0.9, -0.3),
                p(-0.5, -0.3),
                p(0.0, -0.75),
                p(0.0, 0.75),
                p(-0.5, 0.3),
                p(-0.9, 0.3),
            ]);
            if icon == Icon::Volume {
                line(arc(p(0.0, 0.0), u * 0.42, -PI * 0.3, PI * 0.3, 8));
                line(arc(p(0.0, 0.0), u * 0.8, -PI * 0.3, PI * 0.3, 10));
            } else {
                seg(p(0.3, -0.35), p(0.9, 0.35));
                seg(p(0.9, -0.35), p(0.3, 0.35));
            }
        }
        Icon::Sliders => {
            for (y, knob) in [(-0.6, 0.35), (0.0, -0.4), (0.6, 0.1)] {
                seg(p(-0.9, y), p(0.9, y));
                painter.circle_filled(p(knob, y), u * 0.2, color);
            }
        }
        Icon::Monitor => {
            rrect(box_r(-0.95, -0.75, 0.95, 0.5), u * 0.14);
            seg(p(0.0, 0.5), p(0.0, 0.85));
            seg(p(-0.45, 0.88), p(0.45, 0.88));
        }
        Icon::Window => {
            rrect(box_r(-0.95, -0.8, 0.95, 0.8), u * 0.14);
            seg(p(-0.95, -0.38), p(0.95, -0.38));
            painter.circle_filled(p(-0.66, -0.6), u * 0.07, color);
            painter.circle_filled(p(-0.44, -0.6), u * 0.07, color);
        }
        Icon::Image => {
            rrect(box_r(-0.95, -0.8, 0.95, 0.8), u * 0.14);
            painter.circle_stroke(p(0.4, -0.32), u * 0.16, st);
            line(vec![
                p(-0.95, 0.6),
                p(-0.3, -0.05),
                p(0.2, 0.42),
                p(0.45, 0.2),
                p(0.95, 0.62),
            ]);
        }
        Icon::Text => {
            seg(p(-0.75, -0.75), p(0.75, -0.75));
            seg(p(0.0, -0.75), p(0.0, 0.8));
            seg(p(-0.3, 0.8), p(0.3, 0.8));
        }
        Icon::Camera => {
            rrect(box_r(-0.95, -0.5, 0.35, 0.55), u * 0.14);
            closed(vec![
                p(0.35, -0.12),
                p(0.95, -0.45),
                p(0.95, 0.5),
                p(0.35, 0.18),
            ]);
        }
        Icon::Globe => {
            painter.circle_stroke(c, u * 0.9, st);
            seg(p(-0.9, 0.0), p(0.9, 0.0));
            let mut mer = arc(c, u * 0.9, -PI * 0.5, PI * 0.5, 12);
            for q in &mut mer {
                q.x = c.x + (q.x - c.x) * 0.45;
            }
            line(mer.clone());
            for q in &mut mer {
                q.x = c.x - (q.x - c.x);
            }
            line(mer);
        }
        Icon::Swatch => {
            painter.circle_stroke(c, u * 0.88, st);
            painter.add(Shape::convex_polygon(
                arc(c, u * 0.88, -PI * 0.5, PI * 0.5, 14),
                color,
                Stroke::NONE,
            ));
        }
        Icon::Record => {
            painter.circle_filled(c, u * 0.55, color);
        }
        Icon::Stop => {
            painter.rect_filled(box_r(-0.5, -0.5, 0.5, 0.5), u * 0.12, color);
        }
        Icon::Replay | Icon::Refresh | Icon::Undo => {
            let (a0, a1) = if icon == Icon::Undo {
                (PI * 1.05, PI * 2.6)
            } else {
                (-PI * 0.35, PI * 1.35)
            };
            let pts = arc(c, u * 0.72, a0, a1, 22);
            let tip = *pts.last().unwrap_or(&c);
            let prev = pts[pts.len().saturating_sub(3)];
            line(pts);
            let dir = (tip - prev).normalized();
            let n = vec2(-dir.y, dir.x);
            painter.add(Shape::convex_polygon(
                vec![tip + dir * u * 0.3, tip + n * u * 0.26, tip - n * u * 0.26],
                color,
                Stroke::NONE,
            ));
            if icon == Icon::Replay {
                painter.circle_filled(c, u * 0.14, color);
            }
        }
        Icon::Pause => {
            painter.rect_filled(box_r(-0.55, -0.7, -0.15, 0.7), u * 0.08, color);
            painter.rect_filled(box_r(0.15, -0.7, 0.55, 0.7), u * 0.08, color);
        }
        Icon::Vertical => {
            rrect(box_r(-0.5, -0.92, 0.5, 0.92), u * 0.2);
            seg(p(-0.15, 0.62), p(0.15, 0.62));
        }
        Icon::Columns => {
            rrect(box_r(-0.95, -0.7, -0.08, 0.7), u * 0.12);
            rrect(box_r(0.08, -0.7, 0.95, 0.7), u * 0.12);
        }
        Icon::Swap => {
            line(vec![p(-0.8, -0.35), p(0.8, -0.35)]);
            line(vec![p(0.45, -0.7), p(0.8, -0.35), p(0.45, 0.0)]);
            line(vec![p(0.8, 0.4), p(-0.8, 0.4)]);
            line(vec![p(-0.45, 0.05), p(-0.8, 0.4), p(-0.45, 0.75)]);
        }
        Icon::Send => {
            closed(vec![
                p(-0.9, -0.8),
                p(0.9, 0.0),
                p(-0.9, 0.8),
                p(-0.55, 0.0),
            ]);
            seg(p(-0.55, 0.0), p(0.2, 0.0));
        }
        Icon::Sun => {
            painter.circle_stroke(c, u * 0.38, st);
            for i in 0..8 {
                let a = i as f32 / 8.0 * TAU;
                let d = vec2(a.cos(), a.sin());
                seg(c + d * u * 0.62, c + d * u * 0.92);
            }
        }
        Icon::Moon => {
            let mut pts = arc(c, u * 0.85, PI * 0.30, PI * 1.70, 20);
            let inner_c = p(0.42, -0.22);
            let mut inner = arc(inner_c, u * 0.68, PI * 1.62, PI * 0.42, 16);
            inner.reverse();
            inner.reverse();
            pts.extend(inner);
            closed(pts);
        }
        Icon::Chat => {
            let r = box_r(-0.92, -0.78, 0.92, 0.5);
            rrect(r, u * 0.3);
            line(vec![p(-0.45, 0.5), p(-0.6, 0.92), p(-0.05, 0.5)]);
        }
        Icon::Layers => {
            let diamond = |dy: f32| {
                vec![
                    p(0.0, -0.55 + dy),
                    p(0.9, -0.1 + dy),
                    p(0.0, 0.35 + dy),
                    p(-0.9, -0.1 + dy),
                ]
            };
            closed(diamond(-0.15));
            line(vec![p(-0.9, 0.3), p(0.0, 0.75), p(0.9, 0.3)]);
        }
        Icon::Download => {
            seg(p(0.0, -0.9), p(0.0, 0.35));
            line(vec![p(-0.45, -0.05), p(0.0, 0.4), p(0.45, -0.05)]);
            line(vec![p(-0.9, 0.45), p(-0.9, 0.9), p(0.9, 0.9), p(0.9, 0.45)]);
        }
        Icon::Key => {
            painter.circle_stroke(p(-0.45, 0.0), u * 0.38, st);
            seg(p(-0.07, 0.0), p(0.92, 0.0));
            seg(p(0.55, 0.0), p(0.55, 0.38));
            seg(p(0.82, 0.0), p(0.82, 0.3));
        }
        Icon::Mic => {
            rrect(box_r(-0.32, -0.92, 0.32, 0.3), u * 0.32);
            line(arc(p(0.0, -0.05), u * 0.62, 0.0, PI, 12));
            seg(p(0.0, 0.57), p(0.0, 0.92));
        }
        Icon::Bolt => {
            closed(vec![
                p(0.15, -0.95),
                p(-0.6, 0.1),
                p(-0.05, 0.1),
                p(-0.2, 0.95),
                p(0.6, -0.15),
                p(0.05, -0.15),
            ]);
        }
        Icon::Folder => {
            closed(vec![
                p(-0.95, -0.7),
                p(-0.3, -0.7),
                p(-0.1, -0.45),
                p(0.95, -0.45),
                p(0.95, 0.8),
                p(-0.95, 0.8),
            ]);
        }
    }
}

/// Pontos de um arco (ângulos em radianos, sentido horário na tela).
fn arc(center: Pos2, radius: f32, a0: f32, a1: f32, steps: usize) -> Vec<Pos2> {
    let steps = steps.max(2);
    (0..=steps)
        .map(|i| {
            let t = a0 + (a1 - a0) * (i as f32 / steps as f32);
            center + Vec2::new(t.cos(), t.sin()) * radius
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_paints_without_panicking() {
        let ctx = egui::Context::default();
        let all = [
            Icon::Studio,
            Icon::Widgets,
            Icon::Broadcast,
            Icon::Network,
            Icon::Settings,
            Icon::Eye,
            Icon::EyeOff,
            Icon::Lock,
            Icon::Unlock,
            Icon::Trash,
            Icon::ChevronUp,
            Icon::ChevronDown,
            Icon::ChevronLeft,
            Icon::ChevronRight,
            Icon::Plus,
            Icon::Close,
            Icon::Check,
            Icon::Volume,
            Icon::VolumeMute,
            Icon::Sliders,
            Icon::Monitor,
            Icon::Window,
            Icon::Image,
            Icon::Text,
            Icon::Camera,
            Icon::Globe,
            Icon::Swatch,
            Icon::Record,
            Icon::Stop,
            Icon::Replay,
            Icon::Pause,
            Icon::Vertical,
            Icon::Columns,
            Icon::Swap,
            Icon::Undo,
            Icon::Refresh,
            Icon::Send,
            Icon::Sun,
            Icon::Moon,
            Icon::Chat,
            Icon::Layers,
            Icon::Download,
            Icon::Key,
            Icon::Mic,
            Icon::Bolt,
            Icon::Folder,
        ];
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            for (i, icon) in all.iter().enumerate() {
                let r = Rect::from_min_size(pos2(i as f32 * 20.0, 0.0), Vec2::splat(16.0));
                paint_icon(painter, r, *icon, Color32::WHITE);
            }
            // Degenerate rect must be a no-op.
            paint_icon(painter, Rect::NOTHING, Icon::Plus, Color32::WHITE);
        });
        assert!(!out.shapes.is_empty());
        out.textures_delta.clear();
    }
}

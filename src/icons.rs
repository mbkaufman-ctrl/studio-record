//! Line icons drawn with the painter (1.5px strokes, 16/20px boxes), so
//! the UI never relies on emoji or an icon font.

use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Camera,
    Screen,
    Tablet,
    Mic,
    Record,
    Stop,
    Broadcast,
    Window,
    Folder,
    Sun,
    Moon,
    Command,
    Refresh,
    Mirror,
    Layers,
    External,
    Check,
    Pin,
    Alert,
    Link,
    Back,
    Forward,
    Pause,
    MicOff,
    CameraOff,
    Live,
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.5, color);
    let c = rect.center();
    let u = rect.width().min(rect.height()) / 20.0; // 20-unit grid
    let at = |x: f32, y: f32| pos2(c.x + (x - 10.0) * u, c.y + (y - 10.0) * u);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    match icon {
        Icon::Camera => {
            p.rect_stroke(r(2.5, 5.5, 13.5, 14.5), 2.0 * u, s, egui::StrokeKind::Middle);
            p.add(Shape::closed_line(vec![at(13.5, 8.8), at(17.5, 6.5), at(17.5, 13.5), at(13.5, 11.2)], s));
        }
        Icon::Screen => {
            p.rect_stroke(r(2.5, 3.5, 17.5, 13.5), 1.5 * u, s, egui::StrokeKind::Middle);
            p.line_segment([at(10.0, 13.5), at(10.0, 16.5)], s);
            p.line_segment([at(6.5, 16.5), at(13.5, 16.5)], s);
        }
        Icon::Tablet => {
            p.rect_stroke(r(4.5, 2.5, 15.5, 17.5), 2.0 * u, s, egui::StrokeKind::Middle);
            p.line_segment([at(8.5, 15.0), at(11.5, 15.0)], s);
        }
        Icon::Mic => {
            p.rect_stroke(r(7.5, 2.5, 12.5, 11.5), 2.5 * u, s, egui::StrokeKind::Middle);
            arc(p, at(10.0, 9.0), 5.0 * u, 0.0, std::f32::consts::PI, s);
            p.line_segment([at(10.0, 14.0), at(10.0, 17.5)], s);
        }
        Icon::Record => {
            p.circle_filled(c, 5.0 * u, color);
        }
        Icon::Stop => {
            p.rect_filled(r(6.0, 6.0, 14.0, 14.0), 1.5 * u, color);
        }
        Icon::Broadcast => {
            p.circle_filled(c, 1.8 * u, color);
            arc(p, c, 4.5 * u, -0.8, 0.8, s);
            arc(p, c, 4.5 * u, std::f32::consts::PI - 0.8, std::f32::consts::PI + 0.8, s);
            arc(p, c, 7.5 * u, -0.7, 0.7, s);
            arc(p, c, 7.5 * u, std::f32::consts::PI - 0.7, std::f32::consts::PI + 0.7, s);
        }
        Icon::Window => {
            p.rect_stroke(r(2.5, 3.5, 17.5, 16.5), 2.0 * u, s, egui::StrokeKind::Middle);
            p.line_segment([at(2.5, 7.0), at(17.5, 7.0)], s);
        }
        Icon::Folder => {
            p.add(Shape::closed_line(
                vec![at(2.5, 5.0), at(8.0, 5.0), at(9.5, 6.8), at(17.5, 6.8), at(17.5, 15.5), at(2.5, 15.5)],
                s,
            ));
        }
        Icon::Sun => {
            p.circle_stroke(c, 3.2 * u, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * 5.6 * u, c + d * 7.4 * u], s);
            }
        }
        Icon::Moon => {
            arc(p, c, 6.5 * u, 0.9, 5.4, s);
            arc(p, at(12.8, 7.6), 5.0 * u, 1.75, 4.55, s);
        }
        Icon::Command => {
            for (x, y) in [(6.0, 6.0), (14.0, 6.0), (6.0, 14.0), (14.0, 14.0)] {
                p.circle_stroke(at(x, y), 2.0 * u, s);
            }
            p.rect_stroke(r(8.0, 8.0, 12.0, 12.0), 0.0, s, egui::StrokeKind::Middle);
        }
        Icon::Refresh => {
            arc(p, c, 6.0 * u, -2.6, 2.2, s);
            let tip = c + vec2((-2.6f32).cos(), (-2.6f32).sin()) * 6.0 * u;
            p.line_segment([tip, tip + vec2(0.0, -3.0 * u)], s);
            p.line_segment([tip, tip + vec2(3.0 * u, 0.5 * u)], s);
        }
        Icon::Mirror => {
            p.line_segment([at(10.0, 2.5), at(10.0, 17.5)], Stroke::new(1.0, color));
            p.add(Shape::closed_line(vec![at(7.5, 5.5), at(7.5, 14.5), at(2.5, 14.5)], s));
            p.add(Shape::closed_line(vec![at(12.5, 5.5), at(12.5, 14.5), at(17.5, 14.5)], s));
        }
        Icon::Layers => {
            p.add(Shape::closed_line(vec![at(10.0, 3.0), at(17.5, 7.0), at(10.0, 11.0), at(2.5, 7.0)], s));
            p.add(Shape::line(vec![at(2.5, 10.5), at(10.0, 14.5), at(17.5, 10.5)], s));
        }
        Icon::External => {
            p.line_segment([at(8.0, 12.0), at(16.0, 4.0)], s);
            p.add(Shape::line(vec![at(10.5, 4.0), at(16.0, 4.0), at(16.0, 9.5)], s));
            p.add(Shape::line(vec![at(13.5, 11.5), at(13.5, 16.0), at(4.0, 16.0), at(4.0, 6.5), at(8.5, 6.5)], s));
        }
        Icon::Check => {
            p.add(Shape::line(vec![at(4.5, 10.5), at(8.5, 14.0), at(15.5, 6.0)], Stroke::new(2.0, color)));
        }
        Icon::Pin => {
            p.rect_stroke(r(3.0, 3.0, 17.0, 17.0), 2.0 * u, s, egui::StrokeKind::Middle);
            p.rect_filled(r(10.0, 10.0, 15.0, 15.0), 1.0 * u, color);
        }
        Icon::Link => {
            p.rect_stroke(r(2.5, 7.0, 11.5, 13.0), 3.0 * u, s, egui::StrokeKind::Middle);
            p.rect_stroke(r(8.5, 7.0, 17.5, 13.0), 3.0 * u, s, egui::StrokeKind::Middle);
        }
        Icon::Back => {
            p.add(Shape::line(vec![at(12.5, 4.5), at(7.0, 10.0), at(12.5, 15.5)], s));
        }
        Icon::Forward => {
            p.add(Shape::line(vec![at(7.5, 4.5), at(13.0, 10.0), at(7.5, 15.5)], s));
        }
        Icon::Pause => {
            p.rect_filled(r(5.5, 4.5, 8.5, 15.5), 1.0 * u, color);
            p.rect_filled(r(11.5, 4.5, 14.5, 15.5), 1.0 * u, color);
        }
        Icon::MicOff => {
            p.rect_stroke(r(7.5, 2.5, 12.5, 11.5), 2.5 * u, s, egui::StrokeKind::Middle);
            arc(p, at(10.0, 9.0), 5.0 * u, 0.0, std::f32::consts::PI, s);
            p.line_segment([at(10.0, 14.0), at(10.0, 17.5)], s);
            p.line_segment([at(3.0, 3.0), at(17.0, 17.0)], Stroke::new(1.8, color));
        }
        Icon::CameraOff => {
            p.rect_stroke(r(2.5, 5.5, 13.5, 14.5), 2.0 * u, s, egui::StrokeKind::Middle);
            p.add(Shape::closed_line(vec![at(13.5, 8.8), at(17.5, 6.5), at(17.5, 13.5), at(13.5, 11.2)], s));
            p.line_segment([at(2.5, 2.5), at(17.5, 17.5)], Stroke::new(1.8, color));
        }
        Icon::Live => {
            p.circle_stroke(c, 7.0 * u, s);
            p.circle_filled(c, 3.2 * u, color);
        }
        Icon::Alert => {
            p.circle_stroke(c, 7.0 * u, s);
            p.line_segment([at(10.0, 6.0), at(10.0, 11.0)], s);
            p.circle_filled(at(10.0, 13.8), 1.0 * u, color);
        }
    }
}

fn arc(p: &Painter, c: Pos2, radius: f32, a0: f32, a1: f32, s: Stroke) {
    let n = 18;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    p.add(Shape::line(pts, s));
}

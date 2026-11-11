//! Small gear icons authored in the Reference 20-unit view box.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};

pub(super) fn draw(painter: &egui::Painter, rect: Rect, gear: &str, color: Color32) {
    match gear {
        "amp+cab" | "full-rig" => full_rig(painter, rect, color),
        "cab" | "cabinet" => cabinet(painter, rect, color),
        "pedal" => pedal(painter, rect, color),
        "outboard" => outboard(painter, rect, color),
        "space" | "spaces" => space(painter, rect, color),
        "experimental" => experimental(painter, rect, color),
        _ => amp(painter, rect, color),
    }
}

fn full_rig(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(unit(rect, 1.16118), color);
    outlined(
        painter,
        rect,
        (4.6846, 1.0, 15.136, 4.48373),
        1.05652,
        stroke,
    );
    outlined(
        painter,
        rect,
        (3.0, 5.06475, 16.9351, 19.0),
        1.16126,
        stroke,
    );
    for (x, y) in [
        (7.35107, 9.41951),
        (12.5768, 9.41951),
        (7.35107, 14.6452),
        (12.5768, 14.6452),
    ] {
        painter.circle_filled(point(rect, x, y), unit(rect, 2.03221), color);
    }
}

fn amp(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(unit(rect, 1.13333), color);
    outlined(painter, rect, (1.5, 6.2, 18.5, 15.2667), 1.275, stroke);
    let mut arch = vec![point(rect, 6.38141, 7.21383)];
    cubic(&mut arch, rect, (8.0, 4.5), (12.0, 4.5), (13.6186, 7.21383));
    painter.add(egui::epaint::PathShape::line(arch, stroke));
    let panel = box_rect(rect, (4.5, 10.5, 15.5, 14.5));
    painter.rect_filled(panel, unit(rect, 1.0), color);
    for x in [7.0, 10.0, 13.0] {
        painter.circle_filled(point(rect, x, 12.25), unit(rect, 0.75), Color32::BLACK);
    }
}

fn cabinet(painter: &egui::Painter, rect: Rect, color: Color32) {
    outlined(
        painter,
        rect,
        (3.0, 3.0, 17.0, 17.0),
        1.16667,
        Stroke::new(unit(rect, 1.16667), color),
    );
    for (x, y) in [
        (7.37144, 7.37503),
        (12.62144, 7.37503),
        (7.37144, 12.62503),
        (12.62144, 12.62503),
    ] {
        painter.circle_filled(point(rect, x, y), unit(rect, 2.04167), color);
    }
}

fn pedal(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(unit(rect, 1.15116), color);
    outlined(
        painter,
        rect,
        (4.819792, 1.75, 15.18024, 18.25),
        2.11047,
        stroke,
    );
    line(painter, rect, &[(5.0, 8.46512), (15.0, 8.46512)], stroke);
    filled(
        painter,
        rect,
        (7.12212, 11.72674, 12.87793, 15.94774),
        0.57558,
        color,
    );
    outlined(
        painter,
        rect,
        (4.05235, 8.65698, 5.97095, 10.76744),
        0.57558,
        stroke,
    );
    outlined(
        painter,
        rect,
        (14.02909, 8.65698, 15.94774, 10.76744),
        0.57558,
        stroke,
    );
    for x in [8.08142, 11.91863] {
        painter.circle_filled(point(rect, x, 5.01163), unit(rect, 1.34302), color);
    }
}

fn outboard(painter: &egui::Painter, rect: Rect, color: Color32) {
    outlined(
        painter,
        rect,
        (1.0, 6.4, 19.0, 13.6),
        1.8,
        Stroke::new(unit(rect, 1.2), color),
    );
    for x in [4.59968, 8.19972] {
        painter.circle_filled(point(rect, x, 10.00042), unit(rect, 1.2), color);
    }
    filled(
        painter,
        rect,
        (11.1997, 8.80042, 16.5997, 11.20042),
        1.2,
        color,
    );
}

fn space(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(unit(rect, 1.0), color);
    line(
        painter,
        rect,
        &[(2.6918, 15.7439), (9.26923, 15.7439)],
        stroke,
    );
    let mut door = vec![point(rect, 9.26895, 4.46143), point(rect, 9.26895, 16.2694)];
    cubic(
        &mut door,
        rect,
        (9.26895, 17.0),
        (9.6, 17.1),
        (10.17663, 16.9783),
    );
    door.push(point(rect, 15.1156, 15.7439));
    door.push(point(rect, 15.1156, 5.19226));
    cubic(
        &mut door,
        rect,
        (15.1156, 3.8),
        (14.7, 3.9),
        (14.0084, 3.77445),
    );
    door.push(point(rect, 11.08505, 3.043628));
    cubic(&mut door, rect, (10.4, 2.9), (9.8, 3.1), (9.41728, 3.82007));
    cubic(
        &mut door,
        rect,
        (9.25, 4.1),
        (9.26895, 4.3),
        (9.26895, 4.46143),
    );
    painter.add(egui::epaint::PathShape::line(door, stroke));
    let mut frame = vec![point(rect, 9.2691, 4.05072), point(rect, 7.07663, 4.05072)];
    cubic(
        &mut frame,
        rect,
        (6.2, 4.05072),
        (5.61498, 4.7),
        (5.61498, 5.51237),
    );
    frame.push(point(rect, 5.61498, 15.7439));
    painter.add(egui::epaint::PathShape::line(frame, stroke));
    line(
        painter,
        rect,
        &[(11.46133, 9.89733), (11.46968, 9.89733)],
        stroke,
    );
    line(
        painter,
        rect,
        &[(17.3082, 15.7439), (15.1157, 15.7439)],
        stroke,
    );
}

fn experimental(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(unit(rect, 1.0), color);
    painter.circle_filled(point(rect, 10.03223, 10.03241), unit(rect, 0.833335), color);
    let mut first = vec![point(rect, 15.4694, 15.4694)];
    for (control1, control2, end) in [
        ((16.8301, 14.1154), (15.4827, 10.56028), (12.4679, 7.5321)),
        ((9.43972, 4.51725), (5.88461, 3.169912), (4.53059, 4.53059)),
        ((3.169912, 5.88461), (4.51725, 9.43972), (7.5321, 12.4679)),
        ((10.56028, 15.4827), (14.1154, 16.8301), (15.4694, 15.4694)),
    ] {
        cubic(&mut first, rect, control1, control2, end);
    }
    painter.add(egui::epaint::PathShape::line(first, stroke));
    let mut second = vec![point(rect, 12.4679, 12.4679)];
    for (control1, control2, end) in [
        ((15.4827, 9.43972), (16.8301, 5.88461), (15.4694, 4.53059)),
        ((14.1154, 3.169912), (10.56028, 4.51725), (7.5321, 7.5321)),
        ((4.51725, 10.56028), (3.169912, 14.1154), (4.53059, 15.4694)),
        ((5.88461, 16.8301), (9.43972, 15.4827), (12.4679, 12.4679)),
    ] {
        cubic(&mut second, rect, control1, control2, end);
    }
    painter.add(egui::epaint::PathShape::line(second, stroke));
}

fn outlined(
    painter: &egui::Painter,
    rect: Rect,
    bounds: (f32, f32, f32, f32),
    radius: f32,
    stroke: Stroke,
) {
    painter.rect_stroke(
        box_rect(rect, bounds),
        unit(rect, radius),
        stroke,
        egui::StrokeKind::Inside,
    );
}

fn filled(
    painter: &egui::Painter,
    rect: Rect,
    bounds: (f32, f32, f32, f32),
    radius: f32,
    color: Color32,
) {
    painter.rect_filled(box_rect(rect, bounds), unit(rect, radius), color);
}

fn line(painter: &egui::Painter, rect: Rect, points: &[(f32, f32)], stroke: Stroke) {
    painter.add(egui::epaint::PathShape::line(
        points.iter().map(|&(x, y)| point(rect, x, y)).collect(),
        stroke,
    ));
}

fn cubic(
    points: &mut Vec<Pos2>,
    rect: Rect,
    control1: (f32, f32),
    control2: (f32, f32),
    end: (f32, f32),
) {
    let start = points
        .last()
        .copied()
        .unwrap_or_else(|| point(rect, 0.0, 0.0));
    let control1 = point(rect, control1.0, control1.1);
    let control2 = point(rect, control2.0, control2.1);
    let end = point(rect, end.0, end.1);
    for index in 1..=12 {
        let t = index as f32 / 12.0;
        let u = 1.0 - t;
        points.push(egui::pos2(
            u * u * u * start.x
                + 3.0 * u * u * t * control1.x
                + 3.0 * u * t * t * control2.x
                + t * t * t * end.x,
            u * u * u * start.y
                + 3.0 * u * u * t * control1.y
                + 3.0 * u * t * t * control2.y
                + t * t * t * end.y,
        ));
    }
}

fn point(rect: Rect, x: f32, y: f32) -> Pos2 {
    rect.min + egui::vec2(unit(rect, x), unit(rect, y))
}

fn box_rect(rect: Rect, (left, top, right, bottom): (f32, f32, f32, f32)) -> Rect {
    Rect::from_min_max(point(rect, left, top), point(rect, right, bottom))
}

fn unit(rect: Rect, value: f32) -> f32 {
    value * rect.width() / 20.0
}

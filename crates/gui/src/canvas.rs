use std::collections::BTreeSet;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use engine::{Layout, LedKind};

use crate::colors::{contrast, to_color32};
use crate::model::Model;

const UNLIT: Color32 = Color32::from_gray(28);
const PADDING: f32 = 4.0;
const NAMED_KEYS: usize = 6;

#[derive(Debug, Clone, Copy)]
pub struct Transform {
    scale: f32,
    origin: Pos2,
}

impl Transform {
    pub fn fit(bounds: engine::Rect, target: Rect) -> Self {
        let scale = (target.width() / bounds.w).min(target.height() / bounds.h);
        let size = Vec2::new(bounds.w, bounds.h) * scale;
        let origin = target.center() - size / 2.0 - Vec2::new(bounds.x, bounds.y) * scale;
        Self { scale, origin }
    }

    pub fn rect(&self, led: engine::Rect) -> Rect {
        Rect::from_min_size(self.origin + Vec2::new(led.x, led.y) * self.scale, Vec2::new(led.w, led.h) * self.scale)
    }
}

#[derive(Debug, Default)]
pub struct CanvasState {
    marquee: Option<(Pos2, Pos2)>,
    pub last_rect: Option<Rect>,
}

impl CanvasState {
    pub fn transform(&self, layout: &Layout) -> Option<Transform> {
        self.last_rect.map(|rect| Transform::fit(layout.bounds(), rect.shrink(PADDING)))
    }
}

pub fn led_at(layout: &Layout, transform: &Transform, pos: Pos2) -> Option<u8> {
    layout.leds().iter().rev().find(|led| transform.rect(led.rect).contains(pos)).map(|led| led.index)
}

pub fn leds_in(layout: &Layout, transform: &Transform, area: Rect) -> BTreeSet<u8> {
    layout.leds().iter().filter(|led| area.intersects(transform.rect(led.rect))).map(|led| led.index).collect()
}

pub fn click(selection: &mut BTreeSet<u8>, hit: Option<u8>, additive: bool) {
    match hit {
        Some(index) if additive => {
            if !selection.remove(&index) {
                selection.insert(index);
            }
        }
        Some(index) => {
            selection.clear();
            selection.insert(index);
        }
        None if !additive => selection.clear(),
        None => {}
    }
}

pub fn marquee(selection: &mut BTreeSet<u8>, inside: BTreeSet<u8>, additive: bool) {
    if additive {
        selection.extend(inside);
    } else {
        *selection = inside;
    }
}

pub fn selection_label(layout: &Layout, selection: &BTreeSet<u8>) -> String {
    if selection.is_empty() {
        return "Nothing selected".into();
    }
    let names: Vec<String> = selection
        .iter()
        .take(NAMED_KEYS)
        .map(|index| match layout.get(*index) {
            Some(led) if !led.label.is_empty() => led.label.to_owned(),
            _ => format!("LED {index}"),
        })
        .collect();
    match selection.len().saturating_sub(NAMED_KEYS) {
        0 => names.join(", "),
        more => format!("{} and {more} more", names.join(", ")),
    }
}

pub fn show(ui: &mut egui::Ui, model: &mut Model, height: f32) {
    let (response, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
    model.canvas.last_rect = Some(response.rect);
    let Some(transform) = model.canvas.transform(&model.layout) else { return };
    draw(&painter, model, &transform);
    interact(ui, &response, model, &transform);
}

pub fn pane(ui: &mut egui::Ui, model: &mut Model) {
    ui.horizontal(|ui| {
        if ui.button("Select all").clicked() {
            model.selection = model.layout.leds().iter().map(|led| led.index).collect();
        }
        if ui.button("Clear").clicked() {
            model.selection.clear();
        }
        ui.label(selection_label(&model.layout, &model.selection));
    });
    let height = (ui.available_height() * 0.5).clamp(220.0, 420.0);
    show(ui, model, height);
}

fn draw(painter: &egui::Painter, model: &Model, transform: &Transform) {
    for led in model.layout.leds() {
        let rect = transform.rect(led.rect);
        let fill = model.frame.as_ref().map_or(UNLIT, |frame| to_color32(frame[led.index as usize]));
        let rounded = matches!(led.kind, LedKind::DialRing | LedKind::Logo);
        let radius = if rounded {
            CornerRadius::same((rect.width().min(rect.height()) / 2.0) as u8)
        } else {
            CornerRadius::same(3)
        };
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Inside);
        if model.selection.contains(&led.index) {
            painter.rect_stroke(rect, radius, Stroke::new(2.0, Color32::YELLOW), StrokeKind::Outside);
        }
        if !led.label.is_empty() && rect.width() > 14.0 {
            let size = (rect.height() * 0.4).clamp(8.0, 14.0);
            painter.text(rect.center(), Align2::CENTER_CENTER, led.label, FontId::proportional(size), contrast(fill));
        }
    }
    if let Some((start, end)) = model.canvas.marquee {
        painter.rect_stroke(
            Rect::from_two_pos(start, end),
            0,
            Stroke::new(1.0, Color32::LIGHT_BLUE),
            StrokeKind::Inside,
        );
    }
}

fn interact(ui: &egui::Ui, response: &egui::Response, model: &mut Model, transform: &Transform) {
    let additive = ui.input(|input| input.modifiers.command || input.modifiers.shift);
    if response.drag_started()
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        model.canvas.marquee = Some((origin, origin));
    }
    if response.dragged()
        && let (Some((start, _)), Some(pos)) = (model.canvas.marquee, response.interact_pointer_pos())
    {
        model.canvas.marquee = Some((start, pos));
    }
    if response.drag_stopped()
        && let Some((start, end)) = model.canvas.marquee.take()
    {
        let inside = leds_in(&model.layout, transform, Rect::from_two_pos(start, end));
        marquee(&mut model.selection, inside, additive);
    }
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        click(&mut model.selection, led_at(&model.layout, transform, pos), additive);
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::testing::{harness, online};

    fn transform(model: &Model) -> Transform {
        model.canvas.transform(&model.layout).expect("the canvas has been drawn")
    }

    fn center(harness: &Harness<'_, Model>, index: u8) -> Pos2 {
        let model = harness.state();
        transform(model).rect(model.layout.get(index).unwrap().rect).center()
    }

    fn click_at(harness: &mut Harness<'_, Model>, pos: Pos2) {
        harness.hover_at(pos);
        harness.drag_at(pos);
        harness.run();
        harness.drop_at(pos);
        harness.run();
    }

    fn drawn() -> Harness<'static, Model> {
        let mut harness = harness(online("p", &["p"]), |ui, model| show(ui, model, 360.0));
        harness.run();
        harness
    }

    #[test]
    fn fitting_scales_uniformly_and_centres() {
        let bounds = engine::Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let target = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(400.0, 400.0));
        let fitted = Transform::fit(bounds, target).rect(engine::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        assert_eq!(fitted, Rect::from_min_size(Pos2::new(10.0, 110.0), Vec2::new(20.0, 20.0)));
    }

    #[test]
    fn hit_testing_finds_keys_ring_leds_and_the_dial_key() {
        let layout = Layout::ansi();
        let fit = Transform::fit(layout.bounds(), Rect::from_min_size(Pos2::ZERO, Vec2::new(1250.0, 458.0)));
        let at = |index: u8| fit.rect(layout.get(index).unwrap().rect).center();
        assert_eq!(led_at(&layout, &fit, at(26)), Some(26));
        assert_eq!(led_at(&layout, &fit, at(182)), Some(182));
        assert_eq!(led_at(&layout, &fit, at(137)), Some(137));
        assert_eq!(led_at(&layout, &fit, Pos2::new(-50.0, -50.0)), None);
    }

    #[test]
    fn a_box_selects_every_key_it_touches() {
        let layout = Layout::ansi();
        let fit = Transform::fit(layout.bounds(), Rect::from_min_size(Pos2::ZERO, Vec2::new(1250.0, 458.0)));
        let corner = |index: u8| fit.rect(layout.get(index).unwrap().rect);
        let area = [79, 80, 81, 82].into_iter().map(corner).reduce(|a, b| a.union(b)).unwrap();
        assert!(leds_in(&layout, &fit, area).is_superset(&BTreeSet::from([79, 80, 81, 82])));
        assert!(!leds_in(&layout, &fit, area).contains(&26));
    }

    #[test]
    fn plain_clicks_replace_and_modified_clicks_toggle() {
        let mut selection = BTreeSet::new();
        click(&mut selection, Some(4), false);
        click(&mut selection, Some(5), true);
        assert_eq!(selection, BTreeSet::from([4, 5]));
        click(&mut selection, Some(4), true);
        assert_eq!(selection, BTreeSet::from([5]));
        click(&mut selection, Some(7), false);
        assert_eq!(selection, BTreeSet::from([7]));
        click(&mut selection, Some(7), false);
        assert_eq!(selection, BTreeSet::from([7]), "clicking the selected key keeps it");
        click(&mut selection, None, true);
        assert_eq!(selection.len(), 1, "a modified click on nothing keeps the selection");
        click(&mut selection, None, false);
        assert!(selection.is_empty());
        marquee(&mut selection, BTreeSet::from([1, 2]), false);
        marquee(&mut selection, BTreeSet::from([3]), true);
        assert_eq!(selection, BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn the_selection_summary_names_keys_and_counts_the_rest() {
        let layout = Layout::ansi();
        assert_eq!(selection_label(&layout, &BTreeSet::new()), "Nothing selected");
        assert_eq!(selection_label(&layout, &BTreeSet::from([26])), "W");
        assert_eq!(selection_label(&layout, &BTreeSet::from([140])), "LED 140");
        let many: BTreeSet<u8> = (4..12).collect();
        assert!(selection_label(&layout, &many).ends_with("and 2 more"));
    }

    #[test]
    fn clicking_a_key_selects_it_and_shift_click_adds_another() {
        let mut harness = drawn();
        let (w, a) = (center(&harness, 26), center(&harness, 4));
        click_at(&mut harness, w);
        assert_eq!(harness.state().selection, BTreeSet::from([26]));
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
        click_at(&mut harness, a);
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        harness.run();
        assert_eq!(harness.state().selection, BTreeSet::from([4, 26]));
    }

    #[test]
    fn dragging_a_box_selects_the_keys_inside_it() {
        let mut harness = drawn();
        let (from, to) = {
            let model = harness.state();
            let fit = transform(model);
            let rect = |index: u8| fit.rect(model.layout.get(index).unwrap().rect);
            let area = [79, 80, 81, 82].into_iter().map(rect).reduce(|a, b| a.union(b)).unwrap();
            (area.min - Vec2::splat(2.0), area.max + Vec2::splat(2.0))
        };
        harness.hover_at(from);
        harness.drag_at(from);
        harness.run();
        harness.hover_at(to);
        harness.run();
        harness.drop_at(to);
        harness.run();
        assert!(harness.state().selection.is_superset(&BTreeSet::from([79, 80, 81, 82])));
        assert!(!harness.state().selection.contains(&26));
    }

    #[test]
    fn frames_and_selection_draw_without_trouble() {
        let mut model = online("p", &["p"]);
        let mut frame = [k100::Rgb::BLACK; k100::LED_COUNT];
        frame[26] = k100::Rgb::new(255, 0, 0);
        model.frame = Some(frame);
        model.selection.insert(26);
        let mut harness = harness(model, pane);
        harness.run();
        assert!(!harness.get_by_label("Select all").accesskit_node().is_disabled());
    }
}

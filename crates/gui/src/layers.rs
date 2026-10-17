use std::collections::BTreeSet;

use actions::Action;
use eframe::egui;
use engine::effects::KeyColor;
use engine::{BlendMode, EffectConfig, LayerConfig, NamedZone, Zone};
use ipc::{AudioState, Profile};
use k100::Rgb;

use crate::colors::color_button;
use crate::model::Model;

const ACCENT: Rgb = Rgb::new(0x00, 0xBF, 0xFF);
const MAGENTA: Rgb = Rgb::new(0xFF, 0x00, 0x80);
const MAX_COLORS: usize = 8;

pub const NAMED_ZONES: [(NamedZone, &str); 11] = [
    (NamedZone::All, "Everything"),
    (NamedZone::Keys, "Keys"),
    (NamedZone::Wasd, "WASD"),
    (NamedZone::Numpad, "Numpad"),
    (NamedZone::Arrows, "Arrows"),
    (NamedZone::GKeys, "G keys"),
    (NamedZone::Media, "Media keys"),
    (NamedZone::LightBar, "Light bar"),
    (NamedZone::Underglow, "Underglow"),
    (NamedZone::DialRing, "Dial ring"),
    (NamedZone::Logo, "Logo"),
];

pub fn zone_label(zone: &Zone) -> String {
    match zone {
        Zone::Named(named) => {
            NAMED_ZONES.iter().find(|(zone, _)| zone == named).map_or("Zone", |(_, label)| *label).to_owned()
        }
        Zone::Leds(leds) => format!("{} chosen keys", leds.len()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    Solid,
    StaticCustom,
    Gradient,
    RainbowWave,
    ColorWave,
    SpiralRainbow,
    Visor,
    ColorShift,
    ColorPulse,
    Rain,
    Type,
    Ripple,
    AudioVisualizer,
}

impl EffectKind {
    pub const ALL: [EffectKind; 13] = [
        Self::Solid,
        Self::StaticCustom,
        Self::Gradient,
        Self::RainbowWave,
        Self::ColorWave,
        Self::SpiralRainbow,
        Self::Visor,
        Self::ColorShift,
        Self::ColorPulse,
        Self::Rain,
        Self::Type,
        Self::Ripple,
        Self::AudioVisualizer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Solid => "Solid",
            Self::StaticCustom => "Static custom",
            Self::Gradient => "Gradient",
            Self::RainbowWave => "Rainbow wave",
            Self::ColorWave => "Colour wave",
            Self::SpiralRainbow => "Spiral rainbow",
            Self::Visor => "Visor",
            Self::ColorShift => "Colour shift",
            Self::ColorPulse => "Colour pulse",
            Self::Rain => "Rain",
            Self::Type => "Type",
            Self::Ripple => "Ripple",
            Self::AudioVisualizer => "Audio visualizer",
        }
    }

    pub fn of(config: &EffectConfig) -> Self {
        match config {
            EffectConfig::Solid { .. } => Self::Solid,
            EffectConfig::StaticCustom { .. } => Self::StaticCustom,
            EffectConfig::Gradient { .. } => Self::Gradient,
            EffectConfig::RainbowWave { .. } => Self::RainbowWave,
            EffectConfig::ColorWave { .. } => Self::ColorWave,
            EffectConfig::SpiralRainbow { .. } => Self::SpiralRainbow,
            EffectConfig::Visor { .. } => Self::Visor,
            EffectConfig::ColorShift { .. } => Self::ColorShift,
            EffectConfig::ColorPulse { .. } => Self::ColorPulse,
            EffectConfig::Rain { .. } => Self::Rain,
            EffectConfig::Type { .. } => Self::Type,
            EffectConfig::Ripple { .. } => Self::Ripple,
            EffectConfig::AudioVisualizer { .. } => Self::AudioVisualizer,
        }
    }

    pub fn default_config(self) -> EffectConfig {
        let black = Rgb::BLACK;
        match self {
            Self::Solid => EffectConfig::Solid { color: ACCENT },
            Self::StaticCustom => EffectConfig::StaticCustom { base: black, keys: Vec::new() },
            Self::Gradient => EffectConfig::Gradient { from: ACCENT, to: MAGENTA, angle: 0.0 },
            Self::RainbowWave => EffectConfig::RainbowWave { speed: 0.3, angle: 0.0, wavelength: 1.0 },
            Self::ColorWave => {
                EffectConfig::ColorWave { color: ACCENT, background: black, speed: 0.5, width: 0.2, angle: 0.0 }
            }
            Self::SpiralRainbow => EffectConfig::SpiralRainbow { speed: 0.3 },
            Self::Visor => {
                EffectConfig::Visor { color: Rgb::new(255, 0, 0), background: black, speed: 0.5, width: 0.2 }
            }
            Self::ColorShift => EffectConfig::ColorShift { colors: vec![ACCENT, MAGENTA], speed: 0.5 },
            Self::ColorPulse => EffectConfig::ColorPulse { colors: vec![ACCENT, black], speed: 0.5 },
            Self::Rain => EffectConfig::Rain { color: ACCENT, background: black, speed: 0.5, columns: 24 },
            Self::Type => EffectConfig::Type { color: ACCENT, background: black, fade_ms: 1000 },
            Self::Ripple => EffectConfig::Ripple {
                color: ACCENT,
                background: black,
                speed: 1500.0,
                width: 120.0,
                lifetime_ms: 1500,
            },
            Self::AudioVisualizer => EffectConfig::AudioVisualizer {
                low: Rgb::new(0x00, 0xFF, 0x40),
                high: Rgb::new(0xFF, 0x00, 0x00),
                background: black,
                gain: 1.0,
                decay_ms: 400,
            },
        }
    }
}

fn unique_name(layers: &[LayerConfig], base: &str, first_suffix: Option<u32>) -> String {
    let taken = |name: &str| layers.iter().any(|layer| layer.name == name);
    let mut candidate = match first_suffix {
        Some(number) => format!("{base} {number}"),
        None => base.to_owned(),
    };
    let mut number = first_suffix.map_or(2, |first| first + 1);
    while taken(&candidate) {
        candidate = format!("{base} {number}");
        number += 1;
    }
    candidate
}

pub fn new_layer(layers: &[LayerConfig], kind: EffectKind) -> LayerConfig {
    let name = unique_name(layers, kind.label(), Some(1));
    LayerConfig::new(&name, Zone::Named(NamedZone::All), kind.default_config())
}

pub fn duplicate_layer(layers: &[LayerConfig], index: usize) -> LayerConfig {
    let mut copy = layers[index].clone();
    copy.name = unique_name(layers, &format!("{} copy", layers[index].name), None);
    copy
}

pub fn rename_layer(profile: &mut Profile, index: usize, name: &str) {
    let old = std::mem::replace(&mut profile.layers[index].name, name.to_owned());
    let rename = |action: &mut Action| {
        if let Action::LightingTrigger { layer, .. } = action
            && *layer == old
        {
            *layer = name.to_owned();
        }
    };
    for assignment in &mut profile.assignments {
        rename(&mut assignment.action);
    }
    for mode in &mut profile.dial.modes {
        for action in [&mut mode.clockwise, &mut mode.counter_clockwise, &mut mode.press].into_iter().flatten() {
            rename(action);
        }
    }
}

pub fn commit_name(profile: &mut Profile, original: &str, typed: &str) -> Result<(), String> {
    let name = typed.trim();
    let Some(index) = profile.layers.iter().position(|layer| layer.name == original) else { return Ok(()) };
    if name.is_empty() || name == original {
        return Ok(());
    }
    if profile.layers.iter().any(|layer| layer.name == name) {
        return Err(format!("two layers are called {name:?}"));
    }
    rename_layer(profile, index, name);
    Ok(())
}

fn name_row(
    ui: &mut egui::Ui,
    profile: &mut Profile,
    edit: &mut Option<(String, String)>,
    index: usize,
) -> Option<String> {
    let current = profile.layers[index].name.clone();
    let mut problem = None;
    if let Some((original, typed)) = edit.take_if(|(original, _)| *original != current) {
        problem = commit_name(profile, &original, &typed).err();
    }
    let mut buffer = edit.as_ref().map_or_else(|| current.clone(), |(_, typed)| typed.clone());
    let response = ui
        .horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut buffer)
        })
        .inner;
    if response.changed() {
        *edit = Some((current.clone(), buffer.clone()));
    }
    if response.lost_focus() {
        *edit = None;
        problem = commit_name(profile, &current, &buffer).err().or(problem);
    }
    problem
}

pub fn paint_selection(keys: &mut Vec<KeyColor>, selection: &BTreeSet<u8>, color: Rgb) {
    for led in selection {
        match keys.iter_mut().find(|entry| entry.led == *led) {
            Some(entry) => entry.color = color,
            None => keys.push(KeyColor { led: *led, color }),
        }
    }
}

pub fn lighting(ui: &mut egui::Ui, model: &mut Model) {
    crate::canvas::pane(ui, model);
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |columns| {
            layer_list(&mut columns[0], model);
            layer_editor(&mut columns[1], model);
        });
    });
}

fn layer_list(ui: &mut egui::Ui, model: &mut Model) {
    let Some(draft) = model.draft.as_mut() else { return };
    let layers = &mut draft.profile.layers;
    egui::ComboBox::from_label("Add layer").selected_text("Choose an effect").show_ui(ui, |ui| {
        for kind in EffectKind::ALL {
            if ui.selectable_label(false, kind.label()).clicked() {
                let layer = new_layer(layers, kind);
                layers.push(layer);
                model.layer = layers.len() - 1;
            }
        }
    });
    if layers.is_empty() {
        ui.label("No layers yet. Add one to light the keyboard.");
        return;
    }
    model.layer = model.layer.min(layers.len() - 1);
    for index in (0..layers.len()).rev() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut layers[index].enabled, "");
            if ui.selectable_label(model.layer == index, layers[index].name.as_str()).clicked() {
                model.layer = index;
            }
        });
    }
    ui.horizontal(|ui| {
        let index = model.layer;
        if ui.button("Raise").clicked()
            && let Some(moved) = crate::model::move_item(layers, index, true)
        {
            model.layer = moved;
        }
        if ui.button("Lower").clicked()
            && let Some(moved) = crate::model::move_item(layers, index, false)
        {
            model.layer = moved;
        }
        if ui.button("Duplicate").clicked() {
            let copy = duplicate_layer(layers, index);
            layers.insert(index + 1, copy);
            model.layer = index + 1;
        }
        if ui.button("Remove").clicked() {
            layers.remove(index);
            model.layer = index.saturating_sub(1);
        }
    });
}

fn layer_editor(ui: &mut egui::Ui, model: &mut Model) {
    let selection = model.selection.clone();
    let audio = model.state.as_ref().map(|state| state.audio.clone()).unwrap_or_default();
    let Some(draft) = model.draft.as_mut() else { return };
    if draft.profile.layers.is_empty() {
        return;
    }
    let index = model.layer.min(draft.profile.layers.len() - 1);
    if let Some(problem) = name_row(ui, &mut draft.profile, &mut model.name_edit, index) {
        model.toast(problem);
    }
    let Some(draft) = model.draft.as_mut() else { return };
    let layer = &mut draft.profile.layers[index];
    ui.horizontal(|ui| {
        egui::ComboBox::from_label("Zone").selected_text(zone_label(&layer.zone)).show_ui(ui, |ui| {
            for (zone, label) in NAMED_ZONES {
                if ui.selectable_label(layer.zone == Zone::Named(zone), label).clicked() {
                    layer.zone = Zone::Named(zone);
                }
            }
        });
        if ui.add_enabled(!selection.is_empty(), egui::Button::new("Use selected keys")).clicked() {
            layer.zone = Zone::Leds(selection.iter().copied().collect());
        }
    });
    ui.horizontal(|ui| {
        let blend_label = |blend: BlendMode| match blend {
            BlendMode::Normal => "Normal",
            BlendMode::Add => "Add",
            BlendMode::Multiply => "Multiply",
        };
        egui::ComboBox::from_label("Blend").selected_text(blend_label(layer.blend)).show_ui(ui, |ui| {
            for blend in [BlendMode::Normal, BlendMode::Add, BlendMode::Multiply] {
                ui.selectable_value(&mut layer.blend, blend, blend_label(blend));
            }
        });
    });
    ui.add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).text("Opacity"));
    let mut kind = EffectKind::of(&layer.effect);
    egui::ComboBox::from_label("Effect").selected_text(kind.label()).show_ui(ui, |ui| {
        for choice in EffectKind::ALL {
            ui.selectable_value(&mut kind, choice, choice.label());
        }
    });
    if kind != EffectKind::of(&layer.effect) {
        layer.effect = kind.default_config();
    }
    ui.separator();
    effect_params(ui, &mut layer.effect, &selection, &mut model.paint);
    if matches!(layer.effect, EffectConfig::AudioVisualizer { .. }) {
        ui.label(audio_status_label(&audio));
    }
}

fn audio_status_label(audio: &AudioState) -> String {
    match audio {
        AudioState::Off => "Audio off (starts when this profile is active and the keyboard is connected)".to_owned(),
        AudioState::Starting => "Audio starting".to_owned(),
        AudioState::Capturing { device } => format!("Audio capturing: {device}"),
        AudioState::Failed { reason } => format!("Audio failed: {reason}"),
    }
}

fn colour_row(ui: &mut egui::Ui, label: &str, color: &mut Rgb) {
    ui.horizontal(|ui| {
        ui.label(label);
        color_button(ui, color);
    });
}

fn colour_list(ui: &mut egui::Ui, colors: &mut Vec<Rgb>) {
    let mut remove = None;
    for (index, color) in colors.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("Colour {}", index + 1));
            color_button(ui, color);
        });
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(colors.len() < MAX_COLORS, egui::Button::new("Add colour")).clicked() {
            colors.push(ACCENT);
        }
        if ui.add_enabled(colors.len() > 1, egui::Button::new("Remove last colour")).clicked() {
            remove = Some(colors.len() - 1);
        }
    });
    if let Some(index) = remove {
        colors.remove(index);
    }
}

fn speed_slider(ui: &mut egui::Ui, value: &mut f32) {
    ui.add(egui::Slider::new(value, 0.0..=3.0).text("Speed"));
}

fn angle_slider(ui: &mut egui::Ui, value: &mut f32) {
    ui.add(egui::Slider::new(value, 0.0..=360.0).text("Angle"));
}

fn width_slider(ui: &mut egui::Ui, value: &mut f32) {
    ui.add(egui::Slider::new(value, 0.05..=1.0).text("Width"));
}

pub fn effect_params(ui: &mut egui::Ui, effect: &mut EffectConfig, selection: &BTreeSet<u8>, paint: &mut Rgb) {
    match effect {
        EffectConfig::Solid { color } => colour_row(ui, "Colour", color),
        EffectConfig::StaticCustom { base, keys } => {
            colour_row(ui, "Base colour", base);
            ui.label(format!("{} painted keys", keys.len()));
            colour_row(ui, "Paint colour", paint);
            ui.horizontal(|ui| {
                if ui.add_enabled(!selection.is_empty(), egui::Button::new("Paint selected keys")).clicked() {
                    paint_selection(keys, selection, *paint);
                }
                if ui.add_enabled(!keys.is_empty(), egui::Button::new("Clear painted keys")).clicked() {
                    keys.clear();
                }
            });
        }
        EffectConfig::Gradient { from, to, angle } => {
            colour_row(ui, "From", from);
            colour_row(ui, "To", to);
            angle_slider(ui, angle);
        }
        EffectConfig::RainbowWave { speed, angle, wavelength } => {
            speed_slider(ui, speed);
            angle_slider(ui, angle);
            ui.add(egui::Slider::new(wavelength, 0.2..=4.0).text("Wavelength"));
        }
        EffectConfig::ColorWave { color, background, speed, width, angle } => {
            colour_row(ui, "Colour", color);
            colour_row(ui, "Background", background);
            speed_slider(ui, speed);
            width_slider(ui, width);
            angle_slider(ui, angle);
        }
        EffectConfig::SpiralRainbow { speed } => speed_slider(ui, speed),
        EffectConfig::Visor { color, background, speed, width } => {
            colour_row(ui, "Colour", color);
            colour_row(ui, "Background", background);
            speed_slider(ui, speed);
            width_slider(ui, width);
        }
        EffectConfig::ColorShift { colors, speed } | EffectConfig::ColorPulse { colors, speed } => {
            colour_list(ui, colors);
            speed_slider(ui, speed);
        }
        EffectConfig::Rain { color, background, speed, columns } => {
            colour_row(ui, "Colour", color);
            colour_row(ui, "Background", background);
            speed_slider(ui, speed);
            ui.add(egui::Slider::new(columns, 4..=40).text("Columns"));
        }
        EffectConfig::Type { color, background, fade_ms } => {
            colour_row(ui, "Colour", color);
            colour_row(ui, "Background", background);
            ui.add(egui::Slider::new(fade_ms, 100..=5000).text("Fade (ms)"));
        }
        EffectConfig::Ripple { color, background, speed, width, lifetime_ms } => {
            colour_row(ui, "Colour", color);
            colour_row(ui, "Background", background);
            ui.add(egui::Slider::new(speed, 200.0..=4000.0).text("Spread speed"));
            ui.add(egui::Slider::new(width, 20.0..=400.0).text("Ring width"));
            ui.add(egui::Slider::new(lifetime_ms, 200..=4000).text("Lifetime (ms)"));
        }
        EffectConfig::AudioVisualizer { low, high, background, gain, decay_ms } => {
            colour_row(ui, "Bottom colour", low);
            colour_row(ui, "Top colour", high);
            colour_row(ui, "Background", background);
            ui.add(egui::Slider::new(gain, 0.1..=10.0).logarithmic(true).text("Gain"));
            ui.add(egui::Slider::new(decay_ms, 0..=2000).text("Decay (ms)"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use actions::{Action, Assignment, Key};
    use eframe::egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    use engine::effects::KeyColor;
    use engine::{EffectConfig, LayerConfig, NamedZone, Zone};
    use ipc::{Reply, Request};
    use k100::Rgb;

    use super::*;
    use crate::model::{AUTO_APPLY_QUIET, Model};
    use crate::testing::{harness, online, sent};

    fn solid(name: &str) -> LayerConfig {
        LayerConfig::new(name, Zone::Named(NamedZone::All), EffectConfig::Solid { color: Rgb::BLACK })
    }

    fn names(model: &Model) -> Vec<String> {
        model.draft.as_ref().unwrap().profile.layers.iter().map(|layer| layer.name.clone()).collect()
    }

    fn with_layers(layers: Vec<LayerConfig>) -> Model {
        let mut model = online("p", &["p"]);
        model.draft.as_mut().unwrap().profile.layers = layers;
        model
    }

    #[test]
    fn every_effect_kind_round_trips_and_builds() {
        assert_eq!(EffectKind::ALL.len(), 13);
        for kind in EffectKind::ALL {
            let config = kind.default_config();
            assert_eq!(EffectKind::of(&config), kind);
            drop(config.build());
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn new_layers_get_unique_names() {
        let existing = vec![solid("Rainbow wave 1"), solid("Rainbow wave 3")];
        assert_eq!(new_layer(&existing, EffectKind::RainbowWave).name, "Rainbow wave 2");
        assert_eq!(new_layer(&[], EffectKind::Solid).name, "Solid 1");
    }

    #[test]
    fn copies_get_unique_names_and_keep_their_settings() {
        let mut layers = vec![solid("glow")];
        layers[0].opacity = 0.4;
        let copy = duplicate_layer(&layers, 0);
        assert_eq!((copy.name.as_str(), copy.opacity), ("glow copy", 0.4));
        layers.push(copy);
        assert_eq!(duplicate_layer(&layers, 0).name, "glow copy 2");
    }

    #[test]
    fn renaming_a_layer_follows_the_triggers_that_use_it() {
        let trigger = |layer: &str| Action::LightingTrigger { layer: layer.into(), duration_ms: None };
        let mut profile = crate::testing::profile("p");
        profile.layers = vec![solid("alert")];
        profile.assignments =
            vec![Assignment { key: Key::named("g1").unwrap(), action: trigger("alert"), pass_through: vec![] }];
        profile.dial.modes[0].kind = actions::DialModeKind::Custom;
        profile.dial.modes[0].press = Some(trigger("alert"));
        rename_layer(&mut profile, 0, "warning");
        assert_eq!(profile.layers[0].name, "warning");
        assert_eq!(profile.assignments[0].action, trigger("warning"));
        assert_eq!(profile.dial.modes[0].press, Some(trigger("warning")));
        assert!(crate::model::validate_profile(&profile).is_ok());
    }

    #[test]
    fn painting_replaces_existing_entries_and_adds_new_ones() {
        let mut keys = vec![KeyColor { led: 4, color: Rgb::new(1, 1, 1) }];
        paint_selection(&mut keys, &BTreeSet::from([4, 26]), Rgb::new(9, 9, 9));
        assert_eq!(
            keys,
            [KeyColor { led: 4, color: Rgb::new(9, 9, 9) }, KeyColor { led: 26, color: Rgb::new(9, 9, 9) }]
        );
    }

    #[test]
    fn zones_have_readable_labels() {
        assert_eq!(zone_label(&Zone::Named(NamedZone::GKeys)), "G keys");
        assert_eq!(zone_label(&Zone::Leds(vec![1, 2, 3])), "3 chosen keys");
        assert_eq!(NAMED_ZONES.len(), 11);
    }

    #[test]
    fn an_empty_profile_invites_the_first_layer() {
        let mut harness = harness(with_layers(vec![]), lighting);
        harness.run();
        harness.get_by_label("No layers yet. Add one to light the keyboard.");
    }

    #[test]
    fn adding_a_layer_from_the_menu_selects_it() {
        let mut harness = harness(with_layers(vec![solid("base")]), lighting);
        harness.run();
        harness.get_by_label("Add layer").click();
        harness.run();
        harness.get_by_label("Rainbow wave").click();
        harness.run();
        assert_eq!(names(harness.state()), ["base", "Rainbow wave 1"]);
        assert_eq!(harness.state().layer, 1);
    }

    #[test]
    fn choosing_the_audio_visualizer_in_the_effect_combo_installs_its_defaults() {
        let mut harness = harness(with_layers(vec![solid("base")]), lighting);
        harness.run();
        harness.get_by_label("Effect").click();
        harness.run();
        harness.get_by_label("Audio visualizer").click_accesskit();
        harness.run();
        let layer = &harness.state().draft.as_ref().unwrap().profile.layers[0];
        assert_eq!(layer.effect, EffectKind::AudioVisualizer.default_config());
        assert_eq!(
            layer.effect,
            EffectConfig::AudioVisualizer {
                low: Rgb::new(0x00, 0xFF, 0x40),
                high: Rgb::new(0xFF, 0x00, 0x00),
                background: Rgb::BLACK,
                gain: 1.0,
                decay_ms: 400,
            }
        );
    }

    fn visualizer_model(audio: AudioState) -> Model {
        let mut model =
            with_layers(vec![LayerConfig { effect: EffectKind::AudioVisualizer.default_config(), ..solid("bars") }]);
        model.state.as_mut().unwrap().audio = audio;
        model
    }

    #[test]
    fn the_visualizer_editor_shows_the_audio_status() {
        let mut harness = harness(visualizer_model(AudioState::Failed { reason: "boom".into() }), lighting);
        harness.run();
        harness.get_by_label("Audio failed: boom");

        let mut harness =
            self::harness(visualizer_model(AudioState::Capturing { device: "Speakers".into() }), lighting);
        harness.run();
        harness.get_by_label("Audio capturing: Speakers");
    }

    #[test]
    fn the_visualizer_editor_explains_when_audio_is_off() {
        let mut harness = harness(visualizer_model(AudioState::Off), lighting);
        harness.run();
        harness.get_by_label("Audio off (starts when this profile is active and the keyboard is connected)");
    }

    #[test]
    fn other_editors_do_not_show_the_audio_status() {
        let mut model = with_layers(vec![solid("base")]);
        model.state.as_mut().unwrap().audio = AudioState::Failed { reason: "boom".into() };
        let mut harness = harness(model, lighting);
        harness.run();
        assert!(harness.query_by_label("Audio failed: boom").is_none());
    }

    #[test]
    fn raise_moves_the_selected_layer_up_and_keeps_it_selected() {
        let mut model = with_layers(vec![solid("a"), solid("b"), solid("c")]);
        model.layer = 0;
        let mut harness = harness(model, lighting);
        harness.run();
        harness.get_by_label("Raise").click();
        harness.run();
        assert_eq!(names(harness.state()), ["b", "a", "c"]);
        assert_eq!(harness.state().layer, 1);
    }

    #[test]
    fn lower_and_remove_work_on_the_selected_layer() {
        let mut harness = harness(with_layers(vec![solid("base"), solid("top")]), lighting);
        harness.run();
        harness.get_by_label("top").click();
        harness.run();
        assert_eq!(harness.state().layer, 1);
        harness.get_by_label("Lower").click();
        harness.run();
        assert_eq!(names(harness.state()), ["top", "base"]);
        assert_eq!(harness.state().layer, 0);
        harness.get_by_label("Remove").click();
        harness.run();
        assert_eq!(names(harness.state()), ["base"]);
        assert_eq!(harness.state().layer, 0);
    }

    #[test]
    fn the_canvas_selection_becomes_the_layers_zone() {
        let mut model = with_layers(vec![solid("base")]);
        model.selection = BTreeSet::from([26, 4]);
        let mut harness = harness(model, lighting);
        harness.run();
        harness.get_by_label("Use selected keys").click();
        harness.run();
        let zone = &harness.state().draft.as_ref().unwrap().profile.layers[0].zone;
        assert_eq!(*zone, Zone::Leds(vec![4, 26]));
    }

    #[test]
    fn duplicating_inserts_a_copy_above_the_original_and_selects_it() {
        let mut harness = harness(with_layers(vec![solid("same"), solid("other")]), lighting);
        harness.run();
        harness.get_by_label("other").click();
        harness.run();
        harness.get_by_label("Duplicate").click();
        harness.run();
        assert_eq!(names(harness.state()), ["same", "other", "other copy"]);
        assert_eq!(harness.state().layer, 2);
        harness.state_mut().draft.as_mut().unwrap().profile.layers[0].name = "other".into();
        assert!(harness.state().draft_problem().unwrap().contains("two layers"));
        let model = harness.state_mut();
        model.tick(Duration::ZERO);
        model.tick(AUTO_APPLY_QUIET * 3);
        model.tick(AUTO_APPLY_QUIET * 6);
        assert!(sent(model).is_empty());
    }

    #[test]
    fn every_effect_kind_shows_an_editor_that_leaves_it_untouched() {
        for kind in EffectKind::ALL {
            let mut layer = solid("fx");
            layer.effect = kind.default_config();
            let mut harness = harness(with_layers(vec![layer.clone()]), lighting);
            harness.run();
            harness.get_by_label("Name");
            assert_eq!(harness.state().draft.as_ref().unwrap().profile.layers, [layer], "{}", kind.label());
        }
    }

    #[test]
    fn painting_selected_keys_from_the_editor_stores_them_in_the_effect() {
        let mut layer = solid("fx");
        layer.effect = EffectKind::StaticCustom.default_config();
        let mut model = with_layers(vec![layer]);
        model.selection = BTreeSet::from([26, 4]);
        model.paint = Rgb::new(9, 9, 9);
        let mut harness = harness(model, lighting);
        harness.run();
        harness.get_by_label("Paint selected keys").click();
        harness.run();
        let EffectConfig::StaticCustom { keys, .. } = &harness.state().draft.as_ref().unwrap().profile.layers[0].effect
        else {
            panic!("the effect changed kind");
        };
        let painted = Rgb::new(9, 9, 9);
        assert_eq!(keys, &[KeyColor { led: 4, color: painted }, KeyColor { led: 26, color: painted }]);
    }

    fn alert_profile() -> Profile {
        let mut profile = crate::testing::profile("p");
        profile.layers = vec![solid("a"), solid("ab")];
        profile.assignments = vec![Assignment {
            key: Key::named("g1").unwrap(),
            action: Action::LightingTrigger { layer: "ab".into(), duration_ms: None },
            pass_through: vec![],
        }];
        profile
    }

    #[test]
    fn committing_a_name_follows_triggers_and_ignores_empty_or_unchanged_names() {
        let mut profile = alert_profile();
        assert_eq!(commit_name(&mut profile, "ab", "  "), Ok(()));
        assert_eq!(commit_name(&mut profile, "ab", "ab"), Ok(()));
        assert_eq!(profile.layers[1].name, "ab");
        assert_eq!(commit_name(&mut profile, "ab", " warn "), Ok(()));
        assert_eq!(profile.layers[1].name, "warn");
        let Action::LightingTrigger { layer, .. } = &profile.assignments[0].action else { panic!("not a trigger") };
        assert_eq!(layer, "warn");
    }

    #[test]
    fn committing_a_name_another_layer_has_is_refused() {
        let mut profile = alert_profile();
        let before = profile.clone();
        let problem = commit_name(&mut profile, "a", "ab").unwrap_err();
        assert!(problem.contains("two layers are called"));
        assert_eq!(profile, before);
    }

    #[test]
    fn typing_through_a_colliding_name_leaves_other_triggers_alone() {
        let mut model = online("p", &["p"]);
        model.draft = Some(crate::model::Draft::new(alert_profile()));
        let mut harness = harness(model, lighting);
        harness.run();
        let field = harness.get_by_role(Role::TextInput);
        field.click();
        harness.run();
        harness.get_by_role(Role::TextInput).type_text("b");
        harness.run();
        let profile = &harness.state().draft.as_ref().unwrap().profile;
        assert_eq!(profile.layers[0].name, "a", "the name is only committed when the field loses focus");
        assert_eq!(profile.assignments[0].action, Action::LightingTrigger { layer: "ab".into(), duration_ms: None });
        harness.get_by_role(Role::TextInput).type_text("c");
        harness.run();
        harness.get_by_label("Name").click();
        harness.run();
        let profile = &harness.state().draft.as_ref().unwrap().profile;
        assert_eq!(names(harness.state()), ["abc", "ab"]);
        assert_eq!(profile.assignments[0].action, Action::LightingTrigger { layer: "ab".into(), duration_ms: None });
        assert!(harness.state().toasts.is_empty());
    }

    #[test]
    fn a_refused_name_is_reverted_and_reported() {
        let mut model = online("p", &["p"]);
        model.draft = Some(crate::model::Draft::new(alert_profile()));
        let mut harness = harness(model, lighting);
        harness.run();
        harness.get_by_role(Role::TextInput).click();
        harness.run();
        harness.get_by_role(Role::TextInput).type_text("b");
        harness.run();
        harness.get_by_label("Name").click();
        harness.run();
        assert_eq!(names(harness.state()), ["a", "ab"]);
        assert!(harness.state().toasts[0].contains("two layers are called"));
    }

    fn type_a_name(harness: &mut egui_kittest::Harness<'_, Model>) {
        harness.get_by_role(Role::TextInput).click();
        harness.run();
        harness.get_by_role(Role::TextInput).type_text("x");
        harness.run();
        assert_eq!(harness.state().name_edit, Some(("glow".into(), "glowx".into())));
    }

    fn blur_and_check(harness: &mut egui_kittest::Harness<'_, Model>, profile: &str) {
        harness.run();
        harness.get_by_label("Name").click();
        harness.run();
        let draft = harness.state().draft.as_ref().unwrap();
        assert_eq!(draft.profile.name, profile);
        assert_eq!(names(harness.state()), ["glow"]);
        assert_eq!(harness.get_by_role(Role::TextInput).accesskit_node().value().as_deref(), Some("glow"));
    }

    fn glow_profile(name: &str) -> Profile {
        let mut profile = crate::testing::profile(name);
        profile.layers = vec![solid("glow")];
        profile
    }

    #[test]
    fn a_typed_name_is_not_replayed_onto_a_profile_loaded_meanwhile() {
        let mut model = online("p", &["p", "games"]);
        model.draft = Some(crate::model::Draft::new(glow_profile("p")));
        let mut harness = harness(model, lighting);
        harness.run();
        type_a_name(&mut harness);
        let games = glow_profile("games");
        let reply = Reply::Profile { profile: games };
        harness.state_mut().apply(crate::model::Inbox::reply(Request::GetProfile { name: "games".into() }, reply));
        blur_and_check(&mut harness, "games");
    }

    #[test]
    fn a_typed_name_is_not_replayed_after_reverting() {
        let mut model = online("p", &["p"]);
        model.draft = Some(crate::model::Draft::new(glow_profile("p")));
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
        let mut harness = harness(model, lighting);
        harness.run();
        type_a_name(&mut harness);
        harness.state_mut().discard_draft();
        harness.run();
        let reply = Reply::Profile { profile: glow_profile("p") };
        harness.state_mut().apply(crate::model::Inbox::reply(Request::GetProfile { name: "p".into() }, reply));
        blur_and_check(&mut harness, "p");
    }
}

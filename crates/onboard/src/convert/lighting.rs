use std::collections::HashMap;

use engine::effects::{EffectConfig, KeyColor};
use engine::{BlendMode, LayerConfig, Layout, LedKind};
use k100::{LED_COUNT, Rgb};

use super::Report;
use crate::{CommonLighting, Direction, EffectDuration, EffectKind, HwColor, SPECIFIED_COLORS, Speed};

pub const MAX_EFFECTS: usize = 20;

const DEFAULT_WIDTH: f32 = 0.2;
const DEFAULT_WAVELENGTH: f32 = 1.0;
const DEFAULT_COLUMNS: u8 = 24;
const DEFAULT_RIPPLE_WIDTH: f32 = 120.0;
const DEFAULT_LIFETIME_MS: u64 = 1500;

const SLOW_BELOW: f32 = 0.35;
const MEDIUM_BELOW: f32 = 0.75;
const RIPPLE_SPEED_SCALE: f32 = 0.5 / 1500.0;
const SHORT_FADE_MS: u64 = 600;
const MEDIUM_FADE_MS: u64 = 1500;
const COLORS_PER_EFFECT: usize = 2;
const QUARTER_TURN: f32 = 90.0;
const ANGLE_TOLERANCE: f32 = 1e-3;

pub fn convert_layers(layers: &[LayerConfig], layout: &Layout, report: &mut Report) -> Vec<CommonLighting> {
    let mut claimed = [false; LED_COUNT];
    let mut files_by_layer: Vec<Vec<CommonLighting>> = Vec::new();
    let mut file_count = 0;
    for layer in layers.iter().rev() {
        let mut notes = Notes { layer: &layer.name, report };
        let files = convert_layer(layer, layout, &claimed, MAX_EFFECTS - file_count, &mut notes);
        for led in files.iter().flat_map(|file| &file.leds) {
            claimed[*led as usize] = true;
        }
        file_count += files.len();
        if !files.is_empty() {
            files_by_layer.push(files);
        }
    }
    files_by_layer.into_iter().rev().flatten().collect()
}

struct Notes<'a> {
    layer: &'a str,
    report: &'a mut Report,
}

impl Notes<'_> {
    fn skip(&mut self, reason: &str) {
        self.report.skipped.push(format!("layer \"{}\": {reason}", self.layer));
    }

    fn approximate(&mut self, what: &str) {
        self.report.approximated.push(format!("layer \"{}\": {what}", self.layer));
    }

    fn no_hardware_setting(&mut self, parameter: &str, differs_from_default: bool) {
        if differs_from_default {
            self.approximate(&format!("{parameter} has no hardware setting"));
        }
    }
}

fn convert_layer(
    layer: &LayerConfig,
    layout: &Layout,
    claimed: &[bool; LED_COUNT],
    room: usize,
    notes: &mut Notes,
) -> Vec<CommonLighting> {
    if !layer.enabled {
        notes.skip("disabled (lighting triggers have no hardware equivalent)");
        return Vec::new();
    }
    if let Some(reason) = unsupported(&layer.effect) {
        notes.skip(reason);
        return Vec::new();
    }
    let zone_leds = writable_leds(layer, layout);
    if zone_leds.is_empty() {
        notes.skip("its zone has no LEDs the hardware can light");
        return Vec::new();
    }
    let free: Vec<u8> = zone_leds.into_iter().filter(|led| !claimed[*led as usize]).collect();
    if free.is_empty() {
        notes.skip("hidden by the layers above");
        return Vec::new();
    }
    if room == 0 {
        notes.skip(&format!("over the {MAX_EFFECTS} hardware lighting effects"));
        return Vec::new();
    }
    let mut files = effect_files(&layer.effect, &free, notes);
    if files.len() > room {
        notes.skip(&format!(
            "{} of {} files over the {MAX_EFFECTS} hardware lighting effects",
            files.len() - room,
            files.len()
        ));
        files.truncate(room);
    }
    if layer.opacity < 1.0 || layer.blend != BlendMode::Normal {
        notes.approximate("opacity and blend mode have no hardware setting, written opaque");
    }
    files
}

fn unsupported(effect: &EffectConfig) -> Option<&'static str> {
    match effect {
        EffectConfig::Gradient { .. } => Some("no hardware layout in v1"),
        EffectConfig::AudioVisualizer { .. } => Some("needs the PC"),
        EffectConfig::ColorShift { colors, .. } | EffectConfig::ColorPulse { colors, .. } if colors.is_empty() => {
            Some("the effect has no colours")
        }
        _ => None,
    }
}

fn writable_leds(layer: &LayerConfig, layout: &Layout) -> Vec<u8> {
    let mask = layer.zone.mask(layout);
    (0..LED_COUNT as u8)
        .filter(|&led| mask[led as usize])
        .filter(|&led| layout.get(led).is_some_and(|geometry| geometry.kind != LedKind::Indicator))
        .collect()
}

fn effect_files(effect: &EffectConfig, leds: &[u8], notes: &mut Notes) -> Vec<CommonLighting> {
    let leds = leds.to_vec();
    match effect {
        EffectConfig::Solid { color } => vec![CommonLighting::static_color(*color, leds)],
        EffectConfig::StaticCustom { base, keys } => static_custom_files(*base, keys, &leds),
        EffectConfig::RainbowWave { speed, angle, wavelength } => {
            notes.no_hardware_setting("wavelength", *wavelength != DEFAULT_WAVELENGTH);
            let direction = direction_from_angle(*angle, notes);
            vec![hardware(EffectKind::RainbowWave, thirds(*speed), direction, EffectDuration::None, &[], leds)]
        }
        EffectConfig::ColorWave { color, background, speed, width, angle } => {
            notes.no_hardware_setting("width", *width != DEFAULT_WIDTH);
            let direction = direction_from_angle(*angle, notes);
            let colors = [*color, *background];
            vec![hardware(EffectKind::ColorWave, thirds(*speed), direction, EffectDuration::None, &colors, leds)]
        }
        EffectConfig::SpiralRainbow { speed } => vec![hardware(
            EffectKind::SpiralRainbow,
            thirds(*speed),
            Direction::Clockwise,
            EffectDuration::None,
            &[],
            leds,
        )],
        EffectConfig::Visor { color, background, speed, width } => {
            notes.no_hardware_setting("width", *width != DEFAULT_WIDTH);
            let colors = [*color, *background];
            vec![hardware(EffectKind::Visor, thirds(*speed), Direction::Right, EffectDuration::None, &colors, leds)]
        }
        EffectConfig::ColorShift { colors, speed } => {
            vec![color_list_file(EffectKind::ColorShift, colors, *speed, leds, notes)]
        }
        EffectConfig::ColorPulse { colors, speed } => {
            vec![color_list_file(EffectKind::ColorPulse, colors, *speed, leds, notes)]
        }
        EffectConfig::Rain { color, background, speed, columns } => {
            notes.no_hardware_setting("columns", *columns != DEFAULT_COLUMNS);
            let colors = [*color, *background];
            vec![hardware(EffectKind::Rain, thirds(*speed), Direction::Down, EffectDuration::None, &colors, leds)]
        }
        EffectConfig::Type { color, background, fade_ms } => {
            let duration = fade_duration(*fade_ms);
            let colors = [*color, *background];
            vec![hardware(EffectKind::GradientType, speed_of(duration), Direction::None, duration, &colors, leds)]
        }
        EffectConfig::Ripple { color, background, speed, width, lifetime_ms } => {
            notes.no_hardware_setting("width", *width != DEFAULT_RIPPLE_WIDTH);
            notes.no_hardware_setting("lifetime_ms", *lifetime_ms != DEFAULT_LIFETIME_MS);
            let colors = [*color, *background];
            let ranked = thirds(speed * RIPPLE_SPEED_SCALE);
            vec![hardware(EffectKind::RippleType, ranked, Direction::None, EffectDuration::None, &colors, leds)]
        }
        EffectConfig::Gradient { .. } | EffectConfig::AudioVisualizer { .. } => Vec::new(),
    }
}

fn color_list_file(kind: EffectKind, colors: &[Rgb], speed: f32, leds: Vec<u8>, notes: &mut Notes) -> CommonLighting {
    if colors.len() > COLORS_PER_EFFECT {
        notes.approximate(&format!(
            "only the first {COLORS_PER_EFFECT} of {} colours fit, the rest are dropped",
            colors.len()
        ));
    }
    let kept = &colors[..colors.len().min(COLORS_PER_EFFECT)];
    hardware(kind, thirds(speed), Direction::None, EffectDuration::None, kept, leds)
}

fn static_custom_files(base: Rgb, keys: &[KeyColor], leds: &[u8]) -> Vec<CommonLighting> {
    let key_colors: HashMap<u8, Rgb> = keys.iter().map(|key| (key.led, key.color)).collect();
    let mut order: Vec<Rgb> = Vec::new();
    for key in keys {
        let wins = key_colors[&key.led] == key.color;
        if wins && leds.contains(&key.led) && !order.contains(&key.color) {
            order.push(key.color);
        }
    }
    let base_leds: Vec<u8> = leds.iter().copied().filter(|led| !key_colors.contains_key(led)).collect();
    let groups = std::iter::once((base, base_leds)).chain(order.into_iter().map(|color| {
        let group = leds.iter().copied().filter(|led| key_colors.get(led) == Some(&color)).collect();
        (color, group)
    }));
    groups
        .filter(|(_, group)| !group.is_empty())
        .map(|(color, group)| CommonLighting::static_color(color, group))
        .collect()
}

fn hardware(
    kind: EffectKind,
    speed: Speed,
    direction: Direction,
    duration: EffectDuration,
    colors: &[Rgb],
    leds: Vec<u8>,
) -> CommonLighting {
    CommonLighting {
        kind,
        flags: if colors.is_empty() { 0 } else { SPECIFIED_COLORS },
        speed,
        direction,
        duration,
        colors: colors.iter().copied().map(HwColor::opaque).collect(),
        leds,
    }
}

fn thirds(speed: f32) -> Speed {
    if speed < SLOW_BELOW {
        Speed::Slow
    } else if speed < MEDIUM_BELOW {
        Speed::Medium
    } else {
        Speed::Fast
    }
}

fn fade_duration(fade_ms: u64) -> EffectDuration {
    if fade_ms <= SHORT_FADE_MS {
        EffectDuration::Short
    } else if fade_ms <= MEDIUM_FADE_MS {
        EffectDuration::Medium
    } else {
        EffectDuration::Long
    }
}

fn speed_of(duration: EffectDuration) -> Speed {
    match duration {
        EffectDuration::Short => Speed::Fast,
        EffectDuration::Medium => Speed::Medium,
        EffectDuration::Long => Speed::Slow,
        EffectDuration::None => Speed::None,
    }
}

fn direction_from_angle(angle: f32, notes: &mut Notes) -> Direction {
    let quarters = angle.rem_euclid(360.0) / QUARTER_TURN;
    let whole = quarters.floor();
    let rounded_up = quarters - whole > 0.5;
    let direction = match (whole as i32 + i32::from(rounded_up)).rem_euclid(4) {
        0 => Direction::Right,
        1 => Direction::Down,
        2 => Direction::Left,
        _ => Direction::Up,
    };
    if (quarters - quarters.round()).abs() > ANGLE_TOLERANCE {
        notes.approximate(&format!("angle {angle} rounded to {direction:?}"));
    }
    direction
}

#[cfg(test)]
mod tests {
    use engine::effects::KeyColor;
    use engine::{NamedZone, Zone};

    use super::*;
    use crate::{Direction, EffectDuration, EffectKind, SPECIFIED_COLORS, Speed};

    const RED: Rgb = Rgb::new(255, 0, 0);
    const BLUE: Rgb = Rgb::new(0, 0, 255);
    const WHITE: Rgb = Rgb::new(255, 255, 255);

    fn layer(name: &str, zone: Zone, effect: EffectConfig) -> LayerConfig {
        LayerConfig::new(name, zone, effect)
    }

    fn convert(layers: &[LayerConfig]) -> (Vec<CommonLighting>, Report) {
        let mut report = Report::default();
        let effects = convert_layers(layers, &Layout::ansi(), &mut report);
        (effects, report)
    }

    fn wasd(effect: EffectConfig) -> LayerConfig {
        layer("w", Zone::Named(NamedZone::Wasd), effect)
    }

    fn everywhere(effect: EffectConfig) -> LayerConfig {
        layer("all", Zone::Named(NamedZone::All), effect)
    }

    fn color_wave(angle: f32) -> EffectConfig {
        EffectConfig::ColorWave { color: RED, background: BLUE, speed: 0.5, width: 0.2, angle }
    }

    #[test]
    fn solid_over_wasd_is_one_static_color_file() {
        let (effects, report) = convert(&[wasd(EffectConfig::Solid { color: RED })]);
        assert_eq!(effects.len(), 1);
        assert_eq!(
            effects[0].encode(),
            [0x7E, 0x20, 0x01, 0, 0, 0, 0x01, 0xFF, 0, 0, 0xFF, 0x04, 0x04, 0x07, 0x16, 0x1A]
        );
        assert!(report.is_empty());
    }

    #[test]
    fn upper_layers_take_their_leds_away_from_lower_ones() {
        let rainbow = EffectConfig::RainbowWave { speed: 0.5, angle: 0.0, wavelength: 1.0 };
        let (effects, _) = convert(&[everywhere(rainbow), wasd(EffectConfig::Solid { color: RED })]);
        let kinds: Vec<EffectKind> = effects.iter().map(|effect| effect.kind).collect();
        assert_eq!(kinds, [EffectKind::RainbowWave, EffectKind::StaticColor]);
        assert_eq!(effects[0].leds.len(), 173 - 4);
        assert!(![4, 7, 22, 26].iter().any(|led| effects[0].leds.contains(led)));
    }

    #[test]
    fn a_layer_hidden_by_the_layers_above_is_skipped() {
        let under = layer("under", Zone::Named(NamedZone::Wasd), EffectConfig::Solid { color: BLUE });
        let (effects, report) = convert(&[under, wasd(EffectConfig::Solid { color: RED })]);
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].colors[0].rgb, RED);
        assert!(report.skipped[0].contains("\"under\"") && report.skipped[0].contains("hidden"), "{report:?}");
    }

    #[test]
    fn a_zone_without_writable_leds_is_skipped() {
        let (effects, report) = convert(&[layer("x", Zone::Leds(vec![0, 1, 2]), EffectConfig::Solid { color: RED })]);
        assert!(effects.is_empty());
        assert!(report.skipped[0].contains("\"x\"") && report.skipped[0].contains("no LEDs"), "{report:?}");
    }

    #[test]
    fn indicator_leds_are_never_written() {
        let (effects, _) = convert(&[layer("x", Zone::Leds(vec![0, 1, 2, 4]), EffectConfig::Solid { color: RED })]);
        assert_eq!(effects[0].leds, [4]);
    }

    #[test]
    fn static_custom_writes_one_file_per_colour_with_disjoint_leds() {
        let keys =
            vec![KeyColor { led: 4, color: RED }, KeyColor { led: 7, color: RED }, KeyColor { led: 22, color: BLUE }];
        let (effects, _) = convert(&[wasd(EffectConfig::StaticCustom { base: WHITE, keys })]);
        let files: Vec<(Rgb, Vec<u8>)> =
            effects.iter().map(|effect| (effect.colors[0].rgb, effect.leds.clone())).collect();
        assert_eq!(files, [(WHITE, vec![26]), (RED, vec![4, 7]), (BLUE, vec![22])]);
    }

    #[test]
    fn static_custom_keeps_the_last_colour_given_to_a_key_and_ignores_keys_outside_the_zone() {
        let keys =
            vec![KeyColor { led: 4, color: RED }, KeyColor { led: 4, color: BLUE }, KeyColor { led: 100, color: RED }];
        let (effects, _) = convert(&[wasd(EffectConfig::StaticCustom { base: WHITE, keys })]);
        let files: Vec<(Rgb, Vec<u8>)> =
            effects.iter().map(|effect| (effect.colors[0].rgb, effect.leds.clone())).collect();
        assert_eq!(files, [(WHITE, vec![7, 22, 26]), (BLUE, vec![4])]);
    }

    #[test]
    fn static_custom_below_a_layer_only_writes_what_is_left() {
        let keys = vec![KeyColor { led: 4, color: RED }, KeyColor { led: 7, color: BLUE }];
        let under = wasd(EffectConfig::StaticCustom { base: WHITE, keys });
        let over = layer("over", Zone::Leds(vec![4, 26]), EffectConfig::Solid { color: RED });
        let (effects, _) = convert(&[under, over]);
        let files: Vec<(Rgb, Vec<u8>)> =
            effects.iter().map(|effect| (effect.colors[0].rgb, effect.leds.clone())).collect();
        assert_eq!(files, [(WHITE, vec![22]), (BLUE, vec![7]), (RED, vec![4, 26])]);
    }

    #[test]
    fn effect_parameters_follow_the_mapping_table() {
        use Direction as D;
        use EffectDuration as T;
        use EffectKind as K;
        use Speed as S;
        let cases = [
            (color_wave(180.0), K::ColorWave, S::Medium, D::Left, T::None, vec![RED, BLUE]),
            (
                EffectConfig::RainbowWave { speed: 0.1, angle: 90.0, wavelength: 1.0 },
                K::RainbowWave,
                S::Slow,
                D::Down,
                T::None,
                vec![],
            ),
            (
                EffectConfig::RainbowWave { speed: 2.5, angle: 270.0, wavelength: 1.0 },
                K::RainbowWave,
                S::Fast,
                D::Up,
                T::None,
                vec![],
            ),
            (EffectConfig::SpiralRainbow { speed: 0.9 }, K::SpiralRainbow, S::Fast, D::Clockwise, T::None, vec![]),
            (
                EffectConfig::Visor { color: RED, background: BLUE, speed: 0.5, width: 0.2 },
                K::Visor,
                S::Medium,
                D::Right,
                T::None,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::ColorShift { colors: vec![RED, BLUE], speed: 0.2 },
                K::ColorShift,
                S::Slow,
                D::None,
                T::None,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::ColorPulse { colors: vec![RED, BLUE], speed: 0.5 },
                K::ColorPulse,
                S::Medium,
                D::None,
                T::None,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::Rain { color: RED, background: BLUE, speed: 0.5, columns: 24 },
                K::Rain,
                S::Medium,
                D::Down,
                T::None,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::Type { color: RED, background: BLUE, fade_ms: 300 },
                K::GradientType,
                S::Fast,
                D::None,
                T::Short,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::Type { color: RED, background: BLUE, fade_ms: 1000 },
                K::GradientType,
                S::Medium,
                D::None,
                T::Medium,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::Type { color: RED, background: BLUE, fade_ms: 3000 },
                K::GradientType,
                S::Slow,
                D::None,
                T::Long,
                vec![RED, BLUE],
            ),
            (
                EffectConfig::Ripple { color: RED, background: BLUE, speed: 1500.0, width: 120.0, lifetime_ms: 1500 },
                K::RippleType,
                S::Medium,
                D::None,
                T::None,
                vec![RED, BLUE],
            ),
        ];
        for (config, kind, speed, direction, duration, colors) in cases {
            let (effects, _) = convert(&[everywhere(config.clone())]);
            let effect = &effects[0];
            assert_eq!(
                (effect.kind, effect.speed, effect.direction, effect.duration),
                (kind, speed, direction, duration),
                "{config:?}"
            );
            assert_eq!(effect.colors.iter().map(|color| color.rgb).collect::<Vec<_>>(), colors, "{config:?}");
            assert_eq!(effect.flags, if colors.is_empty() { 0 } else { SPECIFIED_COLORS }, "{config:?}");
        }
    }

    #[test]
    fn speed_thresholds_split_at_035_and_075() {
        let speed_of = |speed: f32| convert(&[wasd(EffectConfig::SpiralRainbow { speed })]).0[0].speed;
        assert_eq!(speed_of(0.34), Speed::Slow);
        assert_eq!(speed_of(0.35), Speed::Medium);
        assert_eq!(speed_of(0.74), Speed::Medium);
        assert_eq!(speed_of(0.75), Speed::Fast);
    }

    #[test]
    fn ripple_speed_is_scaled_before_it_is_ranked() {
        let speed_of = |speed: f32| {
            let config = EffectConfig::Ripple { color: RED, background: BLUE, speed, width: 120.0, lifetime_ms: 1500 };
            convert(&[wasd(config)]).0[0].speed
        };
        assert_eq!(speed_of(600.0), Speed::Slow);
        assert_eq!(speed_of(1500.0), Speed::Medium);
        assert_eq!(speed_of(3000.0), Speed::Fast);
    }

    #[test]
    fn angles_round_to_the_nearest_quarter_turn_and_ties_go_to_the_lower_one() {
        let direction_of = |angle: f32| convert(&[wasd(color_wave(angle))]).0[0].direction;
        let cases = [
            (0.0, Direction::Right),
            (44.0, Direction::Right),
            (45.0, Direction::Right),
            (46.0, Direction::Down),
            (90.0, Direction::Down),
            (135.0, Direction::Down),
            (180.0, Direction::Left),
            (225.0, Direction::Left),
            (270.0, Direction::Up),
            (315.0, Direction::Up),
            (359.0, Direction::Right),
            (360.0, Direction::Right),
            (-90.0, Direction::Up),
        ];
        for (angle, direction) in cases {
            assert_eq!(direction_of(angle), direction, "angle {angle}");
        }
    }

    #[test]
    fn diagonal_angles_are_reported_and_right_angles_are_not() {
        let (_, report) = convert(&[wasd(color_wave(45.0))]);
        assert!(report.approximated[0].contains("angle"), "{report:?}");
        assert!(report.approximated[0].contains("\"w\""), "{report:?}");
        let (_, report) = convert(&[wasd(color_wave(270.0))]);
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn extra_colours_are_reported() {
        let (effects, report) =
            convert(&[wasd(EffectConfig::ColorShift { colors: vec![RED, BLUE, WHITE], speed: 0.5 })]);
        assert_eq!(effects[0].colors.len(), 2);
        assert!(report.lines().join("\n").contains("colour"), "{report:?}");
    }

    #[test]
    fn a_single_colour_is_written_once() {
        for config in [
            EffectConfig::ColorShift { colors: vec![RED], speed: 0.5 },
            EffectConfig::ColorPulse { colors: vec![RED], speed: 0.5 },
        ] {
            let (effects, report) = convert(&[wasd(config)]);
            assert_eq!(effects[0].colors.iter().map(|color| color.rgb).collect::<Vec<_>>(), [RED]);
            assert_eq!(effects[0].flags, SPECIFIED_COLORS);
            assert!(report.is_empty(), "{report:?}");
        }
    }

    #[test]
    fn a_colour_effect_without_colours_is_skipped() {
        let (effects, report) = convert(&[wasd(EffectConfig::ColorShift { colors: vec![], speed: 0.5 })]);
        assert!(effects.is_empty());
        assert!(report.skipped[0].contains("no colours"), "{report:?}");
    }

    #[test]
    fn parameters_without_a_hardware_setting_are_reported_when_they_differ_from_the_default() {
        let cases = [
            (EffectConfig::ColorWave { color: RED, background: BLUE, speed: 0.5, width: 0.5, angle: 0.0 }, "width"),
            (EffectConfig::Visor { color: RED, background: BLUE, speed: 0.5, width: 0.5 }, "width"),
            (EffectConfig::RainbowWave { speed: 0.5, angle: 0.0, wavelength: 2.0 }, "wavelength"),
            (EffectConfig::Rain { color: RED, background: BLUE, speed: 0.5, columns: 10 }, "columns"),
            (
                EffectConfig::Ripple { color: RED, background: BLUE, speed: 1500.0, width: 300.0, lifetime_ms: 1500 },
                "width",
            ),
            (
                EffectConfig::Ripple { color: RED, background: BLUE, speed: 1500.0, width: 120.0, lifetime_ms: 900 },
                "lifetime_ms",
            ),
        ];
        for (config, parameter) in cases {
            let (_, report) = convert(&[wasd(config.clone())]);
            assert_eq!(report.approximated.len(), 1, "{config:?}: {report:?}");
            let line = &report.approximated[0];
            assert!(line.contains(parameter) && line.contains("has no hardware setting"), "{line}");
        }
    }

    #[test]
    fn default_parameters_produce_no_report() {
        let defaults = [
            r#"type = "color_wave"
color = '#ff0000'"#,
            r#"type = "visor"
color = '#ff0000'"#,
            r#"type = "rainbow_wave""#,
            r#"type = "rain"
color = '#ff0000'"#,
            r#"type = "ripple"
color = '#ff0000'"#,
            r#"type = "type"
color = '#ff0000'"#,
        ];
        for text in defaults {
            let config: EffectConfig = toml::from_str(text).unwrap();
            let (effects, report) = convert(&[wasd(config)]);
            assert_eq!(effects.len(), 1, "{text}");
            assert!(report.is_empty(), "{text}: {report:?}");
        }
    }

    #[test]
    fn layers_without_a_hardware_layout_are_skipped() {
        let gradient = everywhere(EffectConfig::Gradient { from: RED, to: BLUE, angle: 0.0 });
        let audio = everywhere(toml::from_str("type = \"audio_visualizer\"").unwrap());
        let mut off = wasd(EffectConfig::Solid { color: RED });
        off.enabled = false;
        let (effects, report) = convert(&[gradient, audio, off]);
        assert!(effects.is_empty());
        assert_eq!(report.skipped.len(), 3, "{report:?}");
    }

    #[test]
    fn a_skipped_layer_does_not_hide_the_layers_below_it() {
        let below = wasd(EffectConfig::Solid { color: RED });
        let mut off = wasd(EffectConfig::Solid { color: BLUE });
        off.enabled = false;
        let (effects, _) = convert(&[below, off]);
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].leds, [4, 7, 22, 26]);
    }

    #[test]
    fn opacity_and_blend_modes_are_written_opaque_and_reported() {
        let mut half = wasd(EffectConfig::Solid { color: RED });
        half.opacity = 0.5;
        let (effects, report) = convert(&[half]);
        assert_eq!(effects[0].colors[0].alpha, 0xFF);
        assert!(report.approximated[0].contains("opaque"), "{report:?}");
        let mut added = wasd(EffectConfig::Solid { color: RED });
        added.blend = engine::BlendMode::Add;
        let (_, report) = convert(&[added]);
        assert!(report.approximated[0].contains("opaque"), "{report:?}");
    }

    #[test]
    fn only_the_top_twenty_files_are_kept() {
        let layers: Vec<LayerConfig> = (0..21u8)
            .map(|i| layer(&format!("layer{i}"), Zone::Leds(vec![4 + i]), EffectConfig::Solid { color: RED }))
            .collect();
        let (effects, report) = convert(&layers);
        assert_eq!(effects.len(), MAX_EFFECTS);
        assert_eq!(effects[0].leds, [5]);
        assert!(report.skipped[0].contains("\"layer0\"") && report.skipped[0].contains("20"), "{report:?}");
    }

    #[test]
    fn a_static_custom_layer_is_cut_at_the_limit() {
        let fillers: Vec<LayerConfig> = (0..18u8)
            .map(|i| layer(&format!("f{i}"), Zone::Leds(vec![30 + i]), EffectConfig::Solid { color: RED }))
            .collect();
        let keys = vec![KeyColor { led: 4, color: RED }, KeyColor { led: 7, color: BLUE }];
        let custom = wasd(EffectConfig::StaticCustom { base: WHITE, keys });
        let mut layers = vec![custom];
        layers.extend(fillers);
        let (effects, report) = convert(&layers);
        assert_eq!(effects.len(), MAX_EFFECTS);
        assert!(report.skipped[0].contains("\"w\"") && report.skipped[0].contains("20"), "{report:?}");
    }

    #[test]
    fn defaults_match_the_engine() {
        let parse = |text: &str| toml::from_str::<EffectConfig>(text).unwrap();
        let EffectConfig::ColorWave { width, .. } = parse("type = \"color_wave\"\ncolor = '#010203'") else {
            unreachable!()
        };
        let EffectConfig::Visor { width: visor_width, .. } = parse("type = \"visor\"\ncolor = '#010203'") else {
            unreachable!()
        };
        let EffectConfig::RainbowWave { wavelength, .. } = parse("type = \"rainbow_wave\"") else { unreachable!() };
        let EffectConfig::Rain { columns, .. } = parse("type = \"rain\"\ncolor = '#010203'") else { unreachable!() };
        let EffectConfig::Ripple { width: ripple_width, lifetime_ms, .. } =
            parse("type = \"ripple\"\ncolor = '#010203'")
        else {
            unreachable!()
        };
        assert_eq!(
            (width, visor_width, wavelength, columns, ripple_width, lifetime_ms),
            (
                DEFAULT_WIDTH,
                DEFAULT_WIDTH,
                DEFAULT_WAVELENGTH,
                DEFAULT_COLUMNS,
                DEFAULT_RIPPLE_WIDTH,
                DEFAULT_LIFETIME_MS
            )
        );
    }
}

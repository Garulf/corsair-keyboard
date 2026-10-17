use engine::{EffectConfig, LayerConfig, NamedZone, Zone};
use ipc::{Client, Profile, Reply, Request};
use k100::Rgb;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const LAYER_NAME: &str = "cli-effect";
const EFFECTS: &str =
    "solid, rainbow_wave, spiral_rainbow, color_wave, visor, color_pulse, rain, type, ripple, audio_visualizer, off";

pub fn parse_color(text: &str) -> std::result::Result<Rgb, String> {
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{text:?} is not a colour; use #rrggbb"));
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).map_err(|failure| failure.to_string());
    Ok(Rgb::new(channel(0)?, channel(2)?, channel(4)?))
}

pub fn build(name: &str, color: Option<Rgb>) -> std::result::Result<EffectConfig, String> {
    let need = || color.ok_or_else(|| format!("{name} needs --color #rrggbb"));
    let black = Rgb::BLACK;
    Ok(match name {
        "solid" => EffectConfig::Solid { color: need()? },
        "rainbow_wave" => EffectConfig::RainbowWave { speed: 0.5, angle: 0.0, wavelength: 1.0 },
        "spiral_rainbow" => EffectConfig::SpiralRainbow { speed: 0.5 },
        "color_wave" => {
            EffectConfig::ColorWave { color: need()?, background: black, speed: 0.5, width: 0.2, angle: 0.0 }
        }
        "visor" => EffectConfig::Visor { color: need()?, background: black, speed: 0.5, width: 0.2 },
        "color_pulse" => EffectConfig::ColorPulse { colors: vec![black, need()?], speed: 0.5 },
        "rain" => EffectConfig::Rain { color: need()?, background: black, speed: 0.5, columns: 24 },
        "type" => EffectConfig::Type { color: need()?, background: black, fade_ms: 1000 },
        "ripple" => {
            EffectConfig::Ripple { color: need()?, background: black, speed: 1500.0, width: 120.0, lifetime_ms: 1500 }
        }
        "audio_visualizer" => audio_visualizer(color),
        other => return Err(format!("unknown effect {other:?} (choose one of: {EFFECTS})")),
    })
}

fn audio_visualizer(color: Option<Rgb>) -> EffectConfig {
    EffectConfig::AudioVisualizer {
        low: color.unwrap_or(Rgb::new(0x00, 0xFF, 0x40)),
        high: color.unwrap_or(Rgb::new(0xFF, 0x00, 0x00)),
        background: Rgb::BLACK,
        gain: 1.0,
        decay_ms: 400,
    }
}

pub fn apply(profile: &mut Profile, effect: EffectConfig) {
    profile.layers.retain(|layer| layer.name != LAYER_NAME);
    profile.layers.push(LayerConfig::new(LAYER_NAME, Zone::Named(NamedZone::All), effect));
}

pub fn clear(profile: &mut Profile) -> bool {
    let before = profile.layers.len();
    profile.layers.retain(|layer| layer.name != LAYER_NAME);
    profile.layers.len() != before
}

pub fn run(name: &str, color: Option<&str>) -> Result<()> {
    let color = color.map(parse_color).transpose()?;
    let effect = if name == "off" { None } else { Some(build(name, color)?) };
    let mut client = Client::connect()?;
    let Reply::State { state } = client.request(&Request::GetState)? else {
        return Err("unexpected reply to get_state".into());
    };
    let Reply::Profile { mut profile } = client.request(&Request::GetProfile { name: state.active_profile })? else {
        return Err("unexpected reply to get_profile".into());
    };
    match effect {
        Some(effect) => apply(&mut profile, effect),
        None => {
            clear(&mut profile);
        }
    }
    Ok(crate::remote::expect_done(client.request(&Request::SaveProfile { profile })?)?)
}

#[cfg(test)]
mod tests {
    use engine::NamedZone;

    use super::*;

    fn profile() -> Profile {
        Profile {
            version: 1,
            name: "p".into(),
            apps: vec![],
            layers: vec![LayerConfig::new(
                "base",
                Zone::Named(NamedZone::All),
                EffectConfig::Solid { color: Rgb::BLACK },
            )],
            assignments: vec![],
            dial: actions::DialConfig::default(),
        }
    }

    #[test]
    fn colours_parse_with_or_without_a_hash() {
        assert_eq!(parse_color("#ff8000").unwrap(), Rgb::new(255, 128, 0));
        assert_eq!(parse_color("00ff00").unwrap(), Rgb::new(0, 255, 0));
        for bad in ["", "#fff", "#gggggg", "+f0000", "#ff80000"] {
            assert!(parse_color(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn effects_need_a_colour_only_when_they_use_one() {
        assert!(matches!(build("rainbow_wave", None).unwrap(), EffectConfig::RainbowWave { .. }));
        assert!(build("solid", None).unwrap_err().contains("--color"));
        let red = Some(Rgb::new(255, 0, 0));
        assert_eq!(build("solid", red).unwrap(), EffectConfig::Solid { color: Rgb::new(255, 0, 0) });
        let unknown = build("sparkle", red).unwrap_err();
        assert!(unknown.contains("sparkle") && unknown.contains("ripple"), "{unknown}");
    }

    #[test]
    fn the_audio_visualizer_builds_with_defaults_and_a_colour_overrides_both_ends() {
        let defaults = build("audio_visualizer", None).unwrap();
        assert_eq!(
            defaults,
            EffectConfig::AudioVisualizer {
                low: Rgb::new(0x00, 0xFF, 0x40),
                high: Rgb::new(0xFF, 0x00, 0x00),
                background: Rgb::BLACK,
                gain: 1.0,
                decay_ms: 400,
            }
        );
        let pink = Rgb::new(255, 0, 128);
        let EffectConfig::AudioVisualizer { low, high, decay_ms, .. } = build("audio_visualizer", Some(pink)).unwrap()
        else {
            panic!("not the audio visualizer");
        };
        assert_eq!((low, high, decay_ms), (pink, pink, 400));
    }

    #[test]
    fn applying_twice_keeps_one_layer_on_top_and_clearing_removes_it() {
        let mut profile = profile();
        apply(&mut profile, EffectConfig::Solid { color: Rgb::new(1, 2, 3) });
        apply(&mut profile, EffectConfig::Solid { color: Rgb::new(4, 5, 6) });
        let names: Vec<&str> = profile.layers.iter().map(|layer| layer.name.as_str()).collect();
        assert_eq!(names, ["base", LAYER_NAME]);
        assert_eq!(profile.layers[1].effect, EffectConfig::Solid { color: Rgb::new(4, 5, 6) });
        assert!(clear(&mut profile));
        assert!(!clear(&mut profile));
        assert_eq!(profile.layers.len(), 1);
    }
}

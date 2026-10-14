use std::time::Duration;

use k100::{Frame, LED_COUNT, Rgb};
use serde::{Deserialize, Serialize};

use crate::color::{lerp, unit};
use crate::effects::EffectConfig;
use crate::layout::Layout;
use crate::zone::{Mask, Zone};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Multiply,
}

impl BlendMode {
    pub fn apply(self, base: Rgb, top: Rgb, opacity: f32) -> Rgb {
        let opacity = unit(opacity);
        match self {
            BlendMode::Normal => lerp(base, top, opacity),
            BlendMode::Add => {
                let add = |b: u8, t: u8| (f32::from(b) + f32::from(t) * opacity).min(255.0).round() as u8;
                Rgb::new(add(base.r, top.r), add(base.g, top.g), add(base.b, top.b))
            }
            BlendMode::Multiply => {
                let mul =
                    |b: u8, t: u8| (f32::from(b) * (1.0 - opacity + opacity * f32::from(t) / 255.0)).round() as u8;
                Rgb::new(mul(base.r, top.r), mul(base.g, top.g), mul(base.b, top.b))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub led: u8,
    pub at: Duration,
}

pub struct FrameCtx<'a> {
    pub time: Duration,
    pub layout: &'a Layout,
    pub presses: &'a [KeyPress],
}

pub trait Effect: Send {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame);
}

fn full_opacity() -> f32 {
    1.0
}

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerConfig {
    pub name: String,
    pub zone: Zone,
    pub effect: EffectConfig,
    #[serde(default = "full_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

impl LayerConfig {
    pub fn new(name: &str, zone: Zone, effect: EffectConfig) -> Self {
        Self { name: name.to_owned(), zone, effect, opacity: 1.0, blend: BlendMode::Normal, enabled: true }
    }
}

struct Layer {
    effect: Box<dyn Effect>,
    mask: Mask,
    opacity: f32,
    blend: BlendMode,
}

pub struct Compositor {
    layout: Layout,
    layers: Vec<Layer>,
}

impl Compositor {
    pub fn new(layout: Layout, configs: &[LayerConfig]) -> Self {
        let layers = configs
            .iter()
            .filter(|config| config.enabled)
            .map(|config| Layer {
                effect: config.effect.build(),
                mask: config.zone.mask(&layout),
                opacity: config.opacity,
                blend: config.blend,
            })
            .collect();
        Self { layout, layers }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn render(&mut self, time: Duration, presses: &[KeyPress]) -> Frame {
        let ctx = FrameCtx { time, layout: &self.layout, presses };
        let mut out = [Rgb::BLACK; LED_COUNT];
        let mut scratch = [Rgb::BLACK; LED_COUNT];
        for layer in &mut self.layers {
            scratch.fill(Rgb::BLACK);
            layer.effect.render(&ctx, &mut scratch);
            for index in 0..LED_COUNT {
                if layer.mask[index] {
                    out[index] = layer.blend.apply(out[index], scratch[index], layer.opacity);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EffectConfig;
    use crate::zone::{NamedZone, Zone};

    fn solid(zone: Zone, color: Rgb) -> LayerConfig {
        LayerConfig::new("t", zone, EffectConfig::Solid { color })
    }

    #[test]
    fn upper_layers_cover_lower_layers_only_inside_their_zone() {
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        let layers = [solid(Zone::Named(NamedZone::All), red), solid(Zone::Named(NamedZone::Wasd), blue)];
        let mut compositor = Compositor::new(Layout::ansi(), &layers);
        let frame = compositor.render(Duration::ZERO, &[]);
        assert_eq!(frame[26], blue, "W");
        assert_eq!(frame[20], red, "Q");
        assert_eq!(frame[100], Rgb::BLACK, "not on ANSI");
    }

    #[test]
    fn opacity_and_blend_modes() {
        let base = Rgb::new(100, 100, 100);
        let top = Rgb::new(200, 0, 255);
        assert_eq!(BlendMode::Normal.apply(base, top, 0.5), Rgb::new(150, 50, 178));
        assert_eq!(BlendMode::Add.apply(base, top, 1.0), Rgb::new(255, 100, 255));
        assert_eq!(BlendMode::Multiply.apply(base, top, 1.0), Rgb::new(78, 0, 100));
        assert_eq!(BlendMode::Normal.apply(base, top, 7.0), top, "opacity clamps high");
        assert_eq!(BlendMode::Normal.apply(base, top, -3.0), base, "opacity clamps low");
    }

    #[test]
    fn disabled_layers_are_skipped() {
        let mut layer = solid(Zone::Named(NamedZone::All), Rgb::new(9, 9, 9));
        layer.enabled = false;
        let frame = Compositor::new(Layout::ansi(), &[layer]).render(Duration::ZERO, &[]);
        assert!(frame.iter().all(|&c| c == Rgb::BLACK));
    }

    #[test]
    fn parses_layers_from_toml() {
        #[derive(serde::Deserialize)]
        struct Profile {
            layers: Vec<LayerConfig>,
        }
        let profile: Profile = toml::from_str(
            r##"
            [[layers]]
            name = "base"
            zone = "all"
            effect = { type = "solid", color = "#202020" }

            [[layers]]
            name = "custom"
            zone = [131, 132, 250]
            opacity = 0.5
            blend = "add"
            effect = { type = "solid", color = "#ff0000" }
            "##,
        )
        .unwrap();
        assert_eq!(profile.layers.len(), 2);
        assert_eq!(profile.layers[1].blend, BlendMode::Add);
        let frame = Compositor::new(Layout::ansi(), &profile.layers).render(Duration::ZERO, &[]);
        assert_eq!(frame[131], Rgb::new(160, 32, 32));
        assert_eq!(frame[4], Rgb::new(32, 32, 32));
    }
}

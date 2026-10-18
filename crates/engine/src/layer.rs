use std::time::Duration;

use k100::{Frame, LED_COUNT, Rgb};
use serde::{Deserialize, Serialize};

use crate::clients::{ClientLayer, ClientLighting, PROFILE_PRIORITY};
use crate::color::{lerp, unit};
use crate::effects::EffectConfig;
use crate::layout::Layout;
use crate::spectrum::Spectrum;
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
    pub spectrum: &'a Spectrum,
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
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Clone, Copy, PartialEq)]
enum Forced {
    No,
    WhileHeld,
    Until(Duration),
}

struct Layer {
    name: String,
    enabled: bool,
    forced: Forced,
    effect: Box<dyn Effect>,
    mask: Mask,
    opacity: f32,
    blend: BlendMode,
}

impl Layer {
    fn visible(&mut self, time: Duration) -> bool {
        if let Forced::Until(end) = self.forced
            && time >= end
        {
            self.forced = Forced::No;
        }
        self.enabled || self.forced != Forced::No
    }
}

pub struct Compositor {
    layout: Layout,
    layers: Vec<Layer>,
}

impl Compositor {
    pub fn new(layout: Layout, configs: &[LayerConfig]) -> Self {
        let layers = configs
            .iter()
            .map(|config| Layer {
                name: config.name.clone(),
                enabled: config.enabled,
                forced: Forced::No,
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

    pub fn trigger_layer(&mut self, name: &str, until: Option<Duration>) -> bool {
        let mut found = false;
        for layer in self.layers.iter_mut().filter(|layer| layer.name == name) {
            layer.forced = until.map_or(Forced::WhileHeld, Forced::Until);
            found = true;
        }
        found
    }

    pub fn release_layer(&mut self, name: &str) {
        for layer in self.layers.iter_mut().filter(|layer| layer.name == name && layer.forced == Forced::WhileHeld) {
            layer.forced = Forced::No;
        }
    }

    pub fn render(&mut self, time: Duration, presses: &[KeyPress], spectrum: &Spectrum) -> Frame {
        self.render_with(time, presses, spectrum, &ClientLighting::NONE)
    }

    pub fn render_with(
        &mut self,
        time: Duration,
        presses: &[KeyPress],
        spectrum: &Spectrum,
        clients: &ClientLighting,
    ) -> Frame {
        let mut out = [Rgb::BLACK; LED_COUNT];
        match clients {
            ClientLighting::Exclusive(layer) => paint_client(&mut out, layer),
            ClientLighting::Shared(layers) => {
                let (below, above): (Vec<_>, Vec<_>) =
                    layers.iter().partition(|layer| layer.priority <= PROFILE_PRIORITY);
                below.into_iter().for_each(|layer| paint_client(&mut out, layer));
                self.paint_profile(&mut out, time, presses, spectrum);
                above.into_iter().for_each(|layer| paint_client(&mut out, layer));
            }
        }
        out
    }

    fn paint_profile(&mut self, out: &mut Frame, time: Duration, presses: &[KeyPress], spectrum: &Spectrum) {
        let ctx = FrameCtx { time, layout: &self.layout, presses, spectrum };
        let mut scratch = [Rgb::BLACK; LED_COUNT];
        for layer in &mut self.layers {
            if !layer.visible(time) {
                continue;
            }
            scratch.fill(Rgb::BLACK);
            layer.effect.render(&ctx, &mut scratch);
            for index in 0..LED_COUNT {
                if layer.mask[index] {
                    out[index] = layer.blend.apply(out[index], scratch[index], layer.opacity);
                }
            }
        }
    }
}

fn paint_client(out: &mut Frame, layer: &ClientLayer) {
    for &(led, color, alpha) in &layer.colors {
        if let Some(below) = out.get_mut(usize::from(led)) {
            *below = lerp(*below, color, f32::from(alpha) / 255.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::PROFILE_PRIORITY;
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
        let frame = compositor.render(Duration::ZERO, &[], &Spectrum::SILENT);
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
        let frame = Compositor::new(Layout::ansi(), &[layer]).render(Duration::ZERO, &[], &Spectrum::SILENT);
        assert!(frame.iter().all(|&c| c == Rgb::BLACK));
    }

    #[test]
    fn triggered_layers_show_while_held_or_until_their_time() {
        let red = Rgb::new(255, 0, 0);
        let blue = Rgb::new(0, 0, 255);
        let mut flash = LayerConfig::new("flash", Zone::Named(NamedZone::All), EffectConfig::Solid { color: blue });
        flash.enabled = false;
        let mut compositor = Compositor::new(Layout::ansi(), &[solid(Zone::Named(NamedZone::All), red), flash]);
        let at = Duration::from_millis;
        assert_eq!(compositor.render(at(0), &[], &Spectrum::SILENT)[4], red);
        assert!(compositor.trigger_layer("flash", None));
        assert_eq!(compositor.render(at(10), &[], &Spectrum::SILENT)[4], blue);
        compositor.release_layer("flash");
        assert_eq!(compositor.render(at(20), &[], &Spectrum::SILENT)[4], red);
        compositor.trigger_layer("flash", Some(at(100)));
        assert_eq!(compositor.render(at(50), &[], &Spectrum::SILENT)[4], blue);
        assert_eq!(compositor.render(at(100), &[], &Spectrum::SILENT)[4], red);
        assert_eq!(compositor.render(at(110), &[], &Spectrum::SILENT)[4], red);
        assert!(!compositor.trigger_layer("nope", None));
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
        let frame = Compositor::new(Layout::ansi(), &profile.layers).render(Duration::ZERO, &[], &Spectrum::SILENT);
        assert_eq!(frame[131], Rgb::new(160, 32, 32));
        assert_eq!(frame[4], Rgb::new(32, 32, 32));
    }

    #[test]
    fn misspelled_fields_are_rejected() {
        #[derive(serde::Deserialize, Debug)]
        struct Profile {
            #[allow(dead_code)]
            layers: Vec<LayerConfig>,
        }
        let layer_typo = "[[layers]]\nname = \"a\"\nzone = \"all\"\nopactiy = 0.2\neffect = { type = \"solid\", color = \"#ffffff\" }\n";
        assert!(toml::from_str::<Profile>(layer_typo).unwrap_err().to_string().contains("opactiy"));
        let effect_typo =
            "[[layers]]\nname = \"a\"\nzone = \"all\"\neffect = { type = \"rainbow_wave\", speeed = 5.0 }\n";
        assert!(toml::from_str::<Profile>(effect_typo).unwrap_err().to_string().contains("speeed"));
        let key_typo = "[[layers]]\nname = \"a\"\nzone = \"all\"\neffect = { type = \"static_custom\", keys = [{ led = 4, colour = \"#ffffff\" }] }\n";
        assert!(toml::from_str::<Profile>(key_typo).unwrap_err().to_string().contains("colour"));
    }

    fn client(priority: u8, colors: &[(u8, Rgb, u8)]) -> ClientLayer {
        ClientLayer { priority, colors: colors.to_vec() }
    }

    #[test]
    fn clients_land_below_or_above_the_profile_by_priority() {
        let (red, blue, green) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255), Rgb::new(0, 255, 0));
        let mut compositor = Compositor::new(Layout::ansi(), &[solid(Zone::Named(NamedZone::Wasd), red)]);
        let clients = ClientLighting::Shared(vec![
            client(100, &[(26, blue, 255), (20, blue, 255)]),
            client(128, &[(4, green, 255)]),
        ]);
        let frame = compositor.render_with(Duration::ZERO, &[], &Spectrum::SILENT, &clients);
        assert_eq!(frame[26], red, "W: the profile covers a client below it");
        assert_eq!(frame[20], blue, "Q: outside the profile's zone the low client shows");
        assert_eq!(frame[4], green, "A: a default-priority client covers the profile");
    }

    #[test]
    fn a_client_at_the_profile_priority_lands_below_it() {
        let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        let mut compositor = Compositor::new(Layout::ansi(), &[solid(Zone::Named(NamedZone::All), red)]);
        let clients = ClientLighting::Shared(vec![client(PROFILE_PRIORITY, &[(26, blue, 255)])]);
        assert_eq!(compositor.render_with(Duration::ZERO, &[], &Spectrum::SILENT, &clients)[26], red);
    }

    #[test]
    fn alpha_blends_a_client_color_over_what_is_below() {
        let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        let mut compositor = Compositor::new(Layout::ansi(), &[solid(Zone::Named(NamedZone::All), red)]);
        let clients = ClientLighting::Shared(vec![client(128, &[(26, blue, 128)]), client(200, &[(20, blue, 0)])]);
        let frame = compositor.render_with(Duration::ZERO, &[], &Spectrum::SILENT, &clients);
        assert_eq!(frame[26], crate::color::lerp(red, blue, 128.0 / 255.0));
        assert_eq!(frame[20], red, "alpha 0 is fully transparent");
    }

    #[test]
    fn exclusive_lighting_replaces_the_profile() {
        let mut compositor =
            Compositor::new(Layout::ansi(), &[solid(Zone::Named(NamedZone::All), Rgb::new(255, 0, 0))]);
        let clients = ClientLighting::Exclusive(client(128, &[(26, Rgb::new(0, 0, 255), 255)]));
        let frame = compositor.render_with(Duration::ZERO, &[], &Spectrum::SILENT, &clients);
        assert_eq!((frame[26], frame[20]), (Rgb::new(0, 0, 255), Rgb::BLACK));
    }

    #[test]
    fn higher_priority_clients_cover_lower_ones_and_no_clients_render_as_before() {
        let mut compositor = Compositor::new(Layout::ansi(), &[solid(Zone::Named(NamedZone::All), Rgb::new(9, 9, 9))]);
        let clients = ClientLighting::Shared(vec![
            client(150, &[(26, Rgb::new(0, 0, 255), 255)]),
            client(200, &[(26, Rgb::new(0, 255, 0), 255)]),
        ]);
        assert_eq!(compositor.render_with(Duration::ZERO, &[], &Spectrum::SILENT, &clients)[26], Rgb::new(0, 255, 0));
        let plain = compositor.render(Duration::ZERO, &[], &Spectrum::SILENT);
        assert_eq!(compositor.render_with(Duration::ZERO, &[], &Spectrum::SILENT, &ClientLighting::NONE), plain);
    }
}

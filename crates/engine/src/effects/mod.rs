mod reactive;
mod spatial;
mod temporal;

use std::time::Duration;

use k100::{LED_COUNT, Rgb};
use serde::{Deserialize, Serialize};

use crate::layer::Effect;

fn black() -> Rgb {
    Rgb::BLACK
}

fn default_speed() -> f32 {
    0.5
}

fn default_width() -> f32 {
    0.2
}

fn default_wavelength() -> f32 {
    1.0
}

fn default_columns() -> u8 {
    24
}

fn default_fade_ms() -> u64 {
    1000
}

fn default_ripple_speed() -> f32 {
    1500.0
}

fn default_ripple_width() -> f32 {
    120.0
}

fn default_lifetime_ms() -> u64 {
    1500
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyColor {
    pub led: u8,
    pub color: Rgb,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectConfig {
    Solid {
        color: Rgb,
    },
    StaticCustom {
        #[serde(default = "black")]
        base: Rgb,
        keys: Vec<KeyColor>,
    },
    Gradient {
        from: Rgb,
        to: Rgb,
        #[serde(default)]
        angle: f32,
    },
    RainbowWave {
        #[serde(default = "default_speed")]
        speed: f32,
        #[serde(default)]
        angle: f32,
        #[serde(default = "default_wavelength")]
        wavelength: f32,
    },
    ColorWave {
        color: Rgb,
        #[serde(default = "black")]
        background: Rgb,
        #[serde(default = "default_speed")]
        speed: f32,
        #[serde(default = "default_width")]
        width: f32,
        #[serde(default)]
        angle: f32,
    },
    SpiralRainbow {
        #[serde(default = "default_speed")]
        speed: f32,
    },
    Visor {
        color: Rgb,
        #[serde(default = "black")]
        background: Rgb,
        #[serde(default = "default_speed")]
        speed: f32,
        #[serde(default = "default_width")]
        width: f32,
    },
    ColorShift {
        colors: Vec<Rgb>,
        #[serde(default = "default_speed")]
        speed: f32,
    },
    ColorPulse {
        colors: Vec<Rgb>,
        #[serde(default = "default_speed")]
        speed: f32,
    },
    Rain {
        color: Rgb,
        #[serde(default = "black")]
        background: Rgb,
        #[serde(default = "default_speed")]
        speed: f32,
        #[serde(default = "default_columns")]
        columns: u8,
    },
    Type {
        color: Rgb,
        #[serde(default = "black")]
        background: Rgb,
        #[serde(default = "default_fade_ms")]
        fade_ms: u64,
    },
    Ripple {
        color: Rgb,
        #[serde(default = "black")]
        background: Rgb,
        #[serde(default = "default_ripple_speed")]
        speed: f32,
        #[serde(default = "default_ripple_width")]
        width: f32,
        #[serde(default = "default_lifetime_ms")]
        lifetime_ms: u64,
    },
}

impl EffectConfig {
    pub fn build(&self) -> Box<dyn Effect> {
        match self.clone() {
            EffectConfig::Solid { color } => Box::new(spatial::Solid { color }),
            EffectConfig::StaticCustom { base, keys } => Box::new(spatial::StaticCustom { base, keys }),
            EffectConfig::Gradient { from, to, angle } => Box::new(spatial::Gradient { from, to, angle }),
            EffectConfig::RainbowWave { speed, angle, wavelength } => {
                Box::new(spatial::RainbowWave { speed, angle, wavelength })
            }
            EffectConfig::ColorWave { color, background, speed, width, angle } => {
                Box::new(spatial::ColorWave { color, background, speed, width, angle })
            }
            EffectConfig::SpiralRainbow { speed } => Box::new(spatial::SpiralRainbow { speed }),
            EffectConfig::Visor { color, background, speed, width } => {
                Box::new(spatial::Visor { color, background, speed, width })
            }
            EffectConfig::ColorShift { colors, speed } => Box::new(temporal::ColorShift { colors, speed }),
            EffectConfig::ColorPulse { colors, speed } => Box::new(temporal::ColorPulse { colors, speed }),
            EffectConfig::Rain { color, background, speed, columns } => {
                Box::new(temporal::Rain { color, background, speed, columns })
            }
            EffectConfig::Type { color, background, fade_ms } => Box::new(reactive::Type {
                color,
                background,
                fade: Duration::from_millis(fade_ms),
                last_press: [None; LED_COUNT],
            }),
            EffectConfig::Ripple { color, background, speed, width, lifetime_ms } => Box::new(reactive::Ripple {
                color,
                background,
                speed,
                width,
                lifetime: Duration::from_millis(lifetime_ms),
                active: Vec::new(),
            }),
        }
    }
}

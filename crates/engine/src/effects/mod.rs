mod spatial;
mod temporal;

use k100::Rgb;
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct KeyColor {
    pub led: u8,
    pub color: Rgb,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
        }
    }
}

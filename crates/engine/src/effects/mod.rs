mod spatial;

use k100::Rgb;
use serde::{Deserialize, Serialize};

use crate::layer::Effect;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectConfig {
    Solid { color: Rgb },
}

impl EffectConfig {
    pub fn build(&self) -> Box<dyn Effect> {
        match self {
            EffectConfig::Solid { color } => Box::new(spatial::Solid { color: *color }),
        }
    }
}

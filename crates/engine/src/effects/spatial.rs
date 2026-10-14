use k100::{Frame, Rgb};

use crate::color::{hsv, lerp};
use crate::effects::KeyColor;
use crate::layer::{Effect, FrameCtx};
use crate::layout::Layout;

pub struct Solid {
    pub color: Rgb,
}

impl Effect for Solid {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        for geometry in ctx.layout.leds() {
            out[geometry.index as usize] = self.color;
        }
    }
}

fn phase(ctx: &FrameCtx, speed: f32) -> f32 {
    (ctx.time.as_secs_f64() * f64::from(speed)).rem_euclid(1.0) as f32
}

fn projection(layout: &Layout, index: u8, angle_degrees: f32) -> f32 {
    let Some((x, y)) = layout.normalized(index) else {
        return 0.0;
    };
    let (dy, dx) = angle_degrees.to_radians().sin_cos();
    let half_extent = 0.5 * (dx.abs() + dy.abs());
    let along = (x - 0.5) * dx + (y - 0.5) * dy;
    (along / half_extent + 1.0) / 2.0
}

fn band(position: f32, center: f32, width: f32) -> f32 {
    let half = (width / 2.0).max(f32::EPSILON);
    1.0 - ((position - center).abs() / half).min(1.0)
}

pub struct StaticCustom {
    pub base: Rgb,
    pub keys: Vec<KeyColor>,
}

impl Effect for StaticCustom {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        Solid { color: self.base }.render(ctx, out);
        for key in &self.keys {
            if ctx.layout.get(key.led).is_some() {
                out[key.led as usize] = key.color;
            }
        }
    }
}

pub struct Gradient {
    pub from: Rgb,
    pub to: Rgb,
    pub angle: f32,
}

impl Effect for Gradient {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        for geometry in ctx.layout.leds() {
            let t = projection(ctx.layout, geometry.index, self.angle);
            out[geometry.index as usize] = lerp(self.from, self.to, t);
        }
    }
}

pub struct RainbowWave {
    pub speed: f32,
    pub angle: f32,
    pub wavelength: f32,
}

impl Effect for RainbowWave {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let shift = phase(ctx, self.speed);
        let wavelength = self.wavelength.max(0.01);
        for geometry in ctx.layout.leds() {
            let position = projection(ctx.layout, geometry.index, self.angle) / wavelength;
            out[geometry.index as usize] = hsv(360.0 * (position - shift), 1.0, 1.0);
        }
    }
}

pub struct ColorWave {
    pub color: Rgb,
    pub background: Rgb,
    pub speed: f32,
    pub width: f32,
    pub angle: f32,
}

impl Effect for ColorWave {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let center = phase(ctx, self.speed) * (1.0 + self.width) - self.width / 2.0;
        for geometry in ctx.layout.leds() {
            let position = projection(ctx.layout, geometry.index, self.angle);
            out[geometry.index as usize] = lerp(self.background, self.color, band(position, center, self.width));
        }
    }
}

pub struct SpiralRainbow {
    pub speed: f32,
}

impl Effect for SpiralRainbow {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let shift = phase(ctx, self.speed) * 360.0;
        for geometry in ctx.layout.leds() {
            let (x, y) = ctx.layout.normalized(geometry.index).unwrap_or((0.5, 0.5));
            let angle = (y - 0.5).atan2(x - 0.5).to_degrees();
            out[geometry.index as usize] = hsv(angle + shift, 1.0, 1.0);
        }
    }
}

pub struct Visor {
    pub color: Rgb,
    pub background: Rgb,
    pub speed: f32,
    pub width: f32,
}

impl Effect for Visor {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let center = 1.0 - (2.0 * phase(ctx, self.speed) - 1.0).abs();
        for geometry in ctx.layout.leds() {
            let position = projection(ctx.layout, geometry.index, 0.0);
            out[geometry.index as usize] = lerp(self.background, self.color, band(position, center, self.width));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::effects::{EffectConfig, KeyColor};
    use crate::layout::Layout;
    use k100::{LED_COUNT, Rgb};

    fn render(config: EffectConfig, seconds: f32) -> Frame {
        let layout = Layout::ansi();
        let mut effect = config.build();
        let mut out = [Rgb::BLACK; LED_COUNT];
        let ctx = FrameCtx { time: Duration::from_secs_f32(seconds), layout: &layout, presses: &[] };
        effect.render(&ctx, &mut out);
        out
    }

    const RED: Rgb = Rgb::new(255, 0, 0);
    const BLUE: Rgb = Rgb::new(0, 0, 255);
    const LEFT_KEY: usize = 131;
    const RIGHT_KEY: usize = 99;

    #[test]
    fn static_custom_overrides_listed_keys() {
        let keys = vec![KeyColor { led: 4, color: RED }, KeyColor { led: 250, color: RED }];
        let frame = render(EffectConfig::StaticCustom { base: BLUE, keys }, 0.0);
        assert_eq!(frame[4], RED);
        assert_eq!(frame[22], BLUE);
    }

    #[test]
    fn horizontal_gradient_runs_left_to_right() {
        let frame = render(EffectConfig::Gradient { from: RED, to: BLUE, angle: 0.0 }, 0.0);
        assert!(frame[LEFT_KEY].r > frame[LEFT_KEY].b);
        assert!(frame[RIGHT_KEY].b > frame[RIGHT_KEY].r);
        let vertical = render(EffectConfig::Gradient { from: RED, to: BLUE, angle: 90.0 }, 0.0);
        assert!(vertical[41].r > vertical[44].r, "Esc row redder than space row");
    }

    #[test]
    fn rainbow_wave_moves_with_time() {
        let config = EffectConfig::RainbowWave { speed: 0.5, angle: 0.0, wavelength: 1.0 };
        assert_ne!(render(config.clone(), 0.0)[4], render(config.clone(), 0.3)[4]);
        assert_eq!(render(config.clone(), 0.0)[4], render(config, 2.0)[4], "period is 1/speed");
    }

    #[test]
    fn color_wave_band_passes_over_keys() {
        let config = EffectConfig::ColorWave { color: RED, background: BLUE, speed: 0.5, width: 0.2, angle: 0.0 };
        let lit_early = render(config.clone(), 0.2)[LEFT_KEY];
        let lit_late = render(config, 1.6)[LEFT_KEY];
        assert!(lit_early.r > lit_late.r, "band near the left early, gone later");
    }

    #[test]
    fn spiral_differs_around_the_center() {
        let frame = render(EffectConfig::SpiralRainbow { speed: 0.5 }, 0.0);
        assert_ne!(frame[LEFT_KEY], frame[RIGHT_KEY]);
    }

    #[test]
    fn visor_bounces() {
        let config = EffectConfig::Visor { color: RED, background: Rgb::BLACK, speed: 0.5, width: 0.2 };
        let left_edge = k100::led::LEFT_UNDERGLOW_FIRST as usize;
        let right_edge = k100::led::RIGHT_UNDERGLOW_FIRST as usize;
        assert!(render(config.clone(), 0.0)[left_edge].r > 200, "starts at the left edge");
        assert!(render(config.clone(), 1.0)[right_edge].r > 200, "half a period later at the right");
        assert!(render(config.clone(), 1.0)[left_edge].r < 50, "left edge dark meanwhile");
        assert!(render(config, 2.0)[left_edge].r > 200, "back at the left");
    }

    #[test]
    fn huge_times_stay_finite() {
        for config in [
            EffectConfig::RainbowWave { speed: 0.5, angle: 0.0, wavelength: 1.0 },
            EffectConfig::ColorWave { color: RED, background: BLUE, speed: 3.0, width: 0.2, angle: 45.0 },
            EffectConfig::SpiralRainbow { speed: 2.0 },
            EffectConfig::Visor { color: RED, background: BLUE, speed: 1.0, width: 0.2 },
        ] {
            let frame = render(config, 36_000_000.0);
            assert!(frame.iter().any(|&c| c != Rgb::BLACK));
        }
    }
}

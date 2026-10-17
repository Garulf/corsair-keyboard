use std::f32::consts::TAU;

use k100::{Frame, Rgb};

use crate::color::{lerp, scale};
use crate::layer::{Effect, FrameCtx};

const TAIL: f32 = 0.3;

fn cycles(ctx: &FrameCtx, speed: f32) -> f64 {
    ctx.time.as_secs_f64() * f64::from(speed)
}

fn fill(ctx: &FrameCtx, out: &mut Frame, color: Rgb) {
    for geometry in ctx.layout.leds() {
        out[geometry.index as usize] = color;
    }
}

pub struct ColorShift {
    pub colors: Vec<Rgb>,
    pub speed: f32,
}

impl Effect for ColorShift {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let count = self.colors.len();
        if count == 0 {
            return;
        }
        let position = cycles(ctx, self.speed) * count as f64;
        let index = position.floor().rem_euclid(count as f64) as usize;
        let t = position.rem_euclid(1.0) as f32;
        fill(ctx, out, lerp(self.colors[index], self.colors[(index + 1) % count], t));
    }
}

pub struct ColorPulse {
    pub colors: Vec<Rgb>,
    pub speed: f32,
}

impl Effect for ColorPulse {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let count = self.colors.len();
        if count == 0 {
            return;
        }
        let position = cycles(ctx, self.speed);
        let index = position.floor().rem_euclid(count as f64) as usize;
        let level = 0.5 - 0.5 * (TAU * position.rem_euclid(1.0) as f32).cos();
        fill(ctx, out, scale(self.colors[index], level));
    }
}

pub struct Rain {
    pub color: Rgb,
    pub background: Rgb,
    pub speed: f32,
    pub columns: u8,
}

impl Effect for Rain {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        let columns = f32::from(self.columns.max(1));
        let base = cycles(ctx, self.speed);
        for geometry in ctx.layout.leds() {
            let (x, y) = ctx.layout.normalized(geometry.index).unwrap_or((0.0, 0.0));
            let column = (x * columns).floor() as u32;
            let fall = (base + f64::from(column_phase(column))).rem_euclid(1.0) as f32;
            let head = fall * (1.0 + 2.0 * TAIL) - TAIL;
            let distance = head - y;
            let level = if (0.0..=TAIL).contains(&distance) { 1.0 - distance / TAIL } else { 0.0 };
            out[geometry.index as usize] = lerp(self.background, self.color, level);
        }
    }
}

fn column_phase(column: u32) -> f32 {
    (column.wrapping_mul(2_654_435_761) >> 8) as f32 / (1u32 << 24) as f32
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::effects::EffectConfig;
    use crate::layer::FrameCtx;
    use crate::layout::Layout;
    use crate::spectrum::Spectrum;
    use k100::{Frame, LED_COUNT, Rgb};

    fn render(config: &EffectConfig, seconds: f32) -> Frame {
        let layout = Layout::ansi();
        let mut out = [Rgb::BLACK; LED_COUNT];
        let ctx = FrameCtx {
            time: Duration::from_secs_f32(seconds),
            layout: &layout,
            presses: &[],
            spectrum: &Spectrum::SILENT,
        };
        config.build().render(&ctx, &mut out);
        out
    }

    const RED: Rgb = Rgb::new(255, 0, 0);
    const GREEN: Rgb = Rgb::new(0, 255, 0);

    #[test]
    fn color_shift_crossfades_between_colors() {
        let config = EffectConfig::ColorShift { colors: vec![RED, GREEN], speed: 0.5 };
        assert_eq!(render(&config, 0.0)[4], RED);
        assert_eq!(render(&config, 1.0)[4], GREEN);
        assert_eq!(render(&config, 0.5)[4], Rgb::new(128, 128, 0));
        assert_eq!(render(&config, 0.5)[4], render(&config, 0.5)[99], "whole zone shares the color");
    }

    #[test]
    fn color_pulse_breathes_through_black() {
        let config = EffectConfig::ColorPulse { colors: vec![RED, GREEN], speed: 0.5 };
        assert_eq!(render(&config, 0.0)[4], Rgb::BLACK);
        assert_eq!(render(&config, 1.0)[4], RED);
        assert_eq!(render(&config, 3.0)[4], GREEN, "second cycle uses the next color");
    }

    #[test]
    fn empty_color_lists_render_black() {
        for config in [
            EffectConfig::ColorShift { colors: vec![], speed: 1.0 },
            EffectConfig::ColorPulse { colors: vec![], speed: 1.0 },
        ] {
            assert!(render(&config, 0.7).iter().all(|&c| c == Rgb::BLACK));
        }
    }

    #[test]
    fn rain_is_deterministic_and_partial() {
        let config = EffectConfig::Rain { color: RED, background: Rgb::BLACK, speed: 0.5, columns: 24 };
        let a = render(&config, 1.3);
        assert_eq!(a, render(&config, 1.3));
        let lit = a.iter().filter(|c| c.r > 0).count();
        assert!(lit > 0 && lit < 120, "some drops, not a solid fill: {lit}");
        assert_ne!(a, render(&config, 1.5));
    }
}

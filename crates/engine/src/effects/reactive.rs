use std::time::Duration;

use k100::{Frame, LED_COUNT, Rgb};

use crate::color::lerp;
use crate::layer::{Effect, FrameCtx, KeyPress};

fn age(now: Duration, then: Duration) -> f32 {
    now.saturating_sub(then).as_secs_f32()
}

pub struct Type {
    pub color: Rgb,
    pub background: Rgb,
    pub fade: Duration,
    pub last_press: [Option<Duration>; LED_COUNT],
}

impl Effect for Type {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        for press in ctx.presses {
            if ctx.layout.get(press.led).is_some() {
                self.last_press[press.led as usize] = Some(press.at);
            }
        }
        let fade = self.fade.as_secs_f32().max(f32::EPSILON);
        for geometry in ctx.layout.leds() {
            let index = geometry.index as usize;
            let level = self.last_press[index].map_or(0.0, |at| 1.0 - age(ctx.time, at) / fade);
            out[index] = lerp(self.background, self.color, level);
        }
    }
}

pub struct Ripple {
    pub color: Rgb,
    pub background: Rgb,
    pub speed: f32,
    pub width: f32,
    pub lifetime: Duration,
    pub active: Vec<(KeyPress, (f32, f32))>,
}

impl Effect for Ripple {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        for press in ctx.presses {
            if let Some(geometry) = ctx.layout.get(press.led) {
                self.active.push((*press, geometry.rect.center()));
            }
        }
        let lifetime = self.lifetime;
        self.active
            .retain(|(press, _)| ctx.time.saturating_sub(press.at) <= lifetime && press.at <= ctx.time + lifetime);
        let lifetime_s = lifetime.as_secs_f32().max(f32::EPSILON);
        let half_width = (self.width / 2.0).max(1.0);
        for geometry in ctx.layout.leds() {
            let (x, y) = geometry.rect.center();
            let level = self
                .active
                .iter()
                .map(|(press, (cx, cy))| {
                    let elapsed = age(ctx.time, press.at);
                    let distance = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                    let ring = 1.0 - ((distance - elapsed * self.speed).abs() / half_width).min(1.0);
                    ring * (1.0 - elapsed / lifetime_s)
                })
                .fold(0.0, f32::max);
            out[geometry.index as usize] = lerp(self.background, self.color, level);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::effects::EffectConfig;
    use crate::layer::{Effect, FrameCtx, KeyPress};
    use crate::layout::Layout;
    use k100::{Frame, LED_COUNT, Rgb};

    const RED: Rgb = Rgb::new(255, 0, 0);

    fn step(effect: &mut Box<dyn Effect>, layout: &Layout, ms: u64, presses: &[KeyPress]) -> Frame {
        let mut out = [Rgb::BLACK; LED_COUNT];
        effect.render(&FrameCtx { time: Duration::from_millis(ms), layout, presses }, &mut out);
        out
    }

    fn press(led: u8, ms: u64) -> KeyPress {
        KeyPress { led, at: Duration::from_millis(ms) }
    }

    #[test]
    fn type_lights_pressed_key_and_fades() {
        let layout = Layout::ansi();
        let mut effect = EffectConfig::Type { color: RED, background: Rgb::BLACK, fade_ms: 1000 }.build();
        let frame = step(&mut effect, &layout, 100, &[press(4, 100)]);
        assert_eq!(frame[4], RED);
        assert_eq!(frame[22], Rgb::BLACK);
        assert_eq!(step(&mut effect, &layout, 600, &[])[4], Rgb::new(128, 0, 0));
        assert_eq!(step(&mut effect, &layout, 1200, &[])[4], Rgb::BLACK);
    }

    #[test]
    fn ripple_ring_spreads_from_the_key() {
        let layout = Layout::ansi();
        let config =
            EffectConfig::Ripple { color: RED, background: Rgb::BLACK, speed: 1000.0, width: 120.0, lifetime_ms: 2000 };
        let mut effect = config.build();
        let at_press = step(&mut effect, &layout, 0, &[press(44, 0)]);
        assert!(at_press[44].r > 200, "space lights at once");
        let later = step(&mut effect, &layout, 300, &[]);
        assert!(later[44].r < 50, "center dark once the ring moved on");
        let (cx, cy) = layout.get(44).unwrap().rect.center();
        let ring_lit = layout.leds().iter().any(|g| {
            let (x, y) = g.rect.center();
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            (280.0..320.0).contains(&d) && later[g.index as usize].r > 0
        });
        assert!(ring_lit, "keys about 300 px away are lit");
        assert!(step(&mut effect, &layout, 2500, &[]).iter().all(|c| c.r == 0), "expired");
    }

    #[test]
    fn ignores_unknown_leds_and_time_going_backwards() {
        let layout = Layout::ansi();
        let mut type_effect = EffectConfig::Type { color: RED, background: Rgb::BLACK, fade_ms: 500 }.build();
        let mut ripple =
            EffectConfig::Ripple { color: RED, background: Rgb::BLACK, speed: 1000.0, width: 120.0, lifetime_ms: 500 }
                .build();
        for effect in [&mut type_effect, &mut ripple] {
            step(effect, &layout, 5000, &[press(250, 5000), press(4, 5000)]);
            step(effect, &layout, 10, &[]);
        }
    }
}

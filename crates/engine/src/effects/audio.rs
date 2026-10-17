use std::time::Duration;

use k100::{Frame, Rgb};

use crate::color::lerp;
use crate::layer::{Effect, FrameCtx};
use crate::spectrum::SPECTRUM_BANDS;

const GAIN_RANGE: (f32, f32) = (0.1, 10.0);

pub struct AudioVisualizer {
    pub low: Rgb,
    pub high: Rgb,
    pub background: Rgb,
    pub gain: f32,
    pub decay: Duration,
    pub levels: [f32; SPECTRUM_BANDS],
    pub last: Option<Duration>,
}

impl AudioVisualizer {
    fn follow(&mut self, ctx: &FrameCtx) {
        let elapsed = self.last.map_or(Duration::ZERO, |last| ctx.time.saturating_sub(last));
        self.last = Some(ctx.time);
        let gain = if self.gain.is_finite() { self.gain } else { 1.0 }.clamp(GAIN_RANGE.0, GAIN_RANGE.1);
        let fall = if self.decay.is_zero() { 1.0 } else { elapsed.as_secs_f32() / self.decay.as_secs_f32() };
        for (level, band) in self.levels.iter_mut().zip(ctx.spectrum.bands) {
            let target = (band * gain).min(1.0);
            *level = if target >= *level { target } else { (*level - fall).max(target) };
        }
    }
}

impl Effect for AudioVisualizer {
    fn render(&mut self, ctx: &FrameCtx, out: &mut Frame) {
        self.follow(ctx);
        for geometry in ctx.layout.leds() {
            let (x, y) = ctx.layout.normalized(geometry.index).unwrap_or((0.0, 0.0));
            let level = self.levels[((x * SPECTRUM_BANDS as f32) as usize).min(SPECTRUM_BANDS - 1)];
            let height = 1.0 - y;
            out[geometry.index as usize] =
                if level > 0.0 && height <= level { lerp(self.low, self.high, height) } else { self.background };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::effects::EffectConfig;
    use crate::layer::{Effect, FrameCtx};
    use crate::layout::Layout;
    use crate::spectrum::{SPECTRUM_BANDS, Spectrum};
    use k100::{Frame, LED_COUNT, Rgb};

    const LOW: Rgb = Rgb::new(0, 255, 64);
    const HIGH: Rgb = Rgb::new(255, 0, 0);
    const BACKGROUND: Rgb = Rgb::new(1, 2, 3);
    const LEFT_CTRL: usize = 105;
    const G6: usize = 136;
    const PROFILE_KEY: usize = 128;
    const LIGHT_BAR_FIRST: usize = 138;
    const RIGHT_ARROW: usize = 79;

    fn visualizer(gain: f32, decay_ms: u64) -> Box<dyn Effect> {
        EffectConfig::AudioVisualizer { low: LOW, high: HIGH, background: BACKGROUND, gain, decay_ms }.build()
    }

    fn render(effect: &mut dyn Effect, seconds: f32, spectrum: &Spectrum) -> Frame {
        let layout = Layout::ansi();
        let mut out = [Rgb::BLACK; LED_COUNT];
        let ctx = FrameCtx { time: Duration::from_secs_f32(seconds), layout: &layout, presses: &[], spectrum };
        effect.render(&ctx, &mut out);
        out
    }

    fn flat(level: f32) -> Spectrum {
        Spectrum::from_levels([level; SPECTRUM_BANDS])
    }

    fn laid_out(frame: &Frame) -> Vec<Rgb> {
        Layout::ansi().leds().iter().map(|geometry| frame[geometry.index as usize]).collect()
    }

    #[test]
    fn silence_draws_only_the_background() {
        let frame = render(&mut *visualizer(1.0, 400), 0.0, &Spectrum::SILENT);
        assert!(laid_out(&frame).iter().all(|&color| color == BACKGROUND));
    }

    #[test]
    fn a_full_bass_band_lights_the_left_column_bottom_to_top() {
        let mut bands = [0.0; SPECTRUM_BANDS];
        bands[0] = 1.0;
        let frame = render(&mut *visualizer(1.0, 400), 0.0, &Spectrum::from_levels(bands));
        let bottom = frame[G6];
        assert!(bottom.g > 200 && bottom.r < 40, "near the low colour: {bottom:?}");
        let upper = frame[PROFILE_KEY];
        assert!(upper.r > upper.g, "nearer the high colour: {upper:?}");
        assert_eq!(frame[RIGHT_ARROW], BACKGROUND, "treble side stays dark");
        assert_eq!(frame[LIGHT_BAR_FIRST], BACKGROUND, "other bands are silent");
    }

    #[test]
    fn a_full_spectrum_reaches_the_light_bar_in_the_high_colour() {
        let frame = render(&mut *visualizer(1.0, 400), 0.0, &flat(1.0));
        let top = frame[LIGHT_BAR_FIRST];
        assert!(top.r > 240 && top.g < 20, "near the high colour: {top:?}");
        assert!(frame[LEFT_CTRL].g > 200, "bottom row is near the low colour");
    }

    #[test]
    fn a_half_level_lights_the_bottom_half_only() {
        let frame = render(&mut *visualizer(1.0, 400), 0.0, &flat(0.5));
        assert_ne!(frame[LEFT_CTRL], BACKGROUND);
        assert_ne!(frame[RIGHT_ARROW], BACKGROUND);
        assert_eq!(frame[LIGHT_BAR_FIRST], BACKGROUND);
    }

    #[test]
    fn bars_fall_over_the_decay_time_instead_of_vanishing() {
        let mut effect = visualizer(1.0, 400);
        render(&mut *effect, 0.0, &flat(1.0));
        let halfway = render(&mut *effect, 0.2, &Spectrum::SILENT);
        assert_ne!(halfway[LEFT_CTRL], BACKGROUND);
        assert_eq!(halfway[LIGHT_BAR_FIRST], BACKGROUND);
        let gone = render(&mut *effect, 0.4, &Spectrum::SILENT);
        assert!(laid_out(&gone).iter().all(|&color| color == BACKGROUND));
    }

    #[test]
    fn a_rising_bar_is_taken_at_once_and_no_decay_means_no_smoothing() {
        let mut effect = visualizer(1.0, 0);
        render(&mut *effect, 0.0, &flat(1.0));
        let gone = render(&mut *effect, 0.001, &Spectrum::SILENT);
        assert!(laid_out(&gone).iter().all(|&color| color == BACKGROUND));
        let risen = render(&mut *effect, 0.002, &flat(1.0));
        assert_ne!(risen[LIGHT_BAR_FIRST], BACKGROUND);
    }

    #[test]
    fn gain_is_clamped_and_saturates_at_full_height() {
        let clamped = render(&mut *visualizer(50.0, 400), 0.0, &flat(0.2));
        let maximum = render(&mut *visualizer(10.0, 400), 0.0, &flat(0.2));
        assert_eq!(clamped, maximum);
        assert_ne!(maximum[LIGHT_BAR_FIRST], BACKGROUND, "0.2 at gain 10 fills the column");
    }

    #[test]
    fn a_non_finite_gain_draws_as_unit_gain() {
        let unit = render(&mut *visualizer(1.0, 400), 0.0, &flat(0.2));
        let nan = render(&mut *visualizer(f32::NAN, 400), 0.0, &flat(0.2));
        assert_eq!(nan, unit);
    }

    #[test]
    fn parses_with_only_the_type_given() {
        let config: EffectConfig = toml::from_str(r#"type = "audio_visualizer""#).unwrap();
        assert_eq!(
            config,
            EffectConfig::AudioVisualizer {
                low: Rgb::new(0x00, 0xff, 0x40),
                high: Rgb::new(0xff, 0x00, 0x00),
                background: Rgb::BLACK,
                gain: 1.0,
                decay_ms: 400,
            }
        );
    }
}

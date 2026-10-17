use std::ops::Range;
use std::sync::Arc;

use engine::{SPECTRUM_BANDS, Spectrum};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

const WINDOW: usize = 2048;
const HOP: usize = 1024;
const BINS: usize = WINDOW / 2 + 1;
const LOWEST_HZ: f32 = 40.0;
const HIGHEST_HZ: f32 = 16_000.0;
const FLOOR_DB: f32 = -60.0;

pub struct Analyzer {
    rate: u32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    band_bins: [Range<usize>; SPECTRUM_BANDS],
    ring: Vec<f32>,
    write: usize,
    filled: usize,
    since_hop: usize,
    frame: Vec<f32>,
    bins: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Analyzer {
    pub fn new(rate: u32) -> Analyzer {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let window =
            (0..WINDOW).map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / WINDOW as f32).cos()).collect();
        Analyzer {
            rate,
            window,
            band_bins: std::array::from_fn(|band| bins_of_band(band, rate)),
            ring: vec![0.0; WINDOW],
            write: 0,
            filled: 0,
            since_hop: 0,
            frame: fft.make_input_vec(),
            bins: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&mut self, samples: &[f32], channels: u16) -> Option<Spectrum> {
        let channels = usize::from(channels.max(1));
        let mut latest = None;
        for frame in samples.chunks_exact(channels) {
            self.write_sample(downmix(frame));
            if self.filled == WINDOW && self.since_hop >= HOP {
                self.since_hop = 0;
                latest = Some(self.analyse());
            }
        }
        latest
    }

    fn write_sample(&mut self, sample: f32) {
        self.ring[self.write] = sample;
        self.write = (self.write + 1) % WINDOW;
        self.filled = (self.filled + 1).min(WINDOW);
        self.since_hop += 1;
    }

    fn analyse(&mut self) -> Spectrum {
        let (newer, older) = self.ring.split_at(self.write);
        let chronological = older.iter().chain(newer);
        for ((slot, sample), weight) in self.frame.iter_mut().zip(chronological).zip(&self.window) {
            *slot = sample * weight;
        }
        self.fft
            .process_with_scratch(&mut self.frame, &mut self.bins, &mut self.scratch)
            .expect("buffers are sized by the planner");
        Spectrum::from_levels(std::array::from_fn(|band| {
            let magnitude = self.band_bins[band].clone().map(|bin| self.bins[bin].norm()).fold(0.0, f32::max);
            level_of(magnitude * 4.0 / WINDOW as f32)
        }))
    }
}

fn downmix(frame: &[f32]) -> f32 {
    let sum: f32 = frame.iter().map(|sample| if sample.is_finite() { *sample } else { 0.0 }).sum();
    sum / frame.len() as f32
}

fn level_of(magnitude: f32) -> f32 {
    (20.0 * magnitude.log10() - FLOOR_DB) / -FLOOR_DB
}

pub(crate) fn band_edge(band: usize) -> f32 {
    LOWEST_HZ * (HIGHEST_HZ / LOWEST_HZ).powf(band as f32 / SPECTRUM_BANDS as f32)
}

fn bins_of_band(band: usize, rate: u32) -> Range<usize> {
    let bin_of = |hz: f32| hz * WINDOW as f32 / rate as f32;
    let first = (bin_of(band_edge(band)).ceil() as usize).min(BINS);
    let end = (bin_of(band_edge(band + 1)).ceil() as usize).min(BINS);
    if first < end {
        return first..end;
    }
    let centre = (band_edge(band) * band_edge(band + 1)).sqrt();
    let nearest = (bin_of(centre).round() as usize).min(BINS - 1);
    nearest..nearest + 1
}

#[cfg(test)]
pub(crate) fn band_containing(hz: f32) -> usize {
    (0..SPECTRUM_BANDS).find(|&band| band_edge(band) <= hz && hz < band_edge(band + 1)).unwrap()
}

#[cfg(test)]
pub(crate) fn sine(rate: u32, hz: f64, amplitude: f32, channels: u16, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let phase = std::f64::consts::TAU * hz * frame as f64 / f64::from(rate);
            let value = amplitude * phase.sin() as f32;
            std::iter::repeat_n(value, usize::from(channels))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyse(rate: u32, samples: &[f32], channels: u16) -> Spectrum {
        Analyzer::new(rate).push(samples, channels).expect("enough samples for a spectrum")
    }

    #[test]
    fn silence_gives_a_silent_spectrum() {
        assert_eq!(analyse(48_000, &[0.0; 4096], 1), Spectrum::SILENT);
    }

    #[test]
    fn a_full_scale_sine_peaks_in_its_band() {
        for rate in [44_100, 48_000, 96_000] {
            let spectrum = analyse(rate, &sine(rate, 1000.0, 1.0, 2, 8192), 2);
            let tone_band = band_containing(1000.0);
            assert!(spectrum.bands[tone_band] >= 0.9, "rate {rate}: {:?}", spectrum.bands);
            for band in 0..SPECTRUM_BANDS {
                if band_edge(band + 1) < 250.0 || band_edge(band) > 4000.0 {
                    assert!(spectrum.bands[band] <= 0.3, "rate {rate} band {band}: {:?}", spectrum.bands);
                }
            }
        }
    }

    #[test]
    fn every_band_responds_to_its_own_centre_frequency() {
        for rate in [44_100, 48_000] {
            for band in 0..SPECTRUM_BANDS {
                let centre = f64::from(band_edge(band) * band_edge(band + 1)).sqrt();
                let spectrum = analyse(rate, &sine(rate, centre, 1.0, 1, 4096), 1);
                assert!(spectrum.bands[band] >= 0.8, "rate {rate} band {band} at {centre} Hz: {:?}", spectrum.bands);
            }
        }
    }

    #[test]
    fn quiet_signals_read_lower_than_loud_ones() {
        let spectrum = analyse(48_000, &sine(48_000, 1000.0, 0.01, 1, 4096), 1);
        let level = spectrum.bands[band_containing(1000.0)];
        assert!((0.2..0.5).contains(&level), "level {level}");
    }

    #[test]
    fn non_finite_and_clipped_samples_stay_finite() {
        let hostile = [f32::NAN, f32::INFINITY, 5.0, -5.0, f32::NEG_INFINITY, 0.5];
        let samples: Vec<f32> = hostile.iter().copied().cycle().take(4096).collect();
        let spectrum = analyse(48_000, &samples, 1);
        assert!(spectrum.bands.iter().all(|level| level.is_finite() && (0.0..=1.0).contains(level)));
    }

    #[test]
    fn results_arrive_once_per_hop_whatever_the_chunk_size() {
        for channels in [1_u16, 8] {
            let mut analyzer = Analyzer::new(48_000);
            let data = vec![0.1_f32; 10 * 1024 * usize::from(channels)];
            let mut results = Vec::new();
            let mut fed_frames = 0;
            for chunk in data.chunks(333 * usize::from(channels)) {
                fed_frames += chunk.len() / usize::from(channels);
                if analyzer.push(chunk, channels).is_some() {
                    results.push(fed_frames);
                }
            }
            assert_eq!(results.len(), 9, "channels {channels}: {results:?}");
            assert!(results[0] >= 2048 && results[0] < 2048 + 333, "channels {channels}: {results:?}");
        }
    }
}

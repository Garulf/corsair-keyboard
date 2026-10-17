use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use engine::{SPECTRUM_BANDS, Spectrum};

pub const STALE_AFTER: Duration = Duration::from_millis(150);

const NEVER_PUBLISHED: u64 = 0;

pub struct SpectrumCell {
    bands: [AtomicU32; SPECTRUM_BANDS],
    published_at: AtomicU64,
    origin: Instant,
}

impl SpectrumCell {
    pub fn new() -> SpectrumCell {
        SpectrumCell {
            bands: std::array::from_fn(|_| AtomicU32::new(0)),
            published_at: AtomicU64::new(NEVER_PUBLISHED),
            origin: Instant::now(),
        }
    }

    pub fn publish(&self, spectrum: Spectrum) {
        self.publish_at(spectrum, Instant::now());
    }

    pub fn publish_at(&self, spectrum: Spectrum, now: Instant) {
        for (slot, level) in self.bands.iter().zip(spectrum.bands) {
            slot.store(level.to_bits(), Ordering::Relaxed);
        }
        self.published_at.store(self.stamp(now), Ordering::Release);
    }

    pub fn latest(&self) -> Spectrum {
        self.latest_at(Instant::now())
    }

    pub fn latest_at(&self, now: Instant) -> Spectrum {
        let published_at = self.published_at.load(Ordering::Acquire);
        if published_at == NEVER_PUBLISHED {
            return Spectrum::SILENT;
        }
        let age_ms = self.stamp(now).saturating_sub(published_at);
        if age_ms > STALE_AFTER.as_millis() as u64 {
            return Spectrum::SILENT;
        }
        Spectrum::from_levels(std::array::from_fn(|band| f32::from_bits(self.bands[band].load(Ordering::Relaxed))))
    }

    fn stamp(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.origin).as_millis() as u64 + 1
    }
}

impl Default for SpectrumCell {
    fn default() -> Self {
        SpectrumCell::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> Spectrum {
        Spectrum::from_levels(std::array::from_fn(|band| band as f32 / SPECTRUM_BANDS as f32))
    }

    #[test]
    fn the_latest_published_spectrum_is_read_back() {
        let cell = SpectrumCell::new();
        cell.publish(ramp());
        assert_eq!(cell.latest(), ramp());
    }

    #[test]
    fn a_stale_spectrum_reads_as_silence() {
        let cell = SpectrumCell::new();
        let published = Instant::now();
        cell.publish_at(ramp(), published);
        assert_eq!(cell.latest_at(published + Duration::from_millis(149)), ramp());
        assert_eq!(cell.latest_at(published + Duration::from_millis(151)), Spectrum::SILENT);
    }

    #[test]
    fn nothing_published_reads_as_silence() {
        let cell = SpectrumCell::new();
        assert_eq!(cell.latest(), Spectrum::SILENT);
    }
}

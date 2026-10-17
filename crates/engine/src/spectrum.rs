pub const SPECTRUM_BANDS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spectrum {
    pub bands: [f32; SPECTRUM_BANDS],
}

impl Spectrum {
    pub const SILENT: Spectrum = Spectrum { bands: [0.0; SPECTRUM_BANDS] };

    pub fn from_levels(levels: [f32; SPECTRUM_BANDS]) -> Spectrum {
        Spectrum { bands: levels.map(|level| if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 }) }
    }
}

impl Default for Spectrum {
    fn default() -> Self {
        Spectrum::SILENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_clamped_and_non_finite_values_become_silence() {
        let mut levels = [0.5; SPECTRUM_BANDS];
        levels[0] = 3.0;
        levels[1] = -1.0;
        levels[2] = f32::NAN;
        levels[3] = f32::INFINITY;
        let spectrum = Spectrum::from_levels(levels);
        assert_eq!(&spectrum.bands[..5], &[1.0, 0.0, 0.0, 0.0, 0.5]);
        assert_eq!(Spectrum::default(), Spectrum::SILENT);
    }
}

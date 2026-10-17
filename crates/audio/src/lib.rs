use std::sync::Arc;

mod analyzer;
mod cell;

pub use analyzer::Analyzer;
pub use cell::{STALE_AFTER, SpectrumCell};

pub struct Samples<'a> {
    pub data: &'a [f32],
    pub channels: u16,
    pub rate: u32,
}

pub struct Pipeline {
    cell: Arc<SpectrumCell>,
    analyzer: Option<Analyzer>,
}

impl Pipeline {
    pub fn new(cell: Arc<SpectrumCell>) -> Pipeline {
        Pipeline { cell, analyzer: None }
    }

    pub fn feed(&mut self, samples: Samples<'_>) {
        let analyzer = match &mut self.analyzer {
            Some(analyzer) if analyzer.rate() == samples.rate => analyzer,
            slot => slot.insert(Analyzer::new(samples.rate)),
        };
        if let Some(spectrum) = analyzer.push(samples.data, samples.channels) {
            self.cell.publish(spectrum);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{band_containing, sine};
    use engine::Spectrum;

    #[test]
    fn the_pipeline_publishes_and_follows_a_rate_change() {
        let cell = Arc::new(SpectrumCell::new());
        let mut pipeline = Pipeline::new(Arc::clone(&cell));
        let tone_band = band_containing(1000.0);

        for rate in [48_000, 96_000] {
            cell.publish(Spectrum::SILENT);
            let data = sine(rate, 1000.0, 1.0, 2, 8192);
            pipeline.feed(Samples { data: &data, channels: 2, rate });
            let bands = cell.latest().bands;
            assert!(bands[tone_band] >= 0.9, "rate {rate}: {bands:?}");
            assert!(bands[tone_band - 2] <= 0.3, "rate {rate}: {bands:?}");
            assert!(bands[tone_band + 2] <= 0.3, "rate {rate}: {bands:?}");
        }
    }
}

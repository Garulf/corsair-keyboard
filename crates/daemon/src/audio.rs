use std::thread;

use audio::{AudioHandle, Capture, Status};
use engine::{EffectConfig, LayerConfig, Spectrum};
use ipc::AudioState;

pub trait AudioPort: Send {
    fn set_wanted(&mut self, wanted: bool);
    fn spectrum(&self) -> Spectrum;
    fn state(&self) -> AudioState;
}

pub fn uses_audio(layers: &[LayerConfig]) -> bool {
    layers.iter().any(|layer| matches!(layer.effect, EffectConfig::AudioVisualizer { .. }))
}

pub struct NoAudio;

impl AudioPort for NoAudio {
    fn set_wanted(&mut self, _wanted: bool) {}

    fn spectrum(&self) -> Spectrum {
        Spectrum::SILENT
    }

    fn state(&self) -> AudioState {
        AudioState::Off
    }
}

pub struct SystemAudio {
    handle: Option<AudioHandle>,
    capture: Box<dyn Fn() -> Box<dyn Capture> + Send>,
}

impl SystemAudio {
    pub fn new(capture: impl Fn() -> Box<dyn Capture> + Send + 'static) -> Self {
        Self { handle: None, capture: Box::new(capture) }
    }
}

impl Default for SystemAudio {
    fn default() -> Self {
        Self::new(audio::system_capture)
    }
}

impl AudioPort for SystemAudio {
    fn set_wanted(&mut self, wanted: bool) {
        match (wanted, self.handle.is_some()) {
            (true, false) => self.handle = Some(AudioHandle::spawn((self.capture)())),
            (false, true) => {
                if let Some(handle) = self.handle.take() {
                    thread::spawn(move || drop(handle));
                }
            }
            _ => {}
        }
    }

    fn spectrum(&self) -> Spectrum {
        self.handle.as_ref().map_or(Spectrum::SILENT, AudioHandle::spectrum)
    }

    fn state(&self) -> AudioState {
        match self.handle.as_ref().map(AudioHandle::status) {
            None => AudioState::Off,
            Some(Status::Starting) => AudioState::Starting,
            Some(Status::Capturing { device }) => AudioState::Capturing { device },
            Some(Status::Failed { reason }) => AudioState::Failed { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use audio::{Exit, SampleSink};
    use engine::{NamedZone, Zone};
    use k100::Rgb;

    use super::*;

    const SLOW_STOP: Duration = Duration::from_millis(600);

    struct SteadyCapture {
        stop_delay: Duration,
        stopped: Arc<AtomicBool>,
    }

    impl Capture for SteadyCapture {
        fn run(
            &mut self,
            stop: &AtomicBool,
            device: &mut dyn FnMut(String),
            _sink: SampleSink,
        ) -> Result<Exit, String> {
            device("Test sink".into());
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(self.stop_delay);
            self.stopped.store(true, Ordering::SeqCst);
            Ok(Exit::Stopped)
        }
    }

    struct Probe {
        audio: SystemAudio,
        spawned: Arc<AtomicUsize>,
        stopped: Arc<AtomicBool>,
    }

    fn probe(stop_delay: Duration) -> Probe {
        let spawned = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let audio = SystemAudio::new({
            let (spawned, stopped) = (Arc::clone(&spawned), Arc::clone(&stopped));
            move || {
                spawned.fetch_add(1, Ordering::SeqCst);
                Box::new(SteadyCapture { stop_delay, stopped: Arc::clone(&stopped) })
            }
        });
        Probe { audio, spawned, stopped }
    }

    fn wait_for(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not reached in time");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn layer(effect: EffectConfig, enabled: bool) -> LayerConfig {
        LayerConfig { enabled, ..LayerConfig::new("layer", Zone::Named(NamedZone::All), effect) }
    }

    #[test]
    fn only_visualizer_layers_use_audio_even_when_disabled() {
        let solid = layer(EffectConfig::Solid { color: Rgb::BLACK }, true);
        let visualizer = layer(
            EffectConfig::AudioVisualizer {
                low: Rgb::BLACK,
                high: Rgb::BLACK,
                background: Rgb::BLACK,
                gain: 1.0,
                decay_ms: 0,
            },
            false,
        );
        assert!(!uses_audio(&[]));
        assert!(!uses_audio(std::slice::from_ref(&solid)));
        assert!(uses_audio(&[solid, visualizer]));
    }

    #[test]
    fn no_audio_stays_off_and_silent() {
        let mut none = NoAudio;
        none.set_wanted(true);
        assert_eq!((none.state(), none.spectrum()), (AudioState::Off, Spectrum::SILENT));
    }

    #[test]
    fn nothing_runs_until_audio_is_wanted() {
        let mut probe = probe(Duration::ZERO);
        probe.audio.set_wanted(false);
        assert_eq!((probe.audio.state(), probe.audio.spectrum()), (AudioState::Off, Spectrum::SILENT));
        assert_eq!(probe.spawned.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn wanting_audio_twice_captures_once_and_reports_the_device() {
        let mut probe = probe(Duration::ZERO);
        probe.audio.set_wanted(true);
        probe.audio.set_wanted(true);
        wait_for(|| probe.audio.state() == AudioState::Capturing { device: "Test sink".into() });
        assert_eq!(probe.spawned.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn releasing_audio_stops_the_capture_and_reports_off() {
        let mut probe = probe(Duration::ZERO);
        probe.audio.set_wanted(true);
        wait_for(|| matches!(probe.audio.state(), AudioState::Capturing { .. }));
        probe.audio.set_wanted(false);
        assert_eq!(probe.audio.state(), AudioState::Off);
        wait_for(|| probe.stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn releasing_audio_does_not_wait_for_a_slow_capture_to_stop() {
        let mut probe = probe(SLOW_STOP);
        probe.audio.set_wanted(true);
        wait_for(|| matches!(probe.audio.state(), AudioState::Capturing { .. }));
        let started = Instant::now();
        probe.audio.set_wanted(false);
        assert!(started.elapsed() < SLOW_STOP / 2, "release took {:?}", started.elapsed());
        wait_for(|| probe.stopped.load(Ordering::SeqCst));
    }

    #[test]
    fn a_failing_capture_is_reported_with_its_reason() {
        struct Broken;
        impl Capture for Broken {
            fn run(&mut self, _: &AtomicBool, _: &mut dyn FnMut(String), _: SampleSink) -> Result<Exit, String> {
                Err("no sink".into())
            }
        }
        let mut audio = SystemAudio::new(|| Box::new(Broken));
        audio.set_wanted(true);
        wait_for(|| audio.state() == AudioState::Failed { reason: "no sink".into() });
    }
}

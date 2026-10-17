use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, DeviceId, ErrorKind, FromSample, Host, InputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig,
};

use crate::Samples;
use crate::capture::{Capture, DEVICE_CHECK_EVERY, Exit, SampleSink, follow_moved_default};

const POLL_EVERY: Duration = Duration::from_millis(50);

type EndingSlot = Arc<Mutex<Option<Result<Exit, String>>>>;

pub struct Wasapi;

impl Capture for Wasapi {
    fn run(&mut self, stop: &AtomicBool, device: &mut dyn FnMut(String), sink: SampleSink) -> Result<Exit, String> {
        let host = cpal::default_host();
        let output = host.default_output_device().ok_or("no default audio output device")?;
        let id = output.id().map_err(|error| error.to_string())?;
        let name = output.description().map_err(|error| error.to_string())?.name().to_owned();
        let supported = output.default_output_config().map_err(|error| error.to_string())?;
        let ending = EndingSlot::default();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => loopback::<f32>(&output, supported.config(), sink, &ending),
            SampleFormat::I16 => loopback::<i16>(&output, supported.config(), sink, &ending),
            SampleFormat::U16 => loopback::<u16>(&output, supported.config(), sink, &ending),
            other => Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|error| error.to_string())?;
        device(name);

        let mut next_check = Instant::now() + DEVICE_CHECK_EVERY;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(Exit::Stopped);
            }
            if let Some(ending) = ending.lock().unwrap().take() {
                return follow_moved_default(ending, || default_output_id(&host).as_ref() != Some(&id));
            }
            if Instant::now() >= next_check {
                next_check += DEVICE_CHECK_EVERY;
                if default_output_id(&host).as_ref() != Some(&id) {
                    return Ok(Exit::DeviceChanged);
                }
            }
            thread::sleep(POLL_EVERY);
        }
    }
}

fn default_output_id(host: &Host) -> Option<DeviceId> {
    host.default_output_device()?.id().ok()
}

fn ending_for(error: &cpal::Error) -> Option<Result<Exit, String>> {
    match error.kind() {
        ErrorKind::Xrun => None,
        ErrorKind::StreamInvalidated | ErrorKind::DeviceChanged => Some(Ok(Exit::DeviceChanged)),
        _ => Some(Err(error.to_string())),
    }
}

fn loopback<T>(
    output: &Device,
    config: StreamConfig,
    mut sink: SampleSink,
    ending: &EndingSlot,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels;
    let rate = config.sample_rate;
    let mut converted = Vec::new();
    let on_data = move |data: &[T], _: &InputCallbackInfo| {
        converted.clear();
        converted.extend(data.iter().map(|sample| sample.to_sample::<f32>()));
        sink(Samples { data: &converted, channels, rate });
    };
    let ending = Arc::clone(ending);
    let on_error = move |error: cpal::Error| {
        if let Some(outcome) = ending_for(&error) {
            ending.lock().unwrap().get_or_insert(outcome);
        }
    };
    output.build_input_stream(config, on_data, on_error, None).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_xrun_keeps_the_stream_running() {
        assert_eq!(ending_for(&ErrorKind::Xrun.into()), None);
    }

    #[test]
    fn an_invalidated_stream_follows_the_new_default_device() {
        assert_eq!(ending_for(&ErrorKind::StreamInvalidated.into()), Some(Ok(Exit::DeviceChanged)));
        assert_eq!(ending_for(&ErrorKind::DeviceChanged.into()), Some(Ok(Exit::DeviceChanged)));
    }

    #[test]
    fn a_lost_device_is_a_failure() {
        let error = cpal::Error::from(ErrorKind::DeviceNotAvailable);
        assert_eq!(ending_for(&error), Some(Err(error.to_string())));
    }
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Host, InputCallbackInfo, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::Samples;
use crate::capture::{Capture, DEVICE_CHECK_EVERY, Exit, SampleSink};

const POLL_EVERY: Duration = Duration::from_millis(50);

type FailureSlot = Arc<Mutex<Option<String>>>;

pub struct Wasapi;

impl Capture for Wasapi {
    fn run(&mut self, stop: &AtomicBool, device: &mut dyn FnMut(String), sink: SampleSink) -> Result<Exit, String> {
        let host = cpal::default_host();
        let output = host.default_output_device().ok_or("no default audio output device")?;
        let name = output.description().map_err(|error| error.to_string())?.name().to_owned();
        let supported = output.default_output_config().map_err(|error| error.to_string())?;
        let failure = FailureSlot::default();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => loopback::<f32>(&output, supported.config(), sink, &failure),
            SampleFormat::I16 => loopback::<i16>(&output, supported.config(), sink, &failure),
            SampleFormat::U16 => loopback::<u16>(&output, supported.config(), sink, &failure),
            other => Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|error| error.to_string())?;
        device(name.clone());

        let mut next_check = Instant::now() + DEVICE_CHECK_EVERY;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(Exit::Stopped);
            }
            if let Some(reason) = failure.lock().unwrap().take() {
                return Err(reason);
            }
            if Instant::now() >= next_check {
                next_check += DEVICE_CHECK_EVERY;
                if default_output_name(&host).as_ref() != Some(&name) {
                    return Ok(Exit::DeviceChanged);
                }
            }
            thread::sleep(POLL_EVERY);
        }
    }
}

fn default_output_name(host: &Host) -> Option<String> {
    let description = host.default_output_device()?.description().ok()?;
    Some(description.name().to_owned())
}

fn loopback<T>(
    output: &Device,
    config: StreamConfig,
    mut sink: SampleSink,
    failure: &FailureSlot,
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
    let failure = Arc::clone(failure);
    let on_error = move |error: cpal::Error| *failure.lock().unwrap() = Some(error.to_string());
    output.build_input_stream(config, on_data, on_error, None).map_err(|error| error.to_string())
}

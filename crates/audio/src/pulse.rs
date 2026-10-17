use std::ffi::CStr;
use std::io::{BufRead, BufReader, Cursor, ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pulseaudio::protocol::stream::{BufferAttr, StreamFlags};
use pulseaudio::protocol::{
    self, AuthParams, AuthReply, ChannelMap, Command, CommandReply, CreateRecordStreamReply, GetSinkInfo, Prop, Props,
    ProtocolError, RecordStreamParams, SampleFormat, SampleSpec, SetClientNameReply, SinkInfo,
};

use crate::Samples;
use crate::capture::{Capture, DEVICE_CHECK_EVERY, Exit, SampleSink, follow_moved_default};

const NO_SERVER: &str = "no PulseAudio or PipeWire (pipewire-pulse) server found";
const RATE: u32 = 48_000;
const CHANNELS: u8 = 2;
const BYTES_PER_SAMPLE: usize = size_of::<f32>();
const FRAGMENT_FRAMES: usize = 480;
const READ_TIMEOUT: Duration = Duration::from_millis(200);
const REPLY_TIMEOUT: Duration = Duration::from_secs(1);
const COMMAND_CHANNEL: u32 = u32::MAX;

pub struct PulseMonitor;

impl Capture for PulseMonitor {
    fn run(&mut self, stop: &AtomicBool, device: &mut dyn FnMut(String), mut sink: SampleSink) -> Result<Exit, String> {
        let mut connection = Connection::open()?;
        let output = connection.default_sink()?;
        let monitor = output.monitor_source_index.ok_or("the default output has no monitor source")?;
        let stream = connection.record(monitor)?;
        if stream.sample_spec.format != SampleFormat::Float32Le {
            return Err(format!("unsupported sample format {:?}", stream.sample_spec.format));
        }
        connection.set_read_timeout(READ_TIMEOUT)?;
        device(display_name(&output));

        let outcome = connection.stream_until_exit(&stream, &output.name, stop, &mut sink);
        follow_moved_default(outcome, || default_moved_from(&output.name))
    }
}

fn default_moved_from(captured: &CStr) -> bool {
    let current = Connection::open().and_then(|mut connection| connection.default_sink()).ok();
    has_moved(captured, current.as_ref().map(|sink| sink.name.as_c_str()))
}

fn has_moved(captured: &CStr, current_default: Option<&CStr>) -> bool {
    current_default.is_some_and(|name| name != captured)
}

fn display_name(sink: &SinkInfo) -> String {
    sink.description.as_ref().unwrap_or(&sink.name).to_string_lossy().into_owned()
}

struct Connection {
    socket: BufReader<UnixStream>,
    version: u16,
    sequence: u32,
}

impl Connection {
    fn open() -> Result<Connection, String> {
        let path = pulseaudio::socket_path_from_env().ok_or(NO_SERVER)?;
        let socket = UnixStream::connect(&path)
            .map_err(|error| format!("cannot connect to the audio server at {}: {error}", path.display()))?;
        let mut connection = Connection { socket: BufReader::new(socket), version: protocol::MAX_VERSION, sequence: 0 };
        connection.set_read_timeout(REPLY_TIMEOUT)?;

        let cookie = pulseaudio::cookie_path_from_env().and_then(|path| std::fs::read(path).ok()).unwrap_or_default();
        let auth = AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        let reply: AuthReply = connection.request(&Command::Auth(auth))?;
        connection.version = reply.version.min(protocol::MAX_VERSION);

        let mut props = Props::new();
        props.set(Prop::ApplicationName, c"K100 audio visualizer");
        let _: SetClientNameReply = connection.request(&Command::SetClientName(props))?;
        Ok(connection)
    }

    fn default_sink(&mut self) -> Result<SinkInfo, String> {
        let query = GetSinkInfo { name: Some(protocol::DEFAULT_SINK.to_owned()), ..Default::default() };
        self.request(&Command::GetSinkInfo(query))
    }

    fn record(&mut self, source: u32) -> Result<CreateRecordStreamReply, String> {
        let fragment_bytes = FRAGMENT_FRAMES * usize::from(CHANNELS) * BYTES_PER_SAMPLE;
        self.request(&Command::CreateRecordStream(RecordStreamParams {
            source_index: Some(source),
            sample_spec: SampleSpec { format: SampleFormat::Float32Le, channels: CHANNELS, sample_rate: RATE },
            channel_map: ChannelMap::stereo(),
            buffer_attr: BufferAttr { fragment_size: fragment_bytes as u32, ..Default::default() },
            flags: StreamFlags { adjust_latency: true, ..Default::default() },
            ..Default::default()
        }))
    }

    fn stream_until_exit(
        &mut self,
        stream: &CreateRecordStreamReply,
        captured: &CStr,
        stop: &AtomicBool,
        sink: &mut SampleSink,
    ) -> Result<Exit, String> {
        let channels = u16::from(stream.sample_spec.channels);
        let rate = stream.sample_spec.sample_rate;
        let mut decoder = FrameDecoder::new(channels);
        let mut packet = Vec::new();
        let mut next_check = Instant::now() + DEVICE_CHECK_EVERY;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(Exit::Stopped);
            }
            if Instant::now() >= next_check {
                next_check += DEVICE_CHECK_EVERY;
                if default_moved_from(captured) {
                    return Ok(Exit::DeviceChanged);
                }
            }
            let Some(channel) = self.read_packet(&mut packet)? else { continue };
            if channel == stream.channel {
                sink(Samples { data: decoder.decode(&packet), channels, rate });
            } else if channel == COMMAND_CHANNEL && self.is_stream_killed(&packet) {
                return Err("the PulseAudio server closed the recording stream".to_string());
            }
        }
    }

    fn request<T: CommandReply>(&mut self, command: &Command) -> Result<T, String> {
        self.sequence += 1;
        protocol::write_command_message(self.socket.get_mut(), self.sequence, command, self.version)
            .map_err(server_error)?;
        let (_, reply) = protocol::read_reply_message(&mut self.socket, self.version).map_err(server_error)?;
        Ok(reply)
    }

    fn set_read_timeout(&self, timeout: Duration) -> Result<(), String> {
        self.socket.get_ref().set_read_timeout(Some(timeout)).map_err(|error| error.to_string())
    }

    fn read_packet(&mut self, packet: &mut Vec<u8>) -> Result<Option<u32>, String> {
        match self.socket.fill_buf() {
            Ok([]) => return Err("the audio server closed the connection".to_string()),
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
        let descriptor = protocol::read_descriptor(&mut self.socket).map_err(server_error)?;
        packet.resize(descriptor.length as usize, 0);
        self.socket.read_exact(packet).map_err(|error| error.to_string())?;
        Ok(Some(descriptor.channel))
    }

    fn is_stream_killed(&self, packet: &[u8]) -> bool {
        matches!(
            Command::read_tag_prefixed(&mut Cursor::new(packet), self.version),
            Ok((_, Command::RecordStreamKilled(_)))
        )
    }
}

fn server_error(error: ProtocolError) -> String {
    format!("audio server: {error}")
}

struct FrameDecoder {
    frame_bytes: usize,
    pending: Vec<u8>,
    samples: Vec<f32>,
}

impl FrameDecoder {
    fn new(channels: u16) -> FrameDecoder {
        FrameDecoder {
            frame_bytes: usize::from(channels.max(1)) * BYTES_PER_SAMPLE,
            pending: Vec::new(),
            samples: Vec::new(),
        }
    }

    fn decode(&mut self, bytes: &[u8]) -> &[f32] {
        self.pending.extend_from_slice(bytes);
        let whole_frames = self.pending.len() - self.pending.len() % self.frame_bytes;
        self.samples.clear();
        let (samples, _) = self.pending[..whole_frames].as_chunks::<BYTES_PER_SAMPLE>();
        self.samples.extend(samples.iter().map(|&sample| f32::from_le_bytes(sample)));
        self.pending.drain(..whole_frames);
        &self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn little_endian(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
    }

    #[test]
    fn a_different_default_sink_has_moved() {
        assert!(has_moved(c"speakers", Some(c"headphones")));
    }

    #[test]
    fn the_same_default_sink_has_not_moved() {
        assert!(!has_moved(c"speakers", Some(c"speakers")));
    }

    #[test]
    fn a_failed_default_sink_check_counts_as_no_change() {
        assert!(!has_moved(c"speakers", None));
    }

    #[test]
    fn whole_frames_decode_to_interleaved_samples() {
        let mut decoder = FrameDecoder::new(2);
        let bytes = little_endian(&[0.5, -0.5, 1.0, -1.0]);
        assert_eq!(decoder.decode(&bytes), [0.5, -0.5, 1.0, -1.0]);
    }

    #[test]
    fn a_frame_split_across_packets_is_held_until_complete() {
        let mut decoder = FrameDecoder::new(2);
        let bytes = little_endian(&[0.25, -0.25, 0.75, -0.75]);
        let (first, second) = bytes.split_at(10);

        assert_eq!(decoder.decode(first), [0.25, -0.25]);
        assert_eq!(decoder.decode(second), [0.75, -0.75]);
    }
}

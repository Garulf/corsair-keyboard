use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bragi::{Notification, OperatingMode, PollingRate, PropertyId};
use clap::Subcommand;
use k100::hid::{HidInput, HidTransport, OpenedDevice, open_device};
use k100::{
    Device, DeviceError, IndicatorRecord, InputSource, KeyMappings, KeyStateTracker, LED_COUNT, Mapping, Rgb, Session,
    led,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Subcommand)]
pub enum Diag {
    #[command(flatten)]
    Keyboard(KeyboardDiag),
    /// Show the captured system audio spectrum as text (needs no keyboard)
    Audio {
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeyboardDiag {
    /// Read identity and settings without changing anything
    Info,
    /// Force the keyboard back to hardware mode with all keys normal
    Release,
    /// Show solid colors on every LED, then restore
    Lights {
        #[arg(default_values = ["ff0000", "00ff00", "0000ff", "ffffff", "000000"])]
        colors: Vec<String>,
        #[arg(long, default_value_t = 1500)]
        hold_ms: u64,
    },
    /// Light the dial ring through the indication buffer with alpha 0xFF, then 0x00
    Indicators,
    /// Measure how many frames per second the keyboard accepts
    Fps {
        #[arg(long, default_value_t = 300)]
        frames: u32,
    },
    /// Write brightness and polling rate back with their current values and read them again
    Props,
    /// Suppress one key's native output for a while
    Mask {
        #[arg(default_value = "0x47", value_parser = parse_key)]
        key: u8,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// Print G-key, key and dial events until Ctrl-C or timeout
    Watch {
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
    /// Enter software mode and exit without restoring, to observe firmware revert
    Kill,
    /// Read an onboard file without changing it and print it as hex
    File {
        #[arg(value_parser = parse_file_id)]
        id: u16,
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Play a layer file on the keyboard (effects react to key presses)
    Preview {
        file: std::path::PathBuf,
        #[arg(long, default_value_t = 20)]
        seconds: u64,
        #[arg(long, default_value_t = 60)]
        fps: u32,
    },
}

pub fn run(command: Diag) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))?;

    match command {
        Diag::Audio { seconds } => audio(seconds, &stop),
        Diag::Keyboard(command) => keyboard(command, &stop),
    }
}

fn keyboard(command: KeyboardDiag, stop: &AtomicBool) -> Result<()> {
    let OpenedDevice { commands, input, model } = open_device()?;
    println!("found {} (pid {:04x}){}", model.name, model.pid, if model.tested { "" } else { " [untested]" });
    match command {
        KeyboardDiag::Info => info(commands),
        KeyboardDiag::Release => release(commands, model.key_count),
        KeyboardDiag::Lights { colors, hold_ms } => lights(Session::start(commands, model)?, &colors, hold_ms, stop),
        KeyboardDiag::Indicators => indicators(Session::start(commands, model)?),
        KeyboardDiag::Fps { frames } => fps(Session::start(commands, model)?, frames, stop),
        KeyboardDiag::Props => props(commands),
        KeyboardDiag::Mask { key, seconds } => mask(Session::start(commands, model)?, key, seconds, stop),
        KeyboardDiag::Watch { seconds } => watch(Session::start(commands, model)?, input, seconds, stop),
        KeyboardDiag::Kill => kill(Session::start(commands, model)?),
        KeyboardDiag::File { id, out } => read_file(commands, id, out.as_deref()),
        KeyboardDiag::Preview { file, seconds, fps } => {
            preview(Session::start(commands, model)?, input, &file, seconds, fps, stop)
        }
    }
}

fn info(commands: HidTransport) -> Result<()> {
    let mut device = Device::new(commands);
    let mut read = |property| device.get_property(property).map_err(describe);
    println!("firmware:         {}", read(PropertyId::FIRMWARE_VERSION).map(format_version)?);
    println!("pid:              {:04x?}", read(PropertyId::USB_PID));
    println!("layout:           {:?}", read(PropertyId::PHYSICAL_KEYBOARD_LAYOUT));
    println!("protocol version: {:?}", read(PropertyId::BRAGI_PROTOCOL_VERSION));
    let mode = read(PropertyId::OPERATING_MODE).map(|raw| OperatingMode::from_raw(raw).ok_or(raw));
    println!("operating mode:   {mode:?}");
    println!("brightness:       {:?}", read(PropertyId::BRIGHTNESS));
    for (name, property) in
        [("polling rate:", PropertyId::POLLING_RATE), ("max polling:", PropertyId::MAXIMUM_POLLING_RATE)]
    {
        let rate = read(property).map(|raw| PollingRate::from_raw(raw).map(PollingRate::hz).ok_or(raw));
        println!("{name:<17} {rate:?}");
    }
    println!("winlock:          {:?}", read(PropertyId::WIN_LOCK_ENABLED));
    println!("key reporting:    {:?}", read(PropertyId::KEY_PRESS_NOTIFICATIONS_REPORTING));
    let enabled = read(PropertyId::PRESS_DEBOUNCE_ENABLED);
    let timeout = read(PropertyId::PRESS_DEBOUNCE_TIMEOUT);
    println!("press debounce:   {enabled:?} / {timeout:?}");
    Ok(())
}

fn describe(error: DeviceError) -> String {
    error.to_string()
}

fn release(commands: HidTransport, key_count: usize) -> Result<()> {
    let mut device = Device::new(commands);
    let result = k100::restore(&mut device, key_count);
    let mode = device.get_property(PropertyId::OPERATING_MODE).map(OperatingMode::from_raw);
    println!("restore: {result:?}; operating mode now {mode:?}");
    Ok(result?)
}

fn lights(mut session: Session<HidTransport>, colors: &[String], hold_ms: u64, stop: &AtomicBool) -> Result<()> {
    for color in colors {
        let rgb = parse_rgb(color)?;
        println!("all LEDs {color}");
        session.write_frame(&[rgb; LED_COUNT])?;
        sleep_unless_stopped(Duration::from_millis(hold_ms), stop);
        if stop.load(Ordering::SeqCst) {
            break;
        }
    }
    Ok(session.release()?)
}

fn indicators(mut session: Session<HidTransport>) -> Result<()> {
    session.write_frame(&[Rgb::new(0, 0, 40); LED_COUNT])?;
    for alpha in [0xFF, 0x00] {
        let records: Vec<IndicatorRecord> = (led::DIAL_RING_FIRST..=led::DIAL_RING_LAST)
            .map(|led| IndicatorRecord { led, color: Rgb::new(255, 0, 0), alpha })
            .collect();
        session.write_indicators(&records)?;
        println!("dial ring red with alpha {alpha:#04x} (rest of keyboard dim blue)");
        thread::sleep(Duration::from_secs(3));
    }
    Ok(session.release()?)
}

fn fps(mut session: Session<HidTransport>, frames: u32, stop: &AtomicBool) -> Result<()> {
    let started = Instant::now();
    let mut sent = 0;
    for n in 0..frames {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        session.write_frame(&rainbow_frame(n))?;
        sent += 1;
    }
    let elapsed = started.elapsed();
    let rate = f64::from(sent) / elapsed.as_secs_f64();
    println!("{sent} frames in {elapsed:?}: {rate:.1} fps, {:?} per frame", elapsed / sent.max(1));
    Ok(session.release()?)
}

fn props(commands: HidTransport) -> Result<()> {
    let mut device = Device::new(commands);
    for property in [PropertyId::BRIGHTNESS, PropertyId::POLLING_RATE] {
        let before = device.get_property(property)?;
        device.set_property(property, before)?;
        let after = device.get_property(property)?;
        println!("{property:?}: {before} -> wrote {before} -> read {after}");
    }
    Ok(())
}

fn mask(mut session: Session<HidTransport>, key: u8, seconds: u64, stop: &AtomicBool) -> Result<()> {
    let mut mappings = KeyMappings::all_normal(session.model().key_count);
    mappings.set(key, Mapping::NO_REPORT);
    session.set_key_mappings(&mappings)?;
    session.write_frame(&highlight_frame(Some(key)))?;
    println!("key {key:#04x} (lit red) suppressed for {seconds}s; press it now, it should do nothing");
    sleep_unless_stopped(Duration::from_secs(seconds), stop);
    Ok(session.release()?)
}

fn watch(mut session: Session<HidTransport>, mut input: HidInput, seconds: u64, stop: &AtomicBool) -> Result<()> {
    if !session.try_set_property(PropertyId::KEY_PRESS_NOTIFICATIONS_REPORTING, 1)? {
        println!("key press reporting property not supported; showing whatever the keyboard sends");
    }
    session.write_frame(&highlight_frame(None))?;
    let mut tracker = KeyStateTracker::default();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last_ping = Instant::now();
    println!("watching for {seconds}s: press G-keys, turn the dial, press keys");
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        if last_ping.elapsed() >= k100::PING_INTERVAL {
            session.ping()?;
            last_ping = Instant::now();
        }
        let Some(report) = input.read(Duration::from_millis(100))? else {
            continue;
        };
        let events = Notification::parse(&report).map(|n| tracker.update(&n)).unwrap_or_default();
        println!("raw {:02x?} -> {events:?}", &report[..report.len().min(24)]);
    }
    Ok(session.release()?)
}

fn kill(mut session: Session<HidTransport>) -> Result<()> {
    session.write_frame(&[Rgb::new(255, 0, 255); LED_COUNT])?;
    println!("software mode with magenta lights; exiting without restoring");
    std::mem::forget(session);
    std::process::exit(0);
}

const READ_HANDLE: u8 = 3;

fn read_file(commands: HidTransport, id: u16, out: Option<&std::path::Path>) -> Result<()> {
    let data = Device::new(commands).read_file(READ_HANDLE, bragi::FileId(id))?;
    println!("{}", hex_dump(&data));
    if let Some(path) = out {
        std::fs::write(path, &data)?;
        println!("wrote {} bytes to {}", data.len(), path.display());
    }
    Ok(())
}

fn hex_dump(bytes: &[u8]) -> String {
    let shown = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| (i / 16 + 1) * 16).min(bytes.len());
    let mut lines: Vec<String> = bytes[..shown]
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            format!("{:04x}  {}", row * 16, hex.join(" "))
        })
        .collect();
    let hidden = bytes.len() - shown;
    lines.push(match hidden {
        0 => format!("({} bytes total)", bytes.len()),
        _ => format!("({} bytes total, {hidden} trailing zero bytes not shown)", bytes.len()),
    });
    lines.join("\n")
}

fn parse_file_id(text: &str) -> std::result::Result<u16, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|error| format!("invalid file id {text:?}: {error}"))
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn parse_rgb(text: &str) -> std::result::Result<Rgb, String> {
    let hex = text.trim_start_matches('#');
    let value = (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok()).flatten();
    let value = value.ok_or_else(|| format!("expected a 6-digit hex color, got {text:?}"))?;
    let [_, r, g, b] = value.to_be_bytes();
    Ok(Rgb::new(r, g, b))
}

pub fn parse_key(text: &str) -> std::result::Result<u8, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u8::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|error| format!("invalid key index {text:?}: {error}"))
}

#[derive(serde::Deserialize)]
struct LayerFile {
    layers: Vec<engine::LayerConfig>,
}

pub fn load_layers(text: &str) -> std::result::Result<Vec<engine::LayerConfig>, String> {
    toml::from_str::<LayerFile>(text).map(|file| file.layers).map_err(|error| error.to_string())
}

pub fn frame_interval(fps: u32) -> Duration {
    Duration::from_secs(1) / fps.clamp(1, 200)
}

fn preview(
    mut session: Session<HidTransport>,
    mut input: HidInput,
    file: &std::path::Path,
    seconds: u64,
    fps: u32,
    stop: &AtomicBool,
) -> Result<()> {
    let layers = load_layers(&std::fs::read_to_string(file)?)?;
    let mut compositor = engine::Compositor::new(engine::Layout::ansi(), &layers);
    let mut tracker = KeyStateTracker::default();
    let interval = frame_interval(fps);
    let started = Instant::now();
    let mut last_ping = Instant::now();
    let mut frames = 0u32;
    while started.elapsed() < Duration::from_secs(seconds) && !stop.load(Ordering::SeqCst) {
        let frame_start = Instant::now();
        let mut presses = Vec::new();
        while let Some(report) = input.read(Duration::ZERO)? {
            let events = Notification::parse(&report).map(|n| tracker.update(&n)).unwrap_or_default();
            presses.extend(events.into_iter().filter_map(|event| match event {
                k100::InputEvent::KeyDown(led) => Some(engine::KeyPress { led, at: started.elapsed() }),
                _ => None,
            }));
        }
        session.write_frame(&compositor.render(started.elapsed(), &presses, &engine::Spectrum::SILENT))?;
        frames += 1;
        if last_ping.elapsed() >= k100::PING_INTERVAL {
            session.ping()?;
            last_ping = Instant::now();
        }
        thread::sleep(interval.saturating_sub(frame_start.elapsed()));
    }
    println!("{frames} frames in {:?}", started.elapsed());
    Ok(session.release()?)
}

const METER_RAMP: &[u8] = b" .:-=+*#%@";
const METER_INTERVAL: Duration = Duration::from_millis(100);

pub fn band_line(spectrum: &engine::Spectrum) -> String {
    spectrum
        .bands
        .iter()
        .map(|level| char::from(METER_RAMP[(level * (METER_RAMP.len() - 1) as f32).round() as usize]))
        .collect()
}

fn status_line(status: &audio::Status) -> String {
    match status {
        audio::Status::Starting => "audio: starting".to_string(),
        audio::Status::Capturing { device } => format!("audio: capturing ({device})"),
        audio::Status::Failed { reason } => format!("audio: failed: {reason}"),
    }
}

fn meter_lines(previous: Option<&audio::Status>, status: &audio::Status, spectrum: &engine::Spectrum) -> Vec<String> {
    let mut lines = Vec::new();
    if previous != Some(status) {
        lines.push(status_line(status));
    }
    if matches!(status, audio::Status::Capturing { .. }) {
        lines.push(band_line(spectrum));
    }
    lines
}

fn audio(seconds: u64, stop: &AtomicBool) -> Result<()> {
    let handle = audio::AudioHandle::spawn(audio::system_capture());
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last_status = None;
    loop {
        let status = handle.status();
        for line in meter_lines(last_status.as_ref(), &status, &handle.spectrum()) {
            println!("{line}");
        }
        last_status = Some(status);
        if Instant::now() >= deadline || stop.load(Ordering::SeqCst) {
            break;
        }
        thread::sleep(METER_INTERVAL);
    }
    match handle.status() {
        audio::Status::Failed { reason } => Err(reason.into()),
        _ => Ok(()),
    }
}

pub fn format_version(raw: u32) -> String {
    let [major, minor, build_lo, build_hi] = raw.to_le_bytes();
    format!("{major}.{minor}.{}", u16::from_le_bytes([build_lo, build_hi]))
}

pub fn highlight_frame(key: Option<u8>) -> [Rgb; LED_COUNT] {
    let mut frame = [Rgb::new(60, 60, 60); LED_COUNT];
    if let Some(key) = key.filter(|&key| usize::from(key) < LED_COUNT) {
        frame[usize::from(key)] = Rgb::new(255, 0, 0);
    }
    frame
}

pub fn rainbow_frame(step: u32) -> [Rgb; LED_COUNT] {
    std::array::from_fn(|i| hue((i as u32 * 7 + step * 4) % 360))
}

fn hue(degrees: u32) -> Rgb {
    let x = ((degrees % 60) * 255 / 60) as u8;
    match degrees / 60 {
        0 => Rgb::new(255, x, 0),
        1 => Rgb::new(255 - x, 255, 0),
        2 => Rgb::new(0, 255, x),
        3 => Rgb::new(0, 255 - x, 255),
        4 => Rgb::new(x, 0, 255),
        _ => Rgb::new(255, 0, 255 - x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_rgb("ff8000"), Ok(Rgb::new(255, 128, 0)));
        assert_eq!(parse_rgb("#00ff00"), Ok(Rgb::new(0, 255, 0)));
        assert!(parse_rgb("12345").is_err());
        assert!(parse_rgb("gg0000").is_err());
    }

    #[test]
    fn formats_firmware_versions() {
        assert_eq!(format_version(0x0103_0020), "32.0.259");
    }

    #[test]
    fn rainbow_frames_cover_all_leds() {
        let frame = rainbow_frame(0);
        assert_eq!(frame.len(), LED_COUNT);
        assert_ne!(frame[0], frame[96]);
    }

    #[test]
    fn parses_decimal_and_hex_key_indices() {
        assert_eq!(parse_key("71"), Ok(71));
        assert_eq!(parse_key("0x47"), Ok(71));
        assert_eq!(parse_key("0X83"), Ok(131));
        assert!(parse_key("256").is_err());
        assert!(parse_key("0xzz").is_err());
    }

    #[test]
    fn loads_layer_files() {
        let text = "[[layers]]\nname = \"a\"\nzone = \"all\"\neffect = { type = \"solid\", color = \"#010203\" }\n";
        assert_eq!(load_layers(text).unwrap().len(), 1);
        assert!(load_layers("layers = 3").is_err());
    }

    #[test]
    fn frame_interval_is_clamped() {
        assert_eq!(frame_interval(50), Duration::from_millis(20));
        assert_eq!(frame_interval(0), Duration::from_secs(1));
        assert_eq!(frame_interval(10_000), Duration::from_millis(5));
    }

    #[test]
    fn example_layer_file_parses() {
        let text = include_str!("../../../examples/layers/rainbow-wasd.toml");
        assert!(load_layers(text).unwrap().len() >= 3);
    }

    #[test]
    fn highlight_frame_is_dim_white_with_one_red_key() {
        let frame = highlight_frame(Some(29));
        assert_eq!(frame[29], Rgb::new(255, 0, 0));
        assert_eq!(frame[4], Rgb::new(60, 60, 60));
        assert!(highlight_frame(None).iter().all(|&c| c == Rgb::new(60, 60, 60)));
    }

    #[test]
    fn the_meter_prints_a_new_status_once_and_bands_only_while_capturing() {
        let starting = audio::Status::Starting;
        let failed = audio::Status::Failed { reason: "no sink".to_string() };
        let capturing = audio::Status::Capturing { device: "Speakers".to_string() };
        let full = engine::Spectrum::from_levels([1.0; engine::SPECTRUM_BANDS]);

        assert_eq!(meter_lines(None, &starting, &full), ["audio: starting"]);
        assert!(meter_lines(Some(&starting), &starting, &full).is_empty());
        assert_eq!(meter_lines(Some(&starting), &failed, &full), ["audio: failed: no sink"]);
        assert!(meter_lines(Some(&failed), &failed, &full).is_empty());
        assert_eq!(
            meter_lines(Some(&failed), &capturing, &full),
            ["audio: capturing (Speakers)".to_string(), "@".repeat(32)]
        );
        assert_eq!(meter_lines(Some(&capturing), &capturing, &full), ["@".repeat(32)]);
    }

    #[test]
    fn audio_and_keyboard_checks_parse_as_diag_subcommands() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            diag: Diag,
        }

        let audio = Cli::try_parse_from(["diag", "audio", "--seconds", "3"]).unwrap().diag;
        assert!(matches!(audio, Diag::Audio { seconds: 3 }));
        let info = Cli::try_parse_from(["diag", "info"]).unwrap().diag;
        assert!(matches!(info, Diag::Keyboard(KeyboardDiag::Info)));
        let file = Cli::try_parse_from(["diag", "file", "0x0f"]).unwrap().diag;
        assert!(matches!(file, Diag::Keyboard(KeyboardDiag::File { id: 0x0F, out: None })));
        let decimal = Cli::try_parse_from(["diag", "file", "17", "--out", "x.bin"]).unwrap().diag;
        assert!(matches!(decimal, Diag::Keyboard(KeyboardDiag::File { id: 17, out: Some(_) })));
    }

    #[test]
    fn hex_dump_prints_offset_rows_up_to_the_last_nonzero_row() {
        let mut bytes = vec![0u8; 64];
        bytes[0] = 0x13;
        bytes[17] = 0xAB;
        assert_eq!(
            hex_dump(&bytes),
            [
                "0000  13 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
                "0010  00 ab 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
                "(64 bytes total, 32 trailing zero bytes not shown)",
            ]
            .join("\n")
        );
        assert_eq!(hex_dump(&[0u8; 4]), "(4 bytes total, 4 trailing zero bytes not shown)");
        assert_eq!(hex_dump(&[1, 2]), "0000  01 02\n(2 bytes total)");
    }

    #[test]
    fn band_line_draws_one_character_per_band_by_level() {
        assert_eq!(band_line(&engine::Spectrum::SILENT), " ".repeat(32));
        assert_eq!(band_line(&engine::Spectrum::from_levels([1.0; engine::SPECTRUM_BANDS])), "@".repeat(32));
        assert_eq!(band_line(&engine::Spectrum::from_levels([0.5; engine::SPECTRUM_BANDS])), "+".repeat(32));
    }
}

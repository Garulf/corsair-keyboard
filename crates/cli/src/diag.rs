use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bragi::{FileId, Notification, OperatingMode, PollingRate, PropertyId};
use clap::Subcommand;
use k100::hid::{HidInput, HidTransport, OpenedDevice, open_device};
use k100::{
    Device, DeviceError, IndicatorRecord, InputSource, KeyMappings, KeyStateTracker, LED_COUNT, Mapping, Rgb, Session,
    led,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Subcommand)]
pub enum Diag {
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
        #[arg(default_value_t = 0x47)]
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
}

pub fn run(command: Diag) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))?;

    let OpenedDevice { commands, input, model } = open_device()?;
    println!(
        "found {} (pid {:04x}){}",
        model.name,
        model.pid,
        if model.tested { "" } else { " [untested]" }
    );
    match command {
        Diag::Info => info(commands),
        Diag::Release => release(commands, model.key_count),
        Diag::Lights { colors, hold_ms } => lights(Session::start(commands, model)?, &colors, hold_ms, &stop),
        Diag::Indicators => indicators(Session::start(commands, model)?),
        Diag::Fps { frames } => fps(Session::start(commands, model)?, frames, &stop),
        Diag::Props => props(commands),
        Diag::Mask { key, seconds } => mask(Session::start(commands, model)?, key, seconds, &stop),
        Diag::Watch { seconds } => watch(Session::start(commands, model)?, input, seconds, &stop),
        Diag::Kill => kill(Session::start(commands, model)?),
    }
}

fn info(commands: HidTransport) -> Result<()> {
    let mut device = Device::new(commands);
    let mut read = |property| device.get_property(property).map_err(describe);
    println!(
        "firmware:         {}",
        read(PropertyId::FIRMWARE_VERSION).map(format_version)?
    );
    println!("pid:              {:04x?}", read(PropertyId::USB_PID));
    println!("layout:           {:?}", read(PropertyId::PHYSICAL_KEYBOARD_LAYOUT));
    println!("protocol version: {:?}", read(PropertyId::BRAGI_PROTOCOL_VERSION));
    let mode = read(PropertyId::OPERATING_MODE).map(|raw| OperatingMode::from_raw(raw).ok_or(raw));
    println!("operating mode:   {mode:?}");
    println!("brightness:       {:?}", read(PropertyId::BRIGHTNESS));
    for (name, property) in [
        ("polling rate:", PropertyId::POLLING_RATE),
        ("max polling:", PropertyId::MAXIMUM_POLLING_RATE),
    ] {
        let rate = read(property).map(|raw| PollingRate::from_raw(raw).map(PollingRate::hz).ok_or(raw));
        println!("{name:<17} {rate:?}");
    }
    println!("winlock:          {:?}", read(PropertyId::WIN_LOCK_ENABLED));
    println!(
        "key reporting:    {:?}",
        read(PropertyId::KEY_PRESS_NOTIFICATIONS_REPORTING)
    );
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
    for handle in 0..=2 {
        let _ = device.unbind(handle);
    }
    device.open_file(2, FileId::KEY_MAPPINGS)?;
    device.write_file(2, KeyMappings::all_normal(key_count).as_bytes())?;
    device.unbind(2)?;
    device.set_property(PropertyId::OPERATING_MODE, OperatingMode::Hardware.raw())?;
    println!("released: hardware mode, all keys normal");
    Ok(())
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
            .map(|led| IndicatorRecord {
                led,
                color: Rgb::new(255, 0, 0),
                alpha,
            })
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
    println!(
        "{sent} frames in {elapsed:?}: {rate:.1} fps, {:?} per frame",
        elapsed / sent.max(1)
    );
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
    println!("key {key:#04x} suppressed for {seconds}s; press it now, it should do nothing");
    sleep_unless_stopped(Duration::from_secs(seconds), stop);
    Ok(session.release()?)
}

fn watch(mut session: Session<HidTransport>, mut input: HidInput, seconds: u64, stop: &AtomicBool) -> Result<()> {
    session.set_property(PropertyId::KEY_PRESS_NOTIFICATIONS_REPORTING, 1)?;
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
        let events = Notification::parse(&report)
            .map(|n| tracker.update(&n))
            .unwrap_or_default();
        println!("raw {:02x?} -> {events:?}", &report[..report.len().min(24)]);
    }
    session.set_property(PropertyId::KEY_PRESS_NOTIFICATIONS_REPORTING, 0)?;
    Ok(session.release()?)
}

fn kill(mut session: Session<HidTransport>) -> Result<()> {
    session.write_frame(&[Rgb::new(255, 0, 255); LED_COUNT])?;
    println!("software mode with magenta lights; exiting without restoring");
    std::mem::forget(session);
    std::process::exit(0);
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

pub fn format_version(raw: u32) -> String {
    let [major, minor, build_lo, build_hi] = raw.to_le_bytes();
    format!("{major}.{minor}.{}", u16::from_le_bytes([build_lo, build_hi]))
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
}

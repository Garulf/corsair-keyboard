use std::collections::BTreeMap;
use std::time::Duration;

use actions::RecorderState;
pub use ipc::DialRing;
use k100::{IndicatorRecord, Rgb, led};

pub const FLASH: Duration = Duration::from_secs(3);
const BLINK: Duration = Duration::from_millis(250);
const OVERRIDE: u8 = 0xFF;
const RED: Rgb = Rgb::new(255, 0, 0);
const WHITE: Rgb = Rgb::new(255, 255, 255);
const RING: std::ops::RangeInclusive<u8> = led::DIAL_RING_FIRST..=led::DIAL_RING_LAST;
const INDICATION_LEDS: [u8; 14] = [
    led::MACRO_RECORDING_INDICATOR,
    led::MUTE_INDICATOR,
    led::WIN_LOCK_INDICATOR,
    led::WIN_LOCK,
    led::PROFILE,
    led::DIAL_KEY,
    182,
    183,
    184,
    185,
    186,
    187,
    188,
    189,
];

pub struct Indicators {
    ring: DialRing,
    sent: BTreeMap<u8, (Rgb, u8)>,
    winlock: Option<bool>,
    winlock_flash_until: Option<Duration>,
    profile_flash_until: Option<Duration>,
    dial: Option<(Rgb, Option<Duration>)>,
    recording: RecorderState,
}

impl Indicators {
    pub fn new(ring: DialRing) -> Self {
        Self {
            ring,
            sent: BTreeMap::new(),
            winlock: None,
            winlock_flash_until: None,
            profile_flash_until: None,
            dial: None,
            recording: RecorderState::Idle,
        }
    }

    pub fn set_ring(&mut self, ring: DialRing) {
        self.ring = ring;
    }

    pub fn reset(&mut self) {
        self.sent.clear();
    }

    pub fn winlock(&mut self, enabled: bool, now: Duration, flash: bool) {
        self.winlock = Some(enabled);
        self.winlock_flash_until = (flash && !enabled).then_some(now + FLASH);
    }

    pub fn profile_switched(&mut self, now: Duration) {
        self.profile_flash_until = Some(now + FLASH);
    }

    pub fn dial_mode(&mut self, color: Rgb, now: Duration) {
        let until = match self.ring {
            DialRing::Temporary => Some(now + FLASH),
            DialRing::Continuous => None,
        };
        self.dial = Some((color, until));
    }

    pub fn recording(&mut self, state: RecorderState) {
        self.recording = state;
    }

    pub fn records(&mut self, now: Duration) -> Vec<IndicatorRecord> {
        let wanted = self.wanted(now);
        let mut changed = Vec::new();
        for led in INDICATION_LEDS {
            let value = wanted.get(&led).copied().unwrap_or((Rgb::BLACK, 0));
            if self.sent.get(&led) != Some(&value) {
                self.sent.insert(led, value);
                changed.push(IndicatorRecord { led, color: value.0, alpha: value.1 });
            }
        }
        changed
    }

    fn wanted(&self, now: Duration) -> BTreeMap<u8, (Rgb, u8)> {
        let mut wanted = BTreeMap::new();
        let mut light = |led: u8, color: Rgb| {
            wanted.insert(led, (color, OVERRIDE));
        };
        let active = |until: Option<Duration>| until.is_some_and(|until| now < until);
        match self.winlock {
            Some(true) => [led::WIN_LOCK_INDICATOR, led::WIN_LOCK].into_iter().for_each(|led| light(led, RED)),
            Some(false) if active(self.winlock_flash_until) => {
                [led::WIN_LOCK_INDICATOR, led::WIN_LOCK].into_iter().for_each(|led| light(led, WHITE))
            }
            _ => {}
        }
        if active(self.profile_flash_until) {
            light(led::PROFILE, WHITE);
        }
        if let Some((color, until)) = self.dial
            && until.is_none_or(|until| now < until)
        {
            light(led::DIAL_KEY, color);
            RING.for_each(|led| light(led, color));
        }
        let blink_on = (now.as_millis() / BLINK.as_millis()).is_multiple_of(2);
        match self.recording {
            RecorderState::Recording { .. } => light(led::MACRO_RECORDING_INDICATOR, WHITE),
            RecorderState::AwaitingKey if blink_on => light(led::MACRO_RECORDING_INDICATOR, WHITE),
            _ => {}
        }
        wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn record(led: u8, color: Rgb, alpha: u8) -> IndicatorRecord {
        IndicatorRecord { led, color, alpha }
    }

    #[test]
    fn the_first_frame_hands_every_indicator_to_the_firmware() {
        let mut indicators = Indicators::new(DialRing::Temporary);
        let first = indicators.records(ms(0));
        assert_eq!(first.len(), 14);
        assert!(first.iter().all(|r| r.alpha == 0));
        assert!(indicators.records(ms(10)).is_empty());
        indicators.reset();
        assert_eq!(indicators.records(ms(20)).len(), 14);
    }

    #[test]
    fn winlock_is_red_while_on_and_flashes_white_when_turned_off() {
        let mut indicators = Indicators::new(DialRing::Temporary);
        indicators.records(ms(0));
        indicators.winlock(true, ms(0), true);
        assert_eq!(indicators.records(ms(10)), [record(2, RED, 0xFF), record(led::WIN_LOCK, RED, 0xFF)]);
        indicators.winlock(false, ms(100), true);
        assert_eq!(indicators.records(ms(110)), [record(2, WHITE, 0xFF), record(led::WIN_LOCK, WHITE, 0xFF)]);
        assert!(indicators.records(ms(3099)).is_empty());
        assert_eq!(indicators.records(ms(3100)), [record(2, Rgb::BLACK, 0), record(led::WIN_LOCK, Rgb::BLACK, 0)]);
    }

    #[test]
    fn a_quiet_winlock_sync_does_not_flash() {
        let mut indicators = Indicators::new(DialRing::Temporary);
        indicators.records(ms(0));
        indicators.winlock(false, ms(0), false);
        assert!(indicators.records(ms(10)).is_empty());
    }

    #[test]
    fn profile_switches_flash_the_profile_key() {
        let mut indicators = Indicators::new(DialRing::Temporary);
        indicators.records(ms(0));
        indicators.profile_switched(ms(0));
        assert_eq!(indicators.records(ms(1)), [record(led::PROFILE, WHITE, 0xFF)]);
        assert_eq!(indicators.records(ms(3000)), [record(led::PROFILE, Rgb::BLACK, 0)]);
    }

    #[test]
    fn the_dial_ring_shows_the_mode_colour() {
        let blue = Rgb::new(0, 0xBF, 0xFF);
        let mut temporary = Indicators::new(DialRing::Temporary);
        temporary.records(ms(0));
        temporary.dial_mode(blue, ms(0));
        let shown = temporary.records(ms(1));
        assert_eq!(shown.len(), 9);
        assert!(shown.iter().all(|r| r.color == blue && r.alpha == 0xFF));
        assert!(temporary.records(ms(3000)).iter().all(|r| r.alpha == 0));
        let mut continuous = Indicators::new(DialRing::Continuous);
        continuous.records(ms(0));
        continuous.dial_mode(blue, ms(0));
        assert_eq!(continuous.records(ms(1)).len(), 9);
        assert!(continuous.records(ms(60_000)).is_empty());
    }

    #[test]
    fn recording_lights_the_macro_indicator() {
        let mut indicators = Indicators::new(DialRing::Temporary);
        indicators.records(ms(0));
        indicators.recording(RecorderState::AwaitingKey);
        assert_eq!(indicators.records(ms(0)), [record(0, WHITE, 0xFF)]);
        assert_eq!(indicators.records(ms(250)), [record(0, Rgb::BLACK, 0)]);
        assert_eq!(indicators.records(ms(500)), [record(0, WHITE, 0xFF)]);
        indicators.recording(RecorderState::Recording { key: 131 });
        assert!(indicators.records(ms(750)).is_empty(), "solid while recording");
        indicators.recording(RecorderState::Idle);
        assert_eq!(indicators.records(ms(760)), [record(0, Rgb::BLACK, 0)]);
    }
}

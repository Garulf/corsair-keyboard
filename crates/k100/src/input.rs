use bragi::{Notification, PropertyId};

const MAX_KEY_INDEX: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    KeyDown(u8),
    KeyUp(u8),
    DialCw,
    DialCcw,
    Property { property: PropertyId, value: u32 },
}

#[derive(Debug, Default)]
pub struct KeyStateTracker {
    pressed: Vec<u8>,
}

impl KeyStateTracker {
    pub fn update(&mut self, notification: &Notification) -> Vec<InputEvent> {
        match notification {
            Notification::KeyPressStateChange { bitmap } => self.diff(bitmap),
            Notification::RelativePropertyChange { property, delta } if *property == PropertyId::DIAL_RING_ROTATION => {
                let step = if *delta > 0 { InputEvent::DialCw } else { InputEvent::DialCcw };
                vec![step; delta.unsigned_abs() as usize]
            }
            Notification::PropertyValueChange { property, value } => {
                vec![InputEvent::Property { property: *property, value: *value }]
            }
            _ => Vec::new(),
        }
    }

    fn diff(&mut self, bitmap: &[u8]) -> Vec<InputEvent> {
        let width = bitmap.len().max(self.pressed.len());
        let mut events = Vec::new();
        for key in 0..(width * 8).min(MAX_KEY_INDEX + 1) {
            let was = bit(&self.pressed, key);
            let is = bit(bitmap, key);
            if was != is {
                let key = key as u8;
                events.push(if is { InputEvent::KeyDown(key) } else { InputEvent::KeyUp(key) });
            }
        }
        self.pressed = bitmap.to_vec();
        events
    }
}

fn bit(bytes: &[u8], index: usize) -> bool {
    bytes.get(index / 8).is_some_and(|byte| byte & (1 << (index % 8)) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bragi::{Notification, PropertyId};

    fn bitmap_with(keys: &[u8]) -> Notification {
        let mut bitmap = vec![0u8; 62];
        for &key in keys {
            bitmap[key as usize / 8] |= 1 << (key % 8);
        }
        Notification::KeyPressStateChange { bitmap }
    }

    #[test]
    fn reports_presses_and_releases_lsb_first() {
        let mut tracker = KeyStateTracker::default();
        assert_eq!(tracker.update(&bitmap_with(&[4, 131])), [InputEvent::KeyDown(4), InputEvent::KeyDown(131)]);
        assert_eq!(tracker.update(&bitmap_with(&[131, 137])), [InputEvent::KeyUp(4), InputEvent::KeyDown(137)]);
        assert_eq!(tracker.update(&bitmap_with(&[])), [InputEvent::KeyUp(131), InputEvent::KeyUp(137)]);
    }

    #[test]
    fn repeated_bitmap_is_silent() {
        let mut tracker = KeyStateTracker::default();
        tracker.update(&bitmap_with(&[10]));
        assert!(tracker.update(&bitmap_with(&[10])).is_empty());
    }

    #[test]
    fn dial_deltas_expand_to_steps() {
        let mut tracker = KeyStateTracker::default();
        let turn = |delta| Notification::RelativePropertyChange { property: PropertyId::DIAL_RING_ROTATION, delta };
        assert_eq!(tracker.update(&turn(2)), [InputEvent::DialCw, InputEvent::DialCw]);
        assert_eq!(tracker.update(&turn(-1)), [InputEvent::DialCcw]);
        let other = Notification::RelativePropertyChange { property: PropertyId::BRIGHTNESS, delta: 3 };
        assert!(tracker.update(&other).is_empty());
    }

    #[test]
    fn property_changes_pass_through() {
        let mut tracker = KeyStateTracker::default();
        let change = Notification::PropertyValueChange { property: PropertyId::WIN_LOCK_ENABLED, value: 1 };
        assert_eq!(
            tracker.update(&change),
            [InputEvent::Property { property: PropertyId::WIN_LOCK_ENABLED, value: 1 }]
        );
    }
}

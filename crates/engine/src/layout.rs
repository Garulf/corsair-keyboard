use k100::{LED_COUNT, led};

pub const CANVAS_WIDTH: f32 = 2500.0;
pub const CANVAS_HEIGHT: f32 = 916.0;

const UNIT: f32 = 100.0;
const MAIN_X: f32 = 150.0;
const MAIN_Y: f32 = 220.0;
const GAP: u8 = u8::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LedKind {
    Key,
    Indicator,
    LightBar,
    Underglow,
    DialRing,
    Logo,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedGeometry {
    pub index: u8,
    pub kind: LedKind,
    pub label: &'static str,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub struct Layout {
    leds: Vec<LedGeometry>,
    by_index: [Option<usize>; LED_COUNT],
    bounds: Rect,
}

type Cell = (u8, &'static str, f32, f32);

const fn k(index: u8, label: &'static str, width: f32) -> Cell {
    (index, label, width, 1.0)
}

const fn tall(index: u8, label: &'static str) -> Cell {
    (index, label, 1.0, 2.0)
}

const fn gap(width: f32) -> Cell {
    (GAP, "", width, 1.0)
}

#[rustfmt::skip]
const ROWS: [(f32, &[Cell]); 6] = [
    (0.0, &[
        k(41, "Esc", 1.0), gap(1.0),
        k(58, "F1", 1.0), k(59, "F2", 1.0), k(60, "F3", 1.0), k(61, "F4", 1.0), gap(0.5),
        k(62, "F5", 1.0), k(63, "F6", 1.0), k(64, "F7", 1.0), k(65, "F8", 1.0), gap(0.5),
        k(66, "F9", 1.0), k(67, "F10", 1.0), k(68, "F11", 1.0), k(69, "F12", 1.0), gap(0.25),
        k(70, "PrtSc", 1.0), k(71, "ScrLk", 1.0), k(72, "Pause", 1.0),
    ]),
    (1.25, &[
        k(53, "`", 1.0), k(30, "1", 1.0), k(31, "2", 1.0), k(32, "3", 1.0), k(33, "4", 1.0), k(34, "5", 1.0),
        k(35, "6", 1.0), k(36, "7", 1.0), k(37, "8", 1.0), k(38, "9", 1.0), k(39, "0", 1.0), k(45, "-", 1.0),
        k(46, "=", 1.0), k(42, "Backspace", 2.0), gap(0.25),
        k(73, "Ins", 1.0), k(74, "Home", 1.0), k(75, "PgUp", 1.0), gap(0.25),
        k(83, "NumLk", 1.0), k(84, "/", 1.0), k(85, "*", 1.0), k(86, "-", 1.0),
    ]),
    (2.25, &[
        k(43, "Tab", 1.5), k(20, "Q", 1.0), k(26, "W", 1.0), k(8, "E", 1.0), k(21, "R", 1.0), k(23, "T", 1.0),
        k(28, "Y", 1.0), k(24, "U", 1.0), k(12, "I", 1.0), k(18, "O", 1.0), k(19, "P", 1.0), k(47, "[", 1.0),
        k(48, "]", 1.0), k(49, "\\", 1.5), gap(0.25),
        k(76, "Del", 1.0), k(77, "End", 1.0), k(78, "PgDn", 1.0), gap(0.25),
        k(95, "7", 1.0), k(96, "8", 1.0), k(97, "9", 1.0), tall(87, "+"),
    ]),
    (3.25, &[
        k(57, "Caps", 1.75), k(4, "A", 1.0), k(22, "S", 1.0), k(7, "D", 1.0), k(9, "F", 1.0), k(10, "G", 1.0),
        k(11, "H", 1.0), k(13, "J", 1.0), k(14, "K", 1.0), k(15, "L", 1.0), k(51, ";", 1.0), k(52, "'", 1.0),
        k(40, "Enter", 2.25), gap(3.5),
        k(92, "4", 1.0), k(93, "5", 1.0), k(94, "6", 1.0),
    ]),
    (4.25, &[
        k(106, "Shift", 2.25), k(29, "Z", 1.0), k(27, "X", 1.0), k(6, "C", 1.0), k(25, "V", 1.0), k(5, "B", 1.0),
        k(17, "N", 1.0), k(16, "M", 1.0), k(54, ",", 1.0), k(55, ".", 1.0), k(56, "/", 1.0),
        k(110, "Shift", 2.75), gap(1.25), k(82, "Up", 1.0), gap(1.25),
        k(89, "1", 1.0), k(90, "2", 1.0), k(91, "3", 1.0), tall(88, "Enter"),
    ]),
    (5.25, &[
        k(105, "Ctrl", 1.25), k(108, "Win", 1.25), k(107, "Alt", 1.25), k(44, "Space", 6.25),
        k(111, "Alt", 1.25), k(led::FN, "Fn", 1.25), k(101, "Menu", 1.25), k(109, "Ctrl", 1.25), gap(0.25),
        k(80, "Left", 1.0), k(81, "Down", 1.0), k(79, "Right", 1.0), gap(0.25),
        k(98, "0", 2.0), k(99, ".", 1.0),
    ]),
];

impl Layout {
    pub fn ansi() -> Self {
        let mut leds = Vec::new();
        for (row_y, cells) in ROWS {
            let mut x = 0.0;
            for &(index, label, width, height) in cells {
                if index != GAP {
                    leds.push(key(
                        index,
                        label,
                        MAIN_X + x * UNIT,
                        MAIN_Y + row_y * UNIT,
                        width * UNIT,
                        height * UNIT,
                    ));
                }
                x += width;
            }
        }
        leds.extend(top_row());
        leds.extend(g_keys());
        leds.extend(dial());
        leds.extend(strip(
            LedKind::LightBar,
            led::LIGHT_BAR_FIRST..=led::LIGHT_BAR_LAST,
            |i, n| Rect {
                x: 250.0 + i * (2150.0 / n),
                y: 10.0,
                w: 2150.0 / n - 10.0,
                h: 20.0,
            },
        ));
        leds.extend(strip(
            LedKind::Underglow,
            led::LEFT_UNDERGLOW_FIRST..=led::LEFT_UNDERGLOW_LAST,
            |i, n| Rect {
                x: 0.0,
                y: 230.0 + i * (610.0 / n),
                w: 10.0,
                h: 610.0 / n - 5.0,
            },
        ));
        leds.extend(strip(
            LedKind::Underglow,
            led::RIGHT_UNDERGLOW_FIRST..=led::RIGHT_UNDERGLOW_LAST,
            |i, n| Rect {
                x: 2490.0,
                y: 230.0 + i * (610.0 / n),
                w: 10.0,
                h: 610.0 / n - 5.0,
            },
        ));
        leds.extend(strip(LedKind::Logo, led::LOGO_FIRST..=led::LOGO_LAST, |i, _| Rect {
            x: 1170.0 + i * 55.0,
            y: 50.0,
            w: 45.0,
            h: 45.0,
        }));
        Self::from_leds(leds)
    }

    fn from_leds(leds: Vec<LedGeometry>) -> Self {
        let mut by_index = [None; LED_COUNT];
        for (position, geometry) in leds.iter().enumerate() {
            by_index[geometry.index as usize] = Some(position);
        }
        let bounds = leds.iter().map(|g| g.rect).reduce(union).unwrap_or(Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        });
        Self { leds, by_index, bounds }
    }

    pub fn leds(&self) -> &[LedGeometry] {
        &self.leds
    }

    pub fn get(&self, index: u8) -> Option<&LedGeometry> {
        self.by_index
            .get(index as usize)
            .copied()
            .flatten()
            .map(|position| &self.leds[position])
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn normalized(&self, index: u8) -> Option<(f32, f32)> {
        let (x, y) = self.get(index)?.rect.center();
        Some(((x - self.bounds.x) / self.bounds.w, (y - self.bounds.y) / self.bounds.h))
    }
}

fn key(index: u8, label: &'static str, x: f32, y: f32, w: f32, h: f32) -> LedGeometry {
    LedGeometry {
        index,
        kind: LedKind::Key,
        label,
        rect: Rect { x, y, w, h },
    }
}

fn top_row() -> Vec<LedGeometry> {
    let y = MAIN_Y - UNIT;
    let button = |index, label, x_units: f32| key(index, label, MAIN_X + x_units * UNIT, y, 0.8 * UNIT, 0.8 * UNIT);
    let mut leds = vec![
        button(led::MACRO_RECORD, "MR", 0.0),
        button(led::PROFILE, "Profile", 1.0),
        button(led::BRIGHTNESS, "Light", 13.25),
        button(led::WIN_LOCK, "Lock", 14.25),
        button(led::MUTE, "Mute", 17.25),
        button(led::MEDIA_STOP, "Stop", 18.5),
        button(led::MEDIA_PREVIOUS, "Prev", 19.5),
        button(led::MEDIA_PLAY, "Play", 20.5),
        button(led::MEDIA_NEXT, "Next", 21.5),
    ];
    let indicator = |index, label, x| LedGeometry {
        index,
        kind: LedKind::Indicator,
        label,
        rect: Rect {
            x,
            y: y - 25.0,
            w: 20.0,
            h: 10.0,
        },
    };
    leds.push(indicator(led::MACRO_RECORDING_INDICATOR, "Recording", MAIN_X + 30.0));
    leds.push(indicator(led::MUTE_INDICATOR, "Muted", MAIN_X + 17.25 * UNIT + 30.0));
    leds
}

fn g_keys() -> Vec<LedGeometry> {
    (led::G1..=led::G6)
        .zip(["G1", "G2", "G3", "G4", "G5", "G6"])
        .enumerate()
        .map(|(i, (index, label))| {
            let y = MAIN_Y + (1.25 + i as f32 * 5.0 / 6.0) * UNIT;
            key(index, label, 20.0, y, UNIT, 0.75 * UNIT)
        })
        .collect()
}

fn dial() -> Vec<LedGeometry> {
    let (cx, cy, radius) = (70.0, 160.0, 55.0);
    let mut leds = vec![key(led::DIAL_KEY, "Dial", cx - 30.0, cy - 30.0, 60.0, 60.0)];
    for (i, index) in (led::DIAL_RING_FIRST..=led::DIAL_RING_LAST).enumerate() {
        let angle = std::f32::consts::TAU * i as f32 / 8.0 - std::f32::consts::FRAC_PI_2;
        leds.push(LedGeometry {
            index,
            kind: LedKind::DialRing,
            label: "Ring",
            rect: Rect {
                x: cx + radius * angle.cos() - 8.0,
                y: cy + radius * angle.sin() - 8.0,
                w: 16.0,
                h: 16.0,
            },
        });
    }
    leds
}

fn strip(kind: LedKind, indices: std::ops::RangeInclusive<u8>, place: impl Fn(f32, f32) -> Rect) -> Vec<LedGeometry> {
    let count = indices.clone().count() as f32;
    indices
        .enumerate()
        .map(|(i, index)| LedGeometry {
            index,
            kind,
            label: "",
            rect: place(i as f32, count),
        })
        .collect()
}

fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        w: (a.x + a.w).max(b.x + b.w) - x,
        h: (a.y + a.h).max(b.y + b.h) - y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k100::led;
    use std::collections::HashSet;

    #[test]
    fn indices_are_unique_and_inside_the_canvas() {
        let layout = Layout::ansi();
        let mut seen = HashSet::new();
        for geometry in layout.leds() {
            assert!(seen.insert(geometry.index), "duplicate {}", geometry.index);
            let r = geometry.rect;
            assert!(
                r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= CANVAS_WIDTH && r.y + r.h <= CANVAS_HEIGHT,
                "{geometry:?}"
            );
            assert!((geometry.index as usize) < k100::LED_COUNT);
        }
        assert_eq!(layout.leds().len(), 177);
    }

    #[test]
    fn has_every_zone_led() {
        let layout = Layout::ansi();
        for index in (led::G1..=led::G6).chain(led::LIGHT_BAR_FIRST..=led::LOGO_LAST) {
            assert!(layout.get(index).is_some(), "missing {index}");
        }
        assert_eq!(layout.get(led::DIAL_RING_FIRST).unwrap().kind, LedKind::DialRing);
        assert_eq!(
            layout.get(led::MACRO_RECORDING_INDICATOR).unwrap().kind,
            LedKind::Indicator
        );
        assert!(layout.get(100).is_none(), "ISO key on an ANSI layout");
    }

    #[test]
    fn keys_sit_where_a_typist_expects() {
        let layout = Layout::ansi();
        let x = |i| layout.get(i).unwrap().rect.center().0;
        let y = |i| layout.get(i).unwrap().rect.center().1;
        assert!(x(4) < x(22) && x(22) < x(7), "A S D left to right");
        assert!(y(20) < y(4) && y(4) < y(29), "Q above A above Z");
        assert!(x(83) > x(42), "numpad right of backspace");
        assert!(x(131) < x(41), "G-keys left of Esc");
        assert_eq!(layout.get(44).unwrap().rect.w, 625.0, "6.25u space bar");
    }

    #[test]
    fn normalized_positions_span_unit_square() {
        let layout = Layout::ansi();
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        for geometry in layout.leds() {
            let (x, y) = layout.normalized(geometry.index).unwrap();
            assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
            min_x = min_x.min(x);
            max_x = max_x.max(x);
        }
        assert!(min_x < 0.05 && max_x > 0.95);
        assert_eq!(layout.normalized(250), None);
    }
}

use k100::Rgb;

pub fn hsv(hue_degrees: f32, saturation: f32, value: f32) -> Rgb {
    let hue = if hue_degrees.is_finite() { hue_degrees.rem_euclid(360.0) / 60.0 } else { 0.0 };
    let chroma = value * saturation;
    let x = chroma * (1.0 - ((hue % 2.0) - 1.0).abs());
    let m = value - chroma;
    let (r, g, b) = match hue as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    Rgb::new(channel(r + m), channel(g + m), channel(b + m))
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = unit(t);
    let mix = |x: u8, y: u8| channel((f32::from(x) + (f32::from(y) - f32::from(x)) * t) / 255.0);
    Rgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

pub fn scale(color: Rgb, factor: f32) -> Rgb {
    lerp(Rgb::BLACK, color, factor)
}

pub(crate) fn unit(value: f32) -> f32 {
    if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) }
}

fn channel(value: f32) -> u8 {
    (unit(value) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries_and_wraparound() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(hsv(120.0, 1.0, 1.0), Rgb::new(0, 255, 0));
        assert_eq!(hsv(240.0, 1.0, 1.0), Rgb::new(0, 0, 255));
        assert_eq!(hsv(-120.0, 1.0, 1.0), hsv(240.0, 1.0, 1.0));
        assert_eq!(hsv(720.0, 1.0, 0.0), Rgb::BLACK);
        assert_eq!(hsv(f32::NAN, 1.0, 1.0), Rgb::new(255, 0, 0));
    }

    #[test]
    fn lerp_and_scale_clamp() {
        let a = Rgb::new(0, 100, 200);
        let b = Rgb::new(200, 100, 0);
        assert_eq!(lerp(a, b, 0.5), Rgb::new(100, 100, 100));
        assert_eq!(lerp(a, b, -1.0), a);
        assert_eq!(lerp(a, b, 9.0), b);
        assert_eq!(scale(Rgb::new(200, 100, 50), 0.5), Rgb::new(100, 50, 25));
        assert_eq!(scale(Rgb::new(200, 100, 50), 3.0), Rgb::new(200, 100, 50));
    }
}

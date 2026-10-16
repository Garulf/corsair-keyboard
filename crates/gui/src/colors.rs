use eframe::egui::{self, Color32};
use k100::Rgb;

pub fn to_color32(color: Rgb) -> Color32 {
    Color32::from_rgb(color.r, color.g, color.b)
}

pub fn to_array(color: Rgb) -> [u8; 3] {
    [color.r, color.g, color.b]
}

pub fn from_array(value: [u8; 3]) -> Rgb {
    Rgb::new(value[0], value[1], value[2])
}

pub fn contrast(background: Color32) -> Color32 {
    let luma =
        0.299 * f32::from(background.r()) + 0.587 * f32::from(background.g()) + 0.114 * f32::from(background.b());
    if luma > 140.0 { Color32::BLACK } else { Color32::WHITE }
}

pub fn color_button(ui: &mut egui::Ui, color: &mut Rgb) -> bool {
    let mut value = to_array(*color);
    let changed = ui.color_edit_button_srgb(&mut value).changed();
    if changed {
        *color = from_array(value);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_round_trip_between_k100_and_egui() {
        let color = Rgb::new(10, 20, 30);
        assert_eq!(to_color32(color), Color32::from_rgb(10, 20, 30));
        assert_eq!(from_array(to_array(color)), color);
    }

    #[test]
    fn text_contrasts_with_its_background() {
        assert_eq!(contrast(Color32::from_rgb(250, 250, 250)), Color32::BLACK);
        assert_eq!(contrast(Color32::from_rgb(10, 10, 40)), Color32::WHITE);
    }
}

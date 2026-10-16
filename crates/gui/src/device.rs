use eframe::egui;
use ipc::{Debounce, DeviceState, DialRing, MAX_DEBOUNCE_TIMEOUT, Request, Settings, SettingsPatch, WinLockShortcut};

use crate::model::Model;

const RATES: [u32; 7] = [125, 250, 500, 1000, 2000, 4000, 8000];

pub const DEFAULT_DEBOUNCE: Debounce = Debounce { enabled: true, timeout: 30 };

#[derive(Debug, Default)]
pub struct DeviceForm {
    pub brightness: Option<u8>,
    pub debounce: Option<Debounce>,
}

pub fn polling_choices(max_hz: Option<u32>) -> Vec<u32> {
    RATES.into_iter().filter(|hz| max_hz.is_none_or(|max| *hz <= max)).collect()
}

pub fn slider_commit(changed: bool, dragged: bool, drag_stopped: bool) -> bool {
    drag_stopped || (changed && !dragged)
}

pub fn toggled(current: &[WinLockShortcut], shortcut: WinLockShortcut, on: bool) -> Vec<WinLockShortcut> {
    WinLockShortcut::ALL
        .into_iter()
        .filter(|candidate| if *candidate == shortcut { on } else { current.contains(candidate) })
        .collect()
}

pub fn commit_brightness(model: &mut Model, percent: u8) {
    let percent = percent.min(100);
    if let Some(state) = model.state.as_mut() {
        state.brightness = percent;
    }
    model.device_form.brightness = None;
    model.send(Request::SetBrightness { percent });
}

fn send_patch(model: &mut Model, patch: SettingsPatch) {
    model.send(Request::SetSettings { patch });
}

pub fn device(ui: &mut egui::Ui, model: &mut Model) {
    let Some(state) = model.state.clone() else { return };
    let settings = model.settings.clone().unwrap_or_default();
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Keyboard");
        brightness(ui, model, state.brightness);
        polling(ui, model, state.polling_rate_hz, state.max_polling_rate_hz);
        debounce(ui, model, settings.debounce);
        hardware_mode(ui, model, &state.device);
        ui.separator();
        ui.heading("WinLock");
        winlock(ui, model, state.winlock, settings.winlock_disables.as_deref().unwrap_or(&[]));
        ui.separator();
        ui.heading("Behaviour");
        behaviour(ui, model, &settings, state.autostart);
    });
}

fn brightness(ui: &mut egui::Ui, model: &mut Model, current: u8) {
    let mut value = model.device_form.brightness.unwrap_or(current);
    let response = ui.add(egui::Slider::new(&mut value, 0..=100).text("Brightness (%)"));
    if response.changed() {
        model.device_form.brightness = Some(value);
    }
    if slider_commit(response.changed(), response.dragged(), response.drag_stopped()) {
        commit_brightness(model, value);
    }
}

fn polling(ui: &mut egui::Ui, model: &mut Model, current: Option<u32>, max: Option<u32>) {
    let shown = current.map_or("Keyboard default".to_owned(), |hz| format!("{hz} Hz"));
    egui::ComboBox::from_label("Polling rate").selected_text(shown).show_ui(ui, |ui| {
        for hz in polling_choices(max) {
            if ui.selectable_label(current == Some(hz), format!("{hz} Hz")).clicked() {
                model.send(Request::SetPollingRate { hz });
            }
        }
    });
}

fn debounce(ui: &mut egui::Ui, model: &mut Model, saved: Option<Debounce>) {
    let mut form = model.device_form.debounce.or(saved).unwrap_or(DEFAULT_DEBOUNCE);
    let mut changed = ui.checkbox(&mut form.enabled, "Key debounce").changed();
    ui.horizontal(|ui| {
        let value = egui::DragValue::new(&mut form.timeout).range(0..=MAX_DEBOUNCE_TIMEOUT);
        changed |= ui.add_enabled(form.enabled, value).changed();
        ui.label("firmware units (the keyboard's own default is 30)");
    });
    if changed {
        model.device_form.debounce = Some(form);
    }
    ui.horizontal(|ui| {
        if ui.button("Apply debounce").clicked() {
            model.send(Request::SetDebounce { enabled: form.enabled, timeout: form.timeout });
            model.device_form.debounce = None;
        }
        if saved.is_none() {
            ui.weak("Not changed: the keyboard uses its own setting.");
        }
    });
}

fn hardware_mode(ui: &mut egui::Ui, model: &mut Model, device: &DeviceState) {
    if *device == DeviceState::Released {
        if ui.button("Take the keyboard back").clicked() {
            model.send(Request::Reacquire);
        }
        ui.weak("The keyboard shows its own lighting. Nothing from k100d reaches it until you take it back.");
    } else if ui.button("Release to hardware mode").clicked() {
        model.send(Request::Release);
    }
}

fn winlock(ui: &mut egui::Ui, model: &mut Model, enabled: Option<bool>, shortcuts: &[WinLockShortcut]) {
    let mut on = enabled.unwrap_or(false);
    if ui.add_enabled(enabled.is_some(), egui::Checkbox::new(&mut on, "WinLock is on")).changed() {
        model.send(Request::SetWinLock { enabled: on });
    }
    ui.label("While WinLock is on, also block:");
    for shortcut in WinLockShortcut::ALL {
        let mut blocked = shortcuts.contains(&shortcut);
        if ui.checkbox(&mut blocked, shortcut.label()).changed() {
            let list = toggled(shortcuts, shortcut, blocked);
            send_patch(model, SettingsPatch { winlock_disables: Some(list), ..SettingsPatch::default() });
        }
    }
}

fn behaviour(ui: &mut egui::Ui, model: &mut Model, settings: &Settings, autostart: bool) {
    let mut ring = settings.dial_ring;
    ui.horizontal(|ui| {
        ui.label("Dial ring");
        ui.radio_value(&mut ring, DialRing::Temporary, "Show the mode colour briefly");
        ui.radio_value(&mut ring, DialRing::Continuous, "Keep the mode colour lit");
    });
    if ring != settings.dial_ring {
        send_patch(model, SettingsPatch { dial_ring: Some(ring), ..SettingsPatch::default() });
    }
    let mut switching = settings.auto_switch;
    if ui.checkbox(&mut switching, "Switch profiles by the program in front").changed() {
        send_patch(model, SettingsPatch { auto_switch: Some(switching), ..SettingsPatch::default() });
    }
    if !cfg!(windows) {
        ui.weak("Per-program switching works on Windows only.");
    }
    let mut login = autostart;
    if ui.checkbox(&mut login, "Start k100d when I log in").changed() {
        model.send(Request::SetAutostart { enabled: login });
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use egui_kittest::kittest::{NodeT, Queryable};
    use ipc::Settings;

    use super::*;
    use crate::model::Inbox;
    use crate::testing::{copy, harness, online, sent};
    use ipc::Reply;

    fn patch(change: impl FnOnce(&mut SettingsPatch)) -> Request {
        let mut patch = SettingsPatch::default();
        change(&mut patch);
        Request::SetSettings { patch }
    }

    #[test]
    fn polling_choices_stop_at_the_keyboards_maximum() {
        assert_eq!(polling_choices(Some(1000)), [125, 250, 500, 1000]);
        assert_eq!(polling_choices(None).len(), 7);
        assert_eq!(polling_choices(Some(8000)).last(), Some(&8000));
        assert_eq!(polling_choices(Some(100)), Vec::<u32>::new());
    }

    #[test]
    fn sliders_commit_on_release_or_a_non_drag_change_only() {
        assert!(!slider_commit(true, true, false), "mid-drag");
        assert!(slider_commit(false, false, true), "release");
        assert!(slider_commit(true, false, false), "keyboard or click");
        assert!(!slider_commit(false, false, false), "idle");
    }

    #[test]
    fn toggling_a_shortcut_keeps_the_canonical_order() {
        use WinLockShortcut::*;
        assert_eq!(toggled(&[AltF4], Win, true), [Win, AltF4]);
        assert_eq!(toggled(&[Win, AltF4], Win, false), [AltF4]);
        assert_eq!(toggled(&[], ShiftTab, true), [ShiftTab]);
    }

    #[test]
    fn committing_brightness_clamps_and_updates_the_shown_value() {
        let mut model = online("p", &["p"]);
        model.device_form.brightness = Some(33);
        commit_brightness(&mut model, 250);
        assert_eq!(model.state.as_ref().unwrap().brightness, 100);
        assert_eq!(model.device_form.brightness, None);
        assert_eq!(sent(&mut model), [Request::SetBrightness { percent: 100 }]);
    }

    #[test]
    fn winlock_is_a_switch_that_needs_a_keyboard() {
        let mut model = online("p", &["p"]);
        let mut unknown = harness(copy(&model), device);
        unknown.run();
        assert!(unknown.get_by_label("WinLock is on").accesskit_node().is_disabled());

        model.state.as_mut().unwrap().winlock = Some(false);
        let mut known = harness(model, device);
        known.run();
        known.get_by_label("WinLock is on").click();
        known.run();
        assert_eq!(sent(known.state_mut()), [Request::SetWinLock { enabled: true }]);
    }

    #[test]
    fn shortcut_checkboxes_send_the_whole_list() {
        let mut model = online("p", &["p"]);
        model.settings = Some(Settings { winlock_disables: Some(vec![WinLockShortcut::Win]), ..Settings::default() });
        let mut harness = harness(model, device);
        harness.run();
        harness.get_by_label("Alt+F4").click();
        harness.run();
        let expected = patch(|p| p.winlock_disables = Some(vec![WinLockShortcut::Win, WinLockShortcut::AltF4]));
        assert_eq!(sent(harness.state_mut()), [expected]);
    }

    #[test]
    fn dial_ring_auto_switch_and_autostart_send_their_requests() {
        let mut harness = harness(online("p", &["p"]), device);
        harness.run();
        harness.get_by_label("Keep the mode colour lit").click();
        harness.run();
        harness.get_by_label("Switch profiles by the program in front").click();
        harness.run();
        harness.get_by_label("Start k100d when I log in").click();
        harness.run();
        assert_eq!(
            sent(harness.state_mut()),
            [
                patch(|p| p.dial_ring = Some(DialRing::Continuous)),
                patch(|p| p.auto_switch = Some(false)),
                Request::SetAutostart { enabled: true },
            ]
        );
    }

    #[test]
    fn debounce_is_applied_with_its_button() {
        let mut model = online("p", &["p"]);
        model.device_form.debounce = Some(Debounce { enabled: false, timeout: 45 });
        let mut harness = harness(model, device);
        harness.run();
        harness.get_by_label("Apply debounce").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::SetDebounce { enabled: false, timeout: 45 }]);
        assert_eq!(harness.state().device_form.debounce, None);
    }

    #[test]
    fn an_untouched_debounce_offers_the_keyboards_own_default() {
        let mut harness = harness(online("p", &["p"]), device);
        harness.run();
        harness.get_by_label_contains("uses its own setting");
        harness.get_by_label("Apply debounce").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::SetDebounce { enabled: true, timeout: 30 }]);
    }

    #[test]
    fn release_and_take_back_swap_places() {
        let mut model = online("p", &["p"]);
        let mut first = harness(copy(&model), device);
        first.run();
        first.get_by_label("Release to hardware mode").click();
        first.run();
        assert_eq!(sent(first.state_mut()), [Request::Release]);

        model.state.as_mut().unwrap().device = ipc::DeviceState::Released;
        let mut second = harness(model, device);
        second.run();
        assert!(second.query_by_label("Release to hardware mode").is_none());
        second.get_by_label("Take the keyboard back").click();
        second.run();
        assert_eq!(sent(second.state_mut()), [Request::Reacquire]);
    }

    fn slider_rect(harness: &egui_kittest::Harness<'_, Model>) -> egui::Rect {
        harness.get_all_by_label("Brightness (%)").next().unwrap().rect()
    }

    #[test]
    fn dragging_the_brightness_slider_sends_one_request_on_release() {
        let mut harness = harness(online("p", &["p"]), device);
        harness.run();
        let rect = slider_rect(&harness);
        let start = rect.left_center() + egui::vec2(20.0, 0.0);
        harness.hover_at(start);
        harness.drag_at(start);
        harness.run();
        for step in 1..=5 {
            harness.hover_at(start + egui::vec2(step as f32 * 10.0, 0.0));
            harness.run();
        }
        assert_eq!(sent(harness.state_mut()), [], "nothing is sent while dragging");
        assert!(harness.state().device_form.brightness.is_some());
        harness.drop_at(start + egui::vec2(50.0, 0.0));
        harness.run();
        let requests = sent(harness.state_mut());
        assert!(matches!(requests.as_slice(), [Request::SetBrightness { .. }]), "{requests:?}");
        assert_eq!(harness.state().device_form.brightness, None);
    }

    #[test]
    fn clicking_the_brightness_slider_sends_one_request() {
        let mut harness = harness(online("p", &["p"]), device);
        harness.run();
        let at = slider_rect(&harness).left_center() + egui::vec2(20.0, 0.0);
        harness.hover_at(at);
        harness.drag_at(at);
        harness.run();
        harness.drop_at(at);
        harness.run();
        let requests = sent(harness.state_mut());
        assert!(matches!(requests.as_slice(), [Request::SetBrightness { .. }]), "{requests:?}");
    }

    #[test]
    fn picking_a_polling_rate_offers_only_what_the_keyboard_supports() {
        let mut model = online("p", &["p"]);
        let state = model.state.as_mut().unwrap();
        state.polling_rate_hz = Some(1000);
        state.max_polling_rate_hz = Some(1000);
        let mut harness = harness(model, device);
        harness.run();
        harness.get_by_label("Polling rate").click();
        harness.run();
        assert!(harness.query_by_label("2000 Hz").is_none());
        harness.get_by_label("500 Hz").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::SetPollingRate { hz: 500 }]);
    }

    #[test]
    fn the_pane_never_touches_the_profile_draft() {
        let mut model = online("p", &["p"]);
        model.settings = Some(Settings { winlock_disables: Some(vec![]), ..Settings::default() });
        let before = model.draft.clone();
        let mut harness = harness(model, device);
        harness.run();
        harness.get_by_label("Alt+F4").click();
        harness.get_by_label("Apply debounce").click();
        harness.run();
        assert_eq!(harness.state().draft, before);
    }

    #[test]
    fn the_pane_shows_nothing_before_the_daemon_has_answered() {
        let mut harness = harness(Model::new(), device);
        harness.run();
        assert!(harness.query_by_label("Apply debounce").is_none());
    }

    #[test]
    fn a_refused_settings_write_reaches_the_user() {
        let refusal = || Reply::Error { message: "settings.toml does not parse".into() };
        for request in
            [Request::SetDebounce { enabled: true, timeout: 30 }, Request::SetPollingRate { hz: 500 }, patch_request()]
        {
            let mut model = online("p", &["p"]);
            model.apply(Inbox::reply(request, refusal()));
            assert_eq!(model.toasts, ["settings.toml does not parse"]);
        }
    }

    fn patch_request() -> Request {
        patch(|p| p.auto_switch = Some(false))
    }
}

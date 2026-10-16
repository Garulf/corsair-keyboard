use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};
use ipc::Client;
use ipc::launcher::{LaunchError, Launched};

use crate::io::{Backend, Connector, RETRY_INTERVAL};
use crate::model::{Connection, Effect, Inbox, Model, Pane, device_text};

const REPAINT_EVERY: Duration = Duration::from_millis(100);

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("K100"),
        ..Default::default()
    };
    eframe::run_native(
        "K100",
        options,
        Box::new(|cc| {
            let context = cc.egui_ctx.clone();
            let connector: Connector = Arc::new(Client::connect);
            let backend = Backend::spawn(connector, Arc::new(move || context.request_repaint()), RETRY_INTERVAL);
            Ok(Box::new(App::new(backend)))
        }),
    )
    .map_err(|failure| failure.to_string())
}

pub struct App {
    model: Model,
    backend: Backend,
    started: Instant,
}

impl App {
    pub fn new(backend: Backend) -> Self {
        Self { model: Model::new(), backend, started: Instant::now() }
    }

    fn pump(&mut self) {
        for message in self.backend.drain() {
            self.model.apply(message);
        }
        self.model.tick(self.started.elapsed());
        self.flush();
    }

    fn flush(&mut self) {
        for effect in self.model.take_effects() {
            match effect {
                Effect::Send(request) => self.backend.send(request),
                Effect::StartDaemon => start_daemon(self.backend.injector(), || ipc::launcher::ensure_daemon(&[])),
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        root(ui, &mut self.model);
        self.flush();
        ui.ctx().request_repaint_after(REPAINT_EVERY);
    }
}

fn start_daemon(inbox: Sender<Inbox>, launch: impl FnOnce() -> Result<Launched, LaunchError> + Send + 'static) {
    thread::spawn(move || {
        let outcome = match launch() {
            Ok(_) => Inbox::LaunchFinished,
            Err(failure) => Inbox::LaunchFailed(failure.to_string()),
        };
        let _ = inbox.send(outcome);
    });
}

pub fn root(ui: &mut egui::Ui, model: &mut Model) {
    egui::Panel::bottom("status").show(ui, |ui| status_bar(ui, model));
    if model.connection != Connection::Online {
        egui::CentralPanel::default().show(ui, |ui| offline(ui, model));
        return;
    }
    egui::Panel::left("nav").exact_size(150.0).show(ui, |ui| nav(ui, model));
    egui::CentralPanel::default().show(ui, |ui| {
        stale_banner(ui, model);
        if model.pane.edits_profile() {
            draft_bar(ui, model);
            ui.separator();
        }
        match model.pane {
            Pane::Overview => overview(ui, model),
            Pane::Lighting => crate::layers::lighting(ui, model),
            Pane::Keys => crate::assignments::keys(ui, model),
            Pane::Dial => crate::dial::dial(ui, model),
            Pane::Device => crate::device::device(ui, model),
            Pane::Profiles => crate::profiles::profiles(ui, model),
        }
    });
}

fn nav(ui: &mut egui::Ui, model: &mut Model) {
    ui.add_space(6.0);
    for pane in Pane::ALL {
        ui.selectable_value(&mut model.pane, *pane, pane.label());
    }
}

fn offline(ui: &mut egui::Ui, model: &mut Model) {
    ui.vertical_centered(|ui| {
        ui.add_space(80.0);
        match model.connection.clone() {
            Connection::Offline(reason) => {
                ui.heading("k100d is not running");
                ui.label(reason);
                ui.add_space(12.0);
                let label = if model.starting_daemon { "Starting k100d..." } else { "Start k100d" };
                if ui.add_enabled(!model.starting_daemon, egui::Button::new(label)).clicked() {
                    model.request_start_daemon();
                }
                ui.add_space(6.0);
                ui.weak("This window reconnects by itself as soon as the daemon answers.");
            }
            _ => {
                ui.spinner();
                ui.label("Connecting to k100d...");
            }
        }
    });
}

fn status_bar(ui: &mut egui::Ui, model: &mut Model) {
    ui.horizontal(|ui| {
        match &model.connection {
            Connection::Online => ui.colored_label(Color32::LIGHT_GREEN, "Connected"),
            Connection::Connecting => ui.colored_label(Color32::YELLOW, "Connecting"),
            Connection::Offline(_) => ui.colored_label(Color32::LIGHT_RED, "Offline"),
        };
        if let Some(state) = &model.state {
            ui.separator();
            ui.label(device_text(&state.device));
        }
    });
    let mut dismissed = None;
    for (index, toast) in model.toasts.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.colored_label(Color32::LIGHT_RED, toast);
            if ui.small_button("Dismiss").clicked() {
                dismissed = Some(index);
            }
        });
    }
    if let Some(index) = dismissed {
        model.toasts.remove(index);
    }
}

pub fn stale_banner(ui: &mut egui::Ui, model: &mut Model) {
    let Some(stale) = model.stale.clone() else { return };
    ui.horizontal(|ui| {
        ui.colored_label(Color32::YELLOW, stale.message);
        let keep = if stale.drops_macro { "Keep my edits (drop the macro)" } else { "Keep my edits" };
        if ui.button(keep).clicked() {
            model.keep_edits();
        }
        if ui.button("Reload").clicked() {
            model.discard_draft();
        }
    });
}

pub fn draft_bar(ui: &mut egui::Ui, model: &mut Model) {
    let Some(draft) = model.draft.as_ref() else {
        ui.label("Loading the profile...");
        return;
    };
    let (name, dirty) = (draft.profile.name.clone(), draft.dirty());
    let problem = model.draft_problem();
    let can_save = model.can_save().is_ok();
    let is_active = name == model.active_profile();
    let recording = model.recording();
    ui.horizontal(|ui| {
        ui.strong(if name.is_empty() { "(unnamed)" } else { name.as_str() });
        if !is_active {
            ui.weak("(not the active profile)");
        }
        if dirty {
            ui.colored_label(Color32::YELLOW, "Unsaved changes");
        }
        if let Some(problem) = &problem {
            ui.colored_label(Color32::LIGHT_RED, problem);
        }
        if recording && dirty {
            ui.weak("Finish or cancel the recording first");
        }
        if ui.add_enabled(can_save, egui::Button::new("Apply")).clicked() {
            model.apply_draft();
        }
        if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
            model.discard_draft();
        }
        ui.checkbox(&mut model.auto_apply, "Apply as I edit");
    });
}

fn overview(ui: &mut egui::Ui, model: &Model) {
    ui.heading("Overview");
    let Some(state) = &model.state else { return };
    egui::Grid::new("overview").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        let mut row = |name: &str, value: String| {
            ui.label(name);
            ui.label(value);
            ui.end_row();
        };
        row("Keyboard", device_text(&state.device));
        row("Active profile", state.active_profile.clone());
        row("Brightness", format!("{}%", state.brightness));
        row("Polling rate", state.polling_rate_hz.map_or("device default".to_owned(), |hz| format!("{hz} Hz")));
        row(
            "WinLock",
            match state.winlock {
                Some(true) => "on".to_owned(),
                Some(false) => "off".to_owned(),
                None => "unknown".to_owned(),
            },
        );
        row("Dial mode", state.dial_mode.clone());
        row("Start at login", if state.autostart { "on".to_owned() } else { "off".to_owned() });
    });
    for problem in &state.config_errors {
        ui.colored_label(Color32::LIGHT_RED, problem);
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::{NodeT, Queryable};
    use ipc::Request;

    use super::*;
    use crate::model::Effect;
    use crate::testing::{harness, online};

    #[test]
    fn the_offline_screen_offers_to_start_the_daemon_once() {
        let mut model = Model::new();
        model.apply(Inbox::Disconnected("cannot reach k100d: connection refused".into()));
        let mut harness = harness(model, root);
        harness.run();
        harness.get_by_label_contains("connection refused");
        harness.get_by_label("Start k100d").click();
        harness.run();
        assert_eq!(harness.state_mut().take_effects(), [Effect::StartDaemon]);
        let button = harness.get_by_label("Starting k100d...");
        assert!(button.accesskit_node().is_disabled());
    }

    #[test]
    fn the_launcher_thread_always_reports_back() {
        let (inbox, messages) = std::sync::mpsc::channel();
        start_daemon(inbox.clone(), || Ok(Launched::Started));
        let finished = messages.recv_timeout(Duration::from_secs(5));
        assert_eq!(finished, Ok(Inbox::LaunchFinished));
        start_daemon(inbox, || Err(LaunchError::Missing("k100d".into())));
        let failed = messages.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(failed, Inbox::LaunchFailed(message) if message.contains("k100d")));
    }

    #[test]
    fn connecting_shows_progress_and_no_start_button() {
        let mut harness = harness(Model::new(), root);
        harness.run_steps(2);
        harness.get_by_label("Connecting to k100d...");
        assert!(harness.query_by_label("Start k100d").is_none());
    }

    #[test]
    fn the_overview_lists_the_daemon_state() {
        let mut harness = harness(online("games", &["default", "games"]), root);
        harness.run();
        assert_eq!(harness.get_all_by_label("Overview").count(), 2, "nav entry and heading");
        harness.get_by_label("Active profile");
        harness.get_by_label("games");
        harness.get_by_label("80%");
        assert!(harness.get_all_by_label_contains("K100 RGB connected").count() >= 1);
    }

    #[test]
    fn apply_is_disabled_for_an_invalid_draft_and_the_reason_is_shown() {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.name = String::new();
        let mut harness = harness(model, draft_bar);
        harness.run();
        assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
        harness.get_by_label_contains("needs a name");
    }

    #[test]
    fn apply_is_disabled_while_a_macro_is_being_recorded() {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
        model.state.as_mut().unwrap().recording = actions::RecorderState::Recording { key: 131 };
        let mut harness = harness(model, draft_bar);
        harness.run();
        assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
        harness.get_by_label("Finish or cancel the recording first");
        harness.state_mut().state.as_mut().unwrap().recording = actions::RecorderState::Idle;
        harness.run();
        assert!(!harness.get_by_label("Apply").accesskit_node().is_disabled());
        assert!(harness.query_by_label("Finish or cancel the recording first").is_none());
    }

    #[test]
    fn apply_sends_the_draft_and_revert_reloads_it() {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
        let mut harness = harness(model, draft_bar);
        harness.run();
        harness.get_by_label("Apply").click();
        harness.run();
        assert!(matches!(harness.state_mut().take_effects().as_slice(), [Effect::Send(Request::SaveProfile { .. })]));
        harness.get_by_label("Revert").click();
        harness.run();
        let reload = Effect::Send(Request::GetProfile { name: "games".into() });
        assert_eq!(harness.state_mut().take_effects(), [reload]);
    }

    #[test]
    fn the_stale_banner_offers_keep_or_reload() {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        model.stale = Some(crate::model::Stale::changed_outside("games"));
        let mut harness = harness(model, stale_banner);
        harness.run();
        harness.get_by_label_contains("changed outside");
        harness.get_by_label("Keep my edits").click();
        harness.run();
        assert!(harness.state().stale.is_none());
        assert!(harness.state().draft.as_ref().unwrap().dirty());
    }

    #[test]
    fn a_banner_that_would_drop_a_macro_says_so_and_holds_apply() {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        model.stale = Some(crate::model::Stale::macro_conflict("g1", "games"));
        let mut harness = harness(model, |ui, model| {
            stale_banner(ui, model);
            draft_bar(ui, model);
        });
        harness.run();
        harness.get_by_label_contains("g1");
        assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
        harness.get_by_label("Keep my edits (drop the macro)").click();
        harness.run();
        assert!(harness.state().stale.is_none());
        assert!(!harness.get_by_label("Apply").accesskit_node().is_disabled());
    }

    #[test]
    fn the_lighting_pane_is_in_the_navigation() {
        let mut harness = harness(online("games", &["games"]), root);
        harness.run();
        assert_eq!(harness.state().pane, Pane::Overview);
        harness.get_by_label("Lighting").click();
        harness.run();
        assert_eq!(harness.state().pane, Pane::Lighting);
        harness.get_by_label("Select all");
    }

    #[test]
    fn the_device_pane_is_in_the_navigation_and_leaves_the_profile_alone() {
        assert!(!Pane::Device.edits_profile());
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
        model.pane = Pane::Lighting;
        let mut harness = harness(model, root);
        harness.run();
        harness.get_by_label("Unsaved changes");
        harness.get_by_label("Device").click();
        harness.run();
        assert_eq!(harness.state().pane, Pane::Device);
        harness.get_by_label("Apply debounce");
        assert!(harness.query_by_label("Unsaved changes").is_none());
        assert!(harness.state().draft.as_ref().unwrap().dirty());
    }

    #[test]
    fn the_profiles_pane_is_in_the_navigation_and_edits_the_profile() {
        assert!(Pane::Profiles.edits_profile());
        let mut harness = harness(online("games", &["games"]), root);
        harness.run();
        harness.get_by_label("Profiles").click();
        harness.run();
        assert_eq!(harness.state().pane, Pane::Profiles);
        harness.get_by_label("Editing games");
        harness.get_by_label("Add program");
    }

    #[test]
    fn toasts_can_be_dismissed() {
        let mut model = online("games", &["games"]);
        model.toast("keyboard rejected 8000 Hz");
        let mut harness = harness(model, root);
        harness.run();
        harness.get_by_label("keyboard rejected 8000 Hz");
        harness.get_by_label("Dismiss").click();
        harness.run();
        assert!(harness.state().toasts.is_empty());
    }
}

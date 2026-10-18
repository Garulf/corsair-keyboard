use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};
use ipc::launcher::{LaunchError, Launched};
use ipc::{Client, Request, SdkState, SdkStatus, sdk_client_text};

use crate::io::{Backend, Connector, RETRY_INTERVAL};
use crate::model::{Closing, Connection, Draft, Effect, Inbox, Model, Pane, device_text};

const REPAINT_EVERY: Duration = Duration::from_millis(100);

pub fn run() -> Result<(), String> {
    #[cfg(windows)]
    crate::desktop::require_interactive_desktop()?;
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

    fn flush(&mut self, ctx: &egui::Context) {
        for effect in self.model.take_effects() {
            match effect {
                Effect::Send(request) => self.backend.send(request),
                Effect::CloseWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Effect::StartDaemon => start_daemon(self.backend.injector(), || ipc::launcher::ensure_daemon(&[])),
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for message in self.backend.drain() {
            self.model.apply(message);
        }
        self.model.tick(self.started.elapsed());
        let model = &mut self.model;
        for command in ctx.input(|input| close_commands(model, input.viewport())) {
            ctx.send_viewport_cmd(command);
        }
        self.flush(ctx);
        ctx.request_repaint_after(REPAINT_EVERY);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        root(ui, &mut self.model);
        if self.model.has_effects() {
            ui.ctx().request_repaint();
        }
    }
}

pub fn close_commands(model: &mut Model, viewport: &egui::ViewportInfo) -> Vec<egui::ViewportCommand> {
    if !viewport.close_requested() || model.allow_close() {
        return Vec::new();
    }
    let mut commands = vec![egui::ViewportCommand::CancelClose];
    if viewport.minimized == Some(true) || viewport.occluded == Some(true) {
        commands.extend([egui::ViewportCommand::Minimized(false), egui::ViewportCommand::Focus]);
    }
    commands
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
    close_prompt(ui, model);
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
            Pane::Onboard => crate::onboard_page::onboard_page(ui, model),
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

pub fn close_prompt(ui: &mut egui::Ui, model: &mut Model) {
    if !matches!(model.closing, Closing::Asking | Closing::SavingThenClose) {
        return;
    }
    let refused = model.draft.as_ref().is_some_and(Draft::refused);
    let blocker = match model.can_save() {
        Err(reason) => Some(reason),
        Ok(()) if refused => Some("k100d refused these edits; change them or close without saving.".to_owned()),
        Ok(()) => None,
    };
    egui::Modal::new(egui::Id::unique("close-prompt")).show(ui.ctx(), |ui| {
        ui.heading("Close K100?");
        ui.label("The edited profile has changes that are not saved yet.");
        if let Some(reason) = &blocker {
            ui.weak(reason);
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(blocker.is_none(), egui::Button::new("Save and close")).clicked() {
                model.save_and_close();
            }
            if ui.button("Close without saving").clicked() {
                model.close_without_saving();
            }
            if ui.button("Cancel").clicked() {
                model.cancel_close();
            }
        });
    });
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

fn overview(ui: &mut egui::Ui, model: &mut Model) {
    ui.heading("Overview");
    let Some(state) = model.state.clone() else { return };
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
    sdk_section(ui, model, &state.sdk);
}

fn sdk_section(ui: &mut egui::Ui, model: &mut Model, sdk: &SdkState) {
    let status = match &sdk.status {
        SdkStatus::Unsupported => return,
        SdkStatus::Off => "Off".to_owned(),
        SdkStatus::Listening => "Listening for iCUE SDK apps".to_owned(),
        SdkStatus::InUse => "In use by another program; k100d tries again every 10 s".to_owned(),
        SdkStatus::Failed { reason } => format!("Failed: {reason}"),
    };
    ui.add_space(12.0);
    ui.heading("iCUE SDK");
    ui.label(status);
    for client in &sdk.clients {
        ui.horizontal(|ui| {
            ui.label(sdk_client_text(client));
            if ui.button(format!("Block {}", client.exe)).clicked() {
                model.send(Request::SdkBlock { exe: client.exe.clone(), blocked: true });
            }
        });
    }
    for exe in &sdk.blocked {
        ui.horizontal(|ui| {
            ui.label(format!("{exe} is blocked"));
            if ui.button(format!("Unblock {exe}")).clicked() {
                model.send(Request::SdkBlock { exe: exe.clone(), blocked: false });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::{NodeT, Queryable};
    use ipc::SdkClient;

    use super::*;
    use crate::model::Effect;
    use crate::testing::{harness, online, sent, streamdeck};

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

    fn close_request(minimized: bool) -> egui::ViewportInfo {
        egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            minimized: Some(minimized),
            ..egui::ViewportInfo::default()
        }
    }

    fn unsaved() -> Model {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        model
    }

    #[test]
    fn a_close_request_over_unsaved_edits_is_cancelled_and_brings_a_hidden_window_back() {
        use egui::ViewportCommand::{CancelClose, Focus, Minimized};

        let mut visible = unsaved();
        assert_eq!(close_commands(&mut visible, &close_request(false)), [CancelClose]);
        assert_eq!(visible.closing, crate::model::Closing::Asking);

        let mut minimized = unsaved();
        assert_eq!(close_commands(&mut minimized, &close_request(true)), [CancelClose, Minimized(false), Focus]);
        assert_eq!(minimized.closing, crate::model::Closing::Asking);

        let mut occluded = unsaved();
        let behind = egui::ViewportInfo { occluded: Some(true), ..close_request(false) };
        assert_eq!(close_commands(&mut occluded, &behind), [CancelClose, Minimized(false), Focus]);

        let mut clean = online("games", &["games"]);
        assert!(close_commands(&mut clean, &close_request(true)).is_empty());
        let mut not_closing = unsaved();
        assert!(close_commands(&mut not_closing, &egui::ViewportInfo::default()).is_empty());
        assert_eq!(not_closing.closing, crate::model::Closing::Open);
    }

    fn offline_app(model: Model) -> App {
        let connector: Connector = Arc::new(|| Err(ipc::ClientError::Closed));
        let mut app = App::new(Backend::spawn(connector, Arc::new(|| {}), Duration::from_secs(60)));
        app.model = model;
        app
    }

    fn logic_pass(app: &mut App, viewport: egui::ViewportInfo) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.viewports.insert(egui::ViewportId::ROOT, viewport);
        let mut frame = eframe::Frame::_new_kittest();
        let mut output = ctx.run_logic(&input, |ctx| eframe::App::logic(app, ctx, &mut frame));
        output.viewport_commands.remove(&egui::ViewportId::ROOT).unwrap_or_default()
    }

    #[test]
    fn the_logic_pass_alone_keeps_a_minimized_window_with_unsaved_edits_open() {
        let mut app = offline_app(unsaved());
        let commands = logic_pass(&mut app, close_request(true));
        assert!(commands.contains(&egui::ViewportCommand::CancelClose), "{commands:?}");
        assert!(commands.contains(&egui::ViewportCommand::Minimized(false)), "{commands:?}");
        assert!(commands.contains(&egui::ViewportCommand::Focus), "{commands:?}");
        assert_eq!(app.model.closing, crate::model::Closing::Asking);

        app.model.close_without_saving();
        let commands = logic_pass(&mut app, egui::ViewportInfo::default());
        assert!(commands.contains(&egui::ViewportCommand::Close), "{commands:?}");
        assert!(logic_pass(&mut app, close_request(true)).is_empty(), "the confirmed close goes ahead");
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

    fn with_sdk(status: SdkStatus, clients: Vec<SdkClient>, blocked: &[&str]) -> Model {
        let mut model = online("default", &["default"]);
        let blocked = blocked.iter().map(|exe| (*exe).to_owned()).collect();
        model.state.as_mut().unwrap().sdk = SdkState { status, clients, blocked };
        model
    }

    #[test]
    fn the_overview_lists_sdk_clients_and_blocks_one() {
        let model = with_sdk(SdkStatus::Listening, vec![streamdeck()], &[]);
        let mut harness = harness(model, overview);
        harness.run();
        harness.get_by_label("iCUE SDK");
        harness.get_by_label("Listening for iCUE SDK apps");
        harness.get_by_label("StreamDeck.exe: exclusive G-keys, intercepts G1");
        harness.get_by_label("Block StreamDeck.exe").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::SdkBlock { exe: "StreamDeck.exe".into(), blocked: true }]);
    }

    #[test]
    fn blocked_exes_can_be_unblocked() {
        let model = with_sdk(SdkStatus::Listening, vec![], &["bad.exe"]);
        let mut harness = harness(model, overview);
        harness.run();
        harness.get_by_label("bad.exe is blocked");
        harness.get_by_label("Unblock bad.exe").click();
        harness.run();
        assert_eq!(sent(harness.state_mut()), [Request::SdkBlock { exe: "bad.exe".into(), blocked: false }]);
    }

    #[test]
    fn the_overview_says_when_another_program_owns_the_sdk_pipe() {
        let mut harness = harness(with_sdk(SdkStatus::InUse, vec![], &[]), overview);
        harness.run();
        harness.get_by_label("In use by another program; k100d tries again every 10 s");
    }

    #[test]
    fn the_overview_names_every_sdk_status() {
        for (status, text) in
            [(SdkStatus::Off, "Off"), (SdkStatus::Failed { reason: "denied".into() }, "Failed: denied")]
        {
            let mut harness = harness(with_sdk(status, vec![], &[]), overview);
            harness.run();
            harness.get_by_label(text);
        }
    }

    #[test]
    fn the_sdk_section_is_hidden_where_the_sdk_is_unsupported() {
        let mut harness = harness(with_sdk(SdkStatus::Unsupported, vec![], &[]), overview);
        harness.run();
        assert!(harness.query_by_label("iCUE SDK").is_none());
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

    fn closing(model: Model) -> egui_kittest::Harness<'static, Model> {
        let mut harness = harness(model, close_prompt);
        harness.run();
        harness
    }

    fn asking() -> Model {
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("x.exe".into());
        assert!(!model.allow_close());
        model
    }

    #[test]
    fn the_close_prompt_offers_save_discard_or_cancel() {
        let mut harness = closing(asking());
        harness.get_by_label_contains("not saved");
        harness.get_by_label("Save and close").click();
        harness.run();
        assert_eq!(harness.state().closing, crate::model::Closing::SavingThenClose);
        assert!(harness.get_by_label("Save and close").accesskit_node().is_disabled());

        let mut harness = closing(asking());
        harness.get_by_label("Close without saving").click();
        harness.run();
        assert!(harness.state_mut().take_effects().contains(&Effect::CloseWindow));

        let mut harness = closing(asking());
        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(harness.state().closing, crate::model::Closing::Open);
        assert!(harness.query_by_label("Save and close").is_none());
    }

    #[test]
    fn the_close_prompt_only_offers_to_save_a_draft_that_can_be_saved() {
        let mut model = asking();
        model.draft.as_mut().unwrap().profile.name = String::new();
        let harness = closing(model);
        assert!(harness.get_by_label("Save and close").accesskit_node().is_disabled());
        harness.get_by_label_contains("needs a name");
        assert!(!harness.get_by_label("Close without saving").accesskit_node().is_disabled());
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
    fn the_onboard_pane_is_in_the_navigation_and_leaves_the_profile_alone() {
        assert!(!Pane::Onboard.edits_profile());
        let mut model = online("games", &["games"]);
        model.draft.as_mut().unwrap().profile.apps.push("game.exe".into());
        model.pane = Pane::Lighting;
        let mut harness = harness(model, root);
        harness.run();
        harness.get_by_label("Unsaved changes");
        harness.get_by_label("Onboard").click();
        harness.run();
        assert_eq!(harness.state().pane, Pane::Onboard);
        harness.get_by_label("Onboard profiles");
        assert!(harness.query_by_label("Unsaved changes").is_none());
        assert!(harness.state().draft.as_ref().unwrap().dirty());
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

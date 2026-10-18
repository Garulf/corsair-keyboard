use eframe::egui::{self, Vec2};
use egui_kittest::Harness;
use ipc::{DaemonState, DeviceState, OnboardRequest, OnboardSlot, Profile, Reply, Request, Settings};

use crate::model::{Connection, Draft, Effect, Inbox, Model};

pub fn profile(name: &str) -> Profile {
    Profile {
        version: 1,
        name: name.into(),
        apps: Vec::new(),
        layers: Vec::new(),
        assignments: Vec::new(),
        dial: actions::DialConfig::default(),
    }
}

pub fn mixed_profile(name: &str) -> Profile {
    let mut mixed = profile(name);
    mixed.layers.push(engine::LayerConfig::new(
        "fade",
        engine::Zone::Named(engine::NamedZone::Logo),
        engine::EffectConfig::Gradient { from: k100::Rgb::BLACK, to: k100::Rgb::BLACK, angle: 0.0 },
    ));
    mixed
}

pub fn state(active: &str, profiles: &[&str]) -> DaemonState {
    DaemonState {
        device: DeviceState::Connected { model: "K100 RGB".into(), firmware: "1.15.61".into() },
        active_profile: active.into(),
        profiles: profiles.iter().map(|name| (*name).to_owned()).collect(),
        brightness: 80,
        ..DaemonState::default()
    }
}

pub fn streamdeck() -> ipc::SdkClient {
    ipc::SdkClient {
        exe: "StreamDeck.exe".into(),
        access: ipc::SdkAccess::ExclusiveKeys,
        priority: 128,
        intercepted: vec!["G1".into()],
        refused: false,
    }
}

pub fn online(active: &str, profiles: &[&str]) -> Model {
    let mut model = Model::new();
    model.connection = Connection::Online;
    model.state = Some(state(active, profiles));
    model.settings = Some(Settings::default());
    model.draft = Some(Draft::new(profile(active)));
    model
}

pub fn harness<'a>(model: Model, app: impl FnMut(&mut egui::Ui, &mut Model) + 'a) -> Harness<'a, Model> {
    Harness::builder().with_size(Vec2::new(1100.0, 760.0)).with_accessibility_check(false).build_ui_state(app, model)
}

pub fn sent(model: &mut Model) -> Vec<Request> {
    model
        .take_effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Send(request) => Some(request),
            Effect::StartDaemon | Effect::CloseWindow => None,
        })
        .collect()
}

pub fn copy(model: &Model) -> Model {
    let mut copy = Model::new();
    copy.connection = model.connection.clone();
    copy.state = model.state.clone();
    copy.settings = model.settings.clone();
    copy.draft = model.draft.clone();
    copy
}

pub fn onboard(request: OnboardRequest) -> Request {
    Request::Onboard { request }
}

pub fn onboard_slots(model: &mut Model, names: &[&str], active: Option<u32>) {
    let slots = (0u32..)
        .zip(names)
        .map(|(index, name)| OnboardSlot { index, name: (*name).to_owned(), files: 6, bytes: 400 })
        .collect();
    let reply = Reply::OnboardSlots { slots, active, free_bytes: Some(8_000_000) };
    model.apply(Inbox::reply(onboard(OnboardRequest::List), reply));
}

pub fn editing(edited: Profile, names: &[&str]) -> Model {
    let mut model = online("p", &["p", "games"]);
    model.draft = Some(Draft::new(edited));
    onboard_slots(&mut model, names, Some(0));
    model
}

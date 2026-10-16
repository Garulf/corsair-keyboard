use eframe::egui::{self, Vec2};
use egui_kittest::Harness;
use ipc::{DaemonState, DeviceState, Profile, Request, Settings};

use crate::model::{Connection, Draft, Effect, Model};

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

pub fn state(active: &str, profiles: &[&str]) -> DaemonState {
    DaemonState {
        device: DeviceState::Connected { model: "K100 RGB".into(), firmware: "1.15.61".into() },
        active_profile: active.into(),
        profiles: profiles.iter().map(|name| (*name).to_owned()).collect(),
        brightness: 80,
        ..DaemonState::default()
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
            Effect::StartDaemon => None,
        })
        .collect()
}

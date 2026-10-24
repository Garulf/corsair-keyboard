use std::path::PathBuf;

use clap::Subcommand;
use ipc::{AudioState, Client, DaemonState, DeviceState, Profile, Reply, Request, SdkState, SdkStatus};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List profiles
    List,
    /// Switch the active profile
    Set { name: String },
    /// Print a profile as TOML
    Show { name: String },
    /// Create or replace a profile from a TOML file
    Import { file: PathBuf },
    /// Write a profile to a TOML file
    Export { name: String, file: PathBuf },
}

pub fn status() -> Result<()> {
    match Client::connect()?.request(&Request::GetState)? {
        Reply::State { state } => {
            println!("{}", format_state(&state));
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}").into()),
    }
}

pub fn profile(command: ProfileCommand) -> Result<()> {
    let mut client = Client::connect()?;
    match command {
        ProfileCommand::List => match client.request(&Request::ListProfiles)? {
            Reply::Profiles { names } => names.iter().for_each(|name| println!("{name}")),
            other => return Err(format!("unexpected reply {other:?}").into()),
        },
        ProfileCommand::Set { name } => expect_done(client.request(&Request::SetActiveProfile { name })?)?,
        ProfileCommand::Show { name } => println!("{}", toml::to_string_pretty(&fetch(&mut client, name)?)?),
        ProfileCommand::Import { file } => {
            let profile: Profile = toml::from_str(&std::fs::read_to_string(&file)?)?;
            expect_done(client.request(&Request::SaveProfile { profile })?)?;
        }
        ProfileCommand::Export { name, file } => {
            std::fs::write(&file, toml::to_string_pretty(&fetch(&mut client, name)?)?)?;
        }
    }
    Ok(())
}

pub fn simple(request: Request) -> Result<()> {
    Ok(expect_done(Client::connect()?.request(&request)?)?)
}

pub fn events() -> Result<()> {
    let mut client = Client::connect()?;
    expect_done(client.request(&Request::Subscribe)?)?;
    while let Some(event) = client.next_event()? {
        println!("{event:?}");
    }
    Ok(())
}

pub fn fetch(client: &mut Client, name: String) -> Result<Profile> {
    match client.request(&Request::GetProfile { name })? {
        Reply::Profile { profile } => Ok(profile),
        Reply::Error { message } => Err(message.into()),
        other => Err(format!("unexpected reply {other:?}").into()),
    }
}

pub fn expect_done(reply: Reply) -> std::result::Result<(), String> {
    match reply {
        Reply::Done => Ok(()),
        Reply::Error { message } => Err(message),
        other => Err(format!("unexpected reply {other:?}")),
    }
}

pub fn format_state(state: &DaemonState) -> String {
    let device = match &state.device {
        DeviceState::Connected { model, firmware } => format!("connected: {model} (firmware {firmware})"),
        DeviceState::Disconnected => "no keyboard found".to_owned(),
        DeviceState::Blocked { processes } => format!(
            "blocked by {}; close it, or list it under ignore_conflicts in settings.toml if it does not control the K100",
            processes.join(", ")
        ),
        DeviceState::Released => "released (run `k100 reacquire` to take it back)".to_owned(),
        DeviceState::Paused { reason } => format!("paused: {reason}"),
    };
    let polling = state.polling_rate_hz.map_or("device default".to_owned(), |hz| format!("{hz} Hz"));
    let mut lines = vec![
        format!("keyboard:   {device}"),
        format!("profile:    {} ({} profiles)", state.active_profile, state.profiles.len()),
        format!("brightness: {}%", state.brightness),
        format!("polling:    {polling}"),
    ];
    let winlock = match state.winlock {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    };
    lines.push(format!("winlock:    {winlock}"));
    lines.push(format!("dial:       {}", state.dial_mode));
    lines.push(format!("autostart:  {}", if state.autostart { "on" } else { "off" }));
    match state.recording {
        actions::RecorderState::Idle => {}
        actions::RecorderState::AwaitingKey => lines.push("recording:  press a G key to choose the macro key".into()),
        actions::RecorderState::Recording { key } => {
            let name = actions::Key::from_index(key).map_or(key.to_string(), |key| key.name().to_owned());
            lines.push(format!("recording:  into {name}"));
        }
    }
    match &state.audio {
        AudioState::Off => {}
        AudioState::Starting => lines.push("audio:      starting".into()),
        AudioState::Capturing { device } => lines.push(format!("audio:      capturing ({device})")),
        AudioState::Failed { reason } => lines.push(format!("audio:      failed: {reason}")),
    }
    lines.extend(sdk_lines(&state.sdk));
    lines.extend(state.config_errors.iter().map(|error| format!("config error: {error}")));
    lines.join("\n")
}

fn sdk_lines(sdk: &SdkState) -> Vec<String> {
    let status = match &sdk.status {
        SdkStatus::Unsupported => return Vec::new(),
        SdkStatus::Off => "off".to_owned(),
        SdkStatus::Listening => "listening".to_owned(),
        SdkStatus::InUse => "in use by another program (k100d tries again every 10 s)".to_owned(),
        SdkStatus::Failed { reason } => format!("failed: {reason}"),
    };
    let mut lines = vec![format!("icue sdk:   {status}")];
    lines.extend(sdk.clients.iter().map(|client| format!("  {}", ipc::sdk_client_text(client))));
    if !sdk.blocked.is_empty() {
        lines.push(format!("  blocked: {}", sdk.blocked.join(", ")));
    }
    lines
}

pub fn key_list() -> String {
    actions::all()
        .iter()
        .filter(|def| def.index.is_none() || def.is_on_k100())
        .map(|def| {
            let index = def.index.map_or("-".to_owned(), |index| index.to_string());
            let note = if def.output.is_some() { "" } else { "  input only" };
            format!("{:<18}{index:>4}{note}", def.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use ipc::{AudioState, DaemonState, DeviceState, Reply};

    use super::*;

    #[test]
    fn formats_state_for_humans() {
        let state = DaemonState {
            device: DeviceState::Connected { model: "K100 RGB".into(), firmware: "1.15.61".into() },
            active_profile: "games".into(),
            profiles: vec!["default".into(), "games".into()],
            brightness: 70,
            polling_rate_hz: Some(4000),
            max_polling_rate_hz: Some(8000),
            config_errors: vec!["broken.toml: oops".into()],
            winlock: Some(true),
            dial_mode: "brightness".into(),
            recording: actions::RecorderState::Idle,
            autostart: true,
            audio: AudioState::Off,
            sdk: ipc::SdkState::default(),
        };
        let text = format_state(&state);
        assert!(text.contains("connected: K100 RGB (firmware 1.15.61)"), "{text}");
        assert!(text.contains("profile:    games (2 profiles)"), "{text}");
        assert!(text.contains("brightness: 70%"), "{text}");
        assert!(text.contains("polling:    4000 Hz"), "{text}");
        assert!(text.contains("broken.toml: oops"), "{text}");
        assert!(text.contains("winlock:    on"), "{text}");
        assert!(text.contains("dial:       brightness"), "{text}");
        assert!(text.contains("autostart:  on"), "{text}");
        assert!(!text.contains("recording"), "{text}");
        assert!(!text.contains("audio"), "{text}");
        let capturing = DaemonState { audio: AudioState::Capturing { device: "Speakers".into() }, ..state.clone() };
        assert!(format_state(&capturing).contains("audio:      capturing (Speakers)"));
        let starting = DaemonState { audio: AudioState::Starting, ..state.clone() };
        assert!(format_state(&starting).contains("audio:      starting"));
        let failed = DaemonState { audio: AudioState::Failed { reason: "no sink".into() }, ..state.clone() };
        assert!(format_state(&failed).contains("audio:      failed: no sink"));
        let recording = DaemonState { recording: actions::RecorderState::Recording { key: 131 }, ..state.clone() };
        assert!(format_state(&recording).contains("recording:  into g1"));
        let streamdeck = ipc::SdkClient {
            exe: "StreamDeck.exe".into(),
            access: ipc::SdkAccess::ExclusiveKeys,
            priority: 128,
            intercepted: vec!["G1".into(), "G2".into()],
            refused: false,
        };
        let sdk = ipc::SdkState {
            status: ipc::SdkStatus::Listening,
            clients: vec![streamdeck],
            blocked: vec!["bad.exe".into()],
        };
        let listening = DaemonState { sdk, ..state.clone() };
        let text = format_state(&listening);
        assert!(
            text.contains(
                "icue sdk:   listening\n  StreamDeck.exe: exclusive G-keys, intercepts G1 G2\n  blocked: bad.exe"
            ),
            "{text}"
        );
        let sdk_status = |status| {
            format_state(&DaemonState { sdk: ipc::SdkState { status, ..ipc::SdkState::default() }, ..state.clone() })
        };
        assert!(
            sdk_status(ipc::SdkStatus::InUse)
                .contains("icue sdk:   in use by another program (k100d tries again every 10 s)")
        );
        assert!(sdk_status(ipc::SdkStatus::Off).contains("icue sdk:   off"));
        assert!(sdk_status(ipc::SdkStatus::Failed { reason: "denied".into() }).contains("icue sdk:   failed: denied"));
        assert!(!format_state(&state).contains("icue sdk"), "Unsupported hides the line");
        let blocked = DaemonState { device: DeviceState::Blocked { processes: vec!["iCUE.exe".into()] }, ..state };
        let blocked_text = format_state(&blocked);
        assert!(blocked_text.contains("blocked by iCUE.exe"));
        assert!(blocked_text.contains("ignore_conflicts"), "{blocked_text}");
    }

    #[test]
    fn key_list_shows_names_indices_and_input_only_keys() {
        let text = key_list();
        assert!(text.lines().any(|line| line.starts_with("caps_lock") && line.contains("57")), "{text}");
        assert!(text.lines().any(|line| line.starts_with("g1") && line.contains("input only")), "{text}");
        assert!(text.lines().any(|line| line.starts_with("f13") && line.contains('-')), "{text}");
        assert!(!text.lines().any(|line| line.starts_with("mr ") || line.starts_with("brightness")), "{text}");
    }

    #[test]
    fn done_or_error() {
        assert_eq!(expect_done(Reply::Done), Ok(()));
        assert_eq!(expect_done(Reply::Error { message: "no".into() }), Err("no".to_string()));
        assert!(expect_done(Reply::Profiles { names: vec![] }).is_err());
    }
}

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use clap::{Args, Subcommand};
use daemon::config::{ConfigDir, profile_file_name};
use ipc::{Client, ClientError, InstanceLock, OnboardRequest, OnboardSlot, Profile, Reply, Request, lock_path};
use k100::Device;
use k100::hid::{OpenedDevice, open_device};
use onboard::{Backups, Conversion, convert, execute, replay_hardware_profile};

use crate::remote;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Args)]
pub struct OnboardArgs {
    /// Config folder for direct access and dry runs; while k100d runs, its own folder applies
    #[arg(long, global = true)]
    pub config_dir: Option<PathBuf>,
    #[command(subcommand)]
    pub command: OnboardCommand,
}

#[derive(Debug, Subcommand)]
pub enum OnboardCommand {
    /// List the profiles stored on the keyboard
    List,
    /// Save an app profile to the keyboard (a new slot, or --slot N to overwrite)
    Save {
        profile: String,
        #[arg(long)]
        slot: Option<u32>,
        /// Show what would be written and what is left out, without touching the keyboard
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove a stored profile
    Delete { slot: u32 },
    /// Choose the stored profile the keyboard plays
    Activate { slot: u32 },
    /// Copy every stored profile to a folder
    Backup {
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Put back the stored profiles from a backup folder
    Restore { from: PathBuf },
}

const DAEMON_NOTE: &str =
    "The keyboard plays onboard profiles while k100d is not controlling it: run k100 release or quit k100d.";

pub fn run(args: OnboardArgs) -> Result<()> {
    let OnboardArgs { config_dir, command } = args;
    let route = route(Client::connect())?;
    let Some(request) = request_for(&command)? else {
        let OnboardCommand::Save { profile, .. } = command else { unreachable!("only a dry run has no request") };
        return dry_run(&profile, route, config_dir);
    };
    match route {
        Route::Daemon(client) => {
            if config_dir.is_some() {
                eprintln!("note: k100d is running, so its own config folder applies instead of --config-dir");
            }
            through_daemon(client, &request)
        }
        Route::Direct => direct(&request, config_dir),
    }
}

fn request_for(command: &OnboardCommand) -> Result<Option<OnboardRequest>> {
    Ok(Some(match command {
        OnboardCommand::List => OnboardRequest::List,
        OnboardCommand::Save { dry_run: true, .. } => return Ok(None),
        OnboardCommand::Save { profile, slot, dry_run: false } => {
            OnboardRequest::Save { profile: profile.clone(), slot: *slot }
        }
        OnboardCommand::Delete { slot } => OnboardRequest::Delete { slot: *slot },
        OnboardCommand::Activate { slot } => OnboardRequest::Activate { slot: *slot },
        OnboardCommand::Backup { to } => OnboardRequest::Backup { to: to.as_deref().map(absolute_text).transpose()? },
        OnboardCommand::Restore { from } => OnboardRequest::Restore { from: absolute_text(from)? },
    }))
}

fn absolute_text(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path)?;
    absolute
        .into_os_string()
        .into_string()
        .map_err(|path| -> Box<dyn std::error::Error> { format!("{} is not valid UTF-8", path.display()).into() })
}

fn changes_slots(request: &OnboardRequest) -> bool {
    !matches!(request, OnboardRequest::List | OnboardRequest::Backup { .. })
}

#[derive(Debug)]
enum Route {
    Daemon(Client),
    Direct,
}

fn route(connection: std::result::Result<Client, ClientError>) -> Result<Route> {
    match connection {
        Ok(client) => Ok(Route::Daemon(client)),
        Err(ClientError::Io(_)) => Ok(Route::Direct),
        Err(other) => Err(other.into()),
    }
}

fn through_daemon(mut client: Client, request: &OnboardRequest) -> Result<()> {
    let reply = client.request(&Request::Onboard { request: request.clone() })?;
    println!("{}", render(reply)?);
    if changes_slots(request) {
        println!("{DAEMON_NOTE}");
    }
    Ok(())
}

fn direct(request: &OnboardRequest, config_dir: Option<PathBuf>) -> Result<()> {
    let _lock = direct_lock(&lock_path())?;
    let config = config_folder(config_dir)?;
    let profiles = match request {
        OnboardRequest::Save { profile, .. } => vec![find_profile(&config, profile)?],
        _ => Vec::new(),
    };
    let OpenedDevice { commands, model, .. } = open_device()?;
    let mut device = Device::new(commands);
    let named = |name: &str| profiles.iter().find(|profile| profile.name == name).cloned();
    let reply = execute(&mut device, request, &named, &Backups::under(config.root()), SystemTime::now())?;
    println!("{}", render(reply)?);
    if changes_slots(request) {
        replay_hardware_profile(&mut device, model.key_count).map_err(|error| {
            format!("the change was made, but returning the keyboard to hardware mode failed: {error}")
        })?;
    }
    Ok(())
}

fn direct_lock(path: &Path) -> Result<InstanceLock> {
    InstanceLock::acquire(path).map_err(|error| -> Box<dyn std::error::Error> {
        match error.kind() {
            io::ErrorKind::AlreadyExists => "k100d is running but did not answer; try again or quit it".into(),
            _ => error.into(),
        }
    })
}

fn config_folder(config_dir: Option<PathBuf>) -> Result<ConfigDir> {
    match config_dir {
        Some(path) => Ok(ConfigDir::new(path)),
        None => ConfigDir::default_location().ok_or_else(|| "no config folder found; pass --config-dir".into()),
    }
}

fn dry_run(name: &str, route: Route, config_dir: Option<PathBuf>) -> Result<()> {
    let profile = match route {
        Route::Daemon(mut client) => remote::fetch(&mut client, name.to_owned())?,
        Route::Direct => find_profile(&config_folder(config_dir)?, name)?,
    };
    println!("{}", dry_run_text(&convert(&profile)));
    Ok(())
}

fn find_profile(config: &ConfigDir, name: &str) -> Result<Profile> {
    let (profiles, load_errors) = config.load_profiles();
    let found = profiles.into_iter().find(|profile| profile.name == name);
    found.ok_or_else(|| unknown_profile(config, name, &load_errors).into())
}

fn unknown_profile(config: &ConfigDir, name: &str, load_errors: &[String]) -> String {
    let own_file = format!("{}:", config.profiles_dir().join(profile_file_name(name)).display());
    let quoted_name = format!("{name:?}");
    let own_error = load_errors.iter().find(|error| error.starts_with(&own_file) || error.contains(&quoted_name));
    match own_error {
        Some(error) => format!("profile {name:?} could not be loaded: {error}"),
        None => format!("no profile named {name:?}"),
    }
}

fn render(reply: Reply) -> Result<String> {
    match reply {
        Reply::OnboardSlots { slots, active, free_bytes } => Ok(format_slots(&slots, active, free_bytes)),
        Reply::OnboardDone { message, backup, notes } => Ok(format_done(&message, backup.as_deref(), &notes)),
        Reply::Error { message } => Err(message.into()),
        other => Err(format!("unexpected reply {other:?}").into()),
    }
}

fn format_slots(slots: &[OnboardSlot], active: Option<u32>, free_bytes: Option<u32>) -> String {
    let mut lines = vec!["  slot files bytes  name".to_owned()];
    for slot in slots {
        let marker = if active == Some(slot.index) { '*' } else { ' ' };
        lines.push(format!("{marker} {:<4} {:<5} {:<5}  {}", slot.index, slot.files, slot.bytes, slot.name));
    }
    lines.extend(free_bytes.map(|bytes| format!("free onboard space: {bytes} bytes")));
    lines.join("\n")
}

fn format_done(message: &str, backup: Option<&str>, notes: &[String]) -> String {
    let mut lines = vec![message.to_owned()];
    lines.extend(backup.map(|path| format!("backup: {path}")));
    lines.extend(notes.iter().map(|note| format!("  - {note}")));
    lines.join("\n")
}

fn dry_run_text(conversion: &Conversion) -> String {
    let files = conversion.tree.manifest();
    let bytes: usize = files.iter().map(|file| file.bytes).sum();
    let mut lines = vec![format!("{}: {} files, {bytes} bytes", conversion.tree.name, files.len())];
    lines.extend(files.iter().map(|file| format!("  {}: {} bytes", file.label, file.bytes)));
    lines.extend(conversion.report.lines());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_become_requests_with_absolute_paths() {
        assert_eq!(request_for(&OnboardCommand::Delete { slot: 1 }).unwrap(), Some(OnboardRequest::Delete { slot: 1 }));
        let dry = OnboardCommand::Save { profile: "games".into(), slot: None, dry_run: true };
        assert_eq!(request_for(&dry).unwrap(), None);
        let Some(OnboardRequest::Restore { from }) =
            request_for(&OnboardCommand::Restore { from: "b/1".into() }).unwrap()
        else {
            panic!("expected restore")
        };
        assert!(Path::new(&from).is_absolute() && from.ends_with('1'), "{from}");
        let Some(OnboardRequest::Backup { to: Some(to) }) =
            request_for(&OnboardCommand::Backup { to: Some("out".into()) }).unwrap()
        else {
            panic!("expected backup")
        };
        assert!(Path::new(&to).is_absolute() && to.ends_with("out"), "{to}");
        assert_eq!(
            request_for(&OnboardCommand::Backup { to: None }).unwrap(),
            Some(OnboardRequest::Backup { to: None })
        );
        let save = OnboardCommand::Save { profile: "games".into(), slot: Some(2), dry_run: false };
        assert_eq!(request_for(&save).unwrap(), Some(OnboardRequest::Save { profile: "games".into(), slot: Some(2) }));
    }

    #[test]
    fn slots_print_as_a_table_with_the_active_one_marked() {
        let slots = [
            OnboardSlot { index: 0, name: "HW Profile 2".into(), files: 16, bytes: 570 },
            OnboardSlot { index: 1, name: "games".into(), files: 6, bytes: 412 },
        ];
        assert_eq!(
            format_slots(&slots, Some(0), Some(8_388_608)),
            "  slot files bytes  name\n* 0    16    570    HW Profile 2\n  1    6     412    games\nfree onboard space: 8388608 bytes"
        );
        assert!(!format_slots(&slots, None, None).contains('*'));
        assert!(!format_slots(&slots, None, None).contains("free onboard space"));
    }

    #[test]
    fn done_replies_print_the_message_backup_and_notes() {
        let text =
            format_done("saved \"games\" to slot 1", Some("/b/1"), &["skipped: g4: open_url needs the PC".into()]);
        assert_eq!(text, "saved \"games\" to slot 1\nbackup: /b/1\n  - skipped: g4: open_url needs the PC");
        assert_eq!(format_done("slot 0 is active", None, &[]), "slot 0 is active");
    }

    #[test]
    fn a_missing_daemon_means_direct_access_and_other_failures_do_not() {
        assert!(matches!(route(Err(ClientError::Io(io::ErrorKind::NotFound.into()))), Ok(Route::Direct)));
        assert!(route(Err(ClientError::VersionMismatch { daemon: 3 })).is_err());
    }

    #[test]
    fn direct_access_refuses_while_k100d_holds_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("k100d.lock");
        let held = InstanceLock::acquire(&path).unwrap();
        let refusal = direct_lock(&path).unwrap_err().to_string();
        assert!(refusal.contains("k100d") && refusal.contains("did not answer"), "{refusal}");
        drop(held);
        direct_lock(&path).unwrap();
    }

    fn config_with(files: &[(&str, &str)]) -> (tempfile::TempDir, ConfigDir) {
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::new(temp.path().to_path_buf());
        std::fs::create_dir_all(config.profiles_dir()).unwrap();
        for (file, text) in files {
            std::fs::write(config.profiles_dir().join(file), text).unwrap();
        }
        (temp, config)
    }

    #[test]
    fn a_profile_that_failed_to_load_reports_its_load_error() {
        let (_temp, config) = config_with(&[("games.toml", "name = \"games\"\n[[layers]\n")]);
        let error = find_profile(&config, "games").unwrap_err().to_string();
        assert!(error.contains("games.toml") && error.contains("TOML parse error"), "{error}");
        assert!(!error.contains("no profile named"), "{error}");
    }

    #[test]
    fn a_profile_that_does_not_exist_is_named_and_others_load() {
        let red = "name = \"red\"\n";
        let (_temp, config) = config_with(&[("red.toml", red), ("broken.toml", "name = ")]);
        assert_eq!(find_profile(&config, "games").unwrap_err().to_string(), "no profile named \"games\"");
        assert_eq!(find_profile(&config, "red").unwrap().name, "red");
    }

    #[test]
    fn a_dry_run_lists_files_and_the_report() {
        let profile: ipc::Profile = toml::from_str(include_str!("../../../examples/profiles/actions.toml")).unwrap();
        let text = dry_run_text(&onboard::convert(&profile));
        assert!(text.starts_with("actions: 10 files, "), "{text}");
        for expected in ["  root: ", "  action g1 (text): ", "skipped: layer \"alert\""] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
    }

    #[test]
    fn replies_render_as_text_and_error_replies_become_errors() {
        let done = Reply::OnboardDone { message: "slot 1 (\"games\") is active".into(), backup: None, notes: vec![] };
        assert_eq!(render(done).unwrap(), "slot 1 (\"games\") is active");
        let slots = Reply::OnboardSlots { slots: vec![], active: None, free_bytes: None };
        assert_eq!(render(slots).unwrap(), "  slot files bytes  name");
        assert_eq!(
            render(Reply::Error { message: "no keyboard connected".into() }).unwrap_err().to_string(),
            "no keyboard connected"
        );
        assert!(render(Reply::Done).is_err());
    }

    #[test]
    fn only_requests_that_change_the_stored_slots_cycle_the_keyboard() {
        assert!(!changes_slots(&OnboardRequest::List));
        assert!(!changes_slots(&OnboardRequest::Backup { to: None }));
        for request in [
            OnboardRequest::Save { profile: "a".into(), slot: None },
            OnboardRequest::Delete { slot: 1 },
            OnboardRequest::Activate { slot: 1 },
            OnboardRequest::Restore { from: "/b".into() },
        ] {
            assert!(changes_slots(&request), "{request:?}");
        }
    }
}

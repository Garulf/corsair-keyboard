use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use daemon::audio::SystemAudio;
use daemon::config::ConfigDir;
use daemon::controller::Controller;
use daemon::runtime::{self, Command, HidConnector};
use daemon::sdk_server::with_device_id;
use daemon::server;
use daemon::shutdown::{self, ReleaseSignal};
use notify::{RecursiveMode, Watcher};

fn main() -> ExitCode {
    let instance = match ipc::InstanceLock::acquire(&ipc::lock_path()) {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("k100d: {error}");
            return ExitCode::from(ipc::startup_exit_code(&error));
        }
    };
    match run(instance) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("k100d: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(instance: ipc::InstanceLock) -> Result<(), Box<dyn std::error::Error>> {
    let mut config_dir = None;
    let mut exit_after = None;
    let mut no_tray = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => config_dir = args.next().map(PathBuf::from),
            "--no-tray" => no_tray = true,
            "--exit-after" => exit_after = args.next().map(|s| s.parse::<u64>()).transpose()?,
            other => return Err(format!("unknown argument {other:?}").into()),
        }
    }
    let config = match config_dir {
        Some(path) => ConfigDir::new(path),
        None => ConfigDir::default_location().ok_or("no home directory")?,
    };
    config.ensure_default_profile()?;
    let settings = with_device_id(&config, config.load_settings()?);
    let sdk_enabled = Arc::new(AtomicBool::new(settings.sdk.enabled));
    let (profiles, errors) = config.load_profiles();
    for error in &errors {
        eprintln!("k100d: config: {error}");
    }

    let listener = server::bind(&instance)?;
    let (sender, receiver) = mpsc::channel();

    let released = ReleaseSignal::default();
    shutdown::install(sender.clone(), released.clone())?;
    if let Some(source) = daemon::focus::platform_source() {
        daemon::focus::spawn_watcher(source, sender.clone());
    }

    let reload = sender.clone();
    let watched = config.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        let changed = event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove();
        if changed && !watched.only_logs_changed(&event.paths) {
            let _ = reload.send(Command::Reload);
        }
    })?;
    watcher.watch(&config.profiles_dir(), RecursiveMode::NonRecursive)?;
    watcher.watch(config.root(), RecursiveMode::NonRecursive)?;

    if let Some(seconds) = exit_after {
        let timer = sender.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(seconds));
            let _ = timer.send(Command::Shutdown);
        });
    }

    let server_sender = sender.clone();
    thread::spawn(move || server::serve(listener, server_sender));
    #[cfg(windows)]
    let sdk_listener = spawn_sdk_listener(&sender, &sdk_enabled);

    let (sink, sink_warning): (Box<dyn actions::InputSink>, _) = match actions::platform_sink() {
        Ok(sink) => (sink, None),
        Err(failure) => (Box::new(actions::NullSink), Some(format!("key actions unavailable: {failure}"))),
    };
    let (output, output_thread) = actions::spawn(sink);
    let mut controller =
        Controller::new(settings, profiles, errors, Box::new(output)).with_audio(Box::new(SystemAudio::default()));
    controller.set_autostart(autostart_port());
    if let Some(warning) = sink_warning {
        eprintln!("k100d: {warning}");
        controller.add_warning(warning);
        controller.set_output_available(false);
    }

    #[cfg(any(windows, feature = "tray"))]
    let tray = if no_tray {
        None
    } else {
        daemon::tray::spawn(sender.clone()).map_err(|failure| eprintln!("k100d: tray unavailable: {failure}")).ok()
    };
    #[cfg(not(any(windows, feature = "tray")))]
    let _ = no_tray;

    eprintln!("k100d: config in {}", config.root().display());
    runtime::run(controller, config, receiver, sender, HidConnector, sdk_enabled);
    #[cfg(windows)]
    if let Some(sdk_listener) = sdk_listener {
        sdk_listener.stop();
    }
    #[cfg(any(windows, feature = "tray"))]
    if let Some(tray) = tray {
        tray.stop();
    }
    let _ = output_thread.join();
    released.notify();
    drop(instance);
    eprintln!("k100d: stopped");
    Ok(())
}

#[cfg(windows)]
fn spawn_sdk_listener(
    sender: &mpsc::Sender<Command>,
    enabled: &Arc<AtomicBool>,
) -> Option<daemon::sdk_server::windows::SdkListener> {
    match daemon::sdk_server::windows::SdkListener::spawn(sender.clone(), enabled.clone()) {
        Ok(listener) => Some(listener),
        Err(error) => {
            eprintln!("k100d: iCUE SDK server unavailable: {error}");
            let status = ipc::SdkStatus::Failed { reason: error.to_string() };
            let _ = sender.send(Command::Sdk(daemon::sdk_server::SdkCommand::Status(status)));
            None
        }
    }
}

fn autostart_port() -> Box<dyn install::Autostart> {
    let k100 = match ipc::launcher::sibling_binary("k100") {
        Ok(path) if path.exists() => path,
        _ => return Box::new(install::Unavailable("k100 was not found next to k100d".into())),
    };
    install::platform(k100).unwrap_or_else(|failure| Box::new(install::Unavailable(failure.to_string())))
}

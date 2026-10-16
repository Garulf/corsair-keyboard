use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use daemon::config::ConfigDir;
use daemon::controller::Controller;
use daemon::runtime::{self, Command, HidConnector};
use daemon::server;
use daemon::shutdown::{self, ReleaseSignal};
use notify::{RecursiveMode, Watcher};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("k100d: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut config_dir = None;
    let mut exit_after = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => config_dir = args.next().map(PathBuf::from),
            "--exit-after" => exit_after = args.next().map(|s| s.parse::<u64>()).transpose()?,
            other => return Err(format!("unknown argument {other:?}").into()),
        }
    }
    let instance = ipc::InstanceLock::acquire(&ipc::lock_path())?;
    let config = match config_dir {
        Some(path) => ConfigDir::new(path),
        None => ConfigDir::default_location().ok_or("no home directory")?,
    };
    config.ensure_default_profile()?;
    let settings = config.load_settings()?;
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
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| e.kind.is_modify() || e.kind.is_create() || e.kind.is_remove()) {
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

    let (sink, sink_warning): (Box<dyn actions::InputSink>, _) = match actions::platform_sink() {
        Ok(sink) => (sink, None),
        Err(failure) => (Box::new(actions::NullSink), Some(format!("key actions unavailable: {failure}"))),
    };
    let (output, output_thread) = actions::spawn(sink);
    let mut controller = Controller::new(settings, profiles, errors, Box::new(output));
    controller.set_autostart(autostart_port());
    if let Some(warning) = sink_warning {
        eprintln!("k100d: {warning}");
        controller.add_warning(warning);
        controller.set_output_available(false);
    }

    eprintln!("k100d: config in {}", config.root().display());
    runtime::run(controller, config, receiver, sender, HidConnector);
    let _ = output_thread.join();
    released.notify();
    drop(instance);
    eprintln!("k100d: stopped");
    Ok(())
}

fn autostart_port() -> Box<dyn install::Autostart> {
    let k100 = match ipc::launcher::sibling_binary("k100") {
        Ok(path) if path.exists() => path,
        _ => return Box::new(install::Unavailable("k100 was not found next to k100d".into())),
    };
    install::platform(k100).unwrap_or_else(|failure| Box::new(install::Unavailable(failure.to_string())))
}

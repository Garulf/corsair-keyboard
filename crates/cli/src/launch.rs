use ipc::launcher::{self, Launched};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn daemon(foreground: bool, args: &[String]) -> Result<()> {
    if foreground {
        let daemon = launcher::sibling_binary("k100d")?;
        let status = std::process::Command::new(&daemon)
            .args(args)
            .status()
            .map_err(|failure| format!("cannot run {}: {failure}", daemon.display()))?;
        return if status.success() { Ok(()) } else { Err(format!("k100d exited with {status}").into()) };
    }
    match launcher::ensure_daemon(args)? {
        Launched::AlreadyRunning => println!("k100d is already running"),
        Launched::Started => println!("k100d started"),
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AutostartAction {
    On,
    Off,
    Status,
}

pub fn autostart(action: AutostartAction) -> Result<()> {
    let entry = install::platform(std::env::current_exe()?)?;
    match action {
        AutostartAction::On => {
            entry.enable()?;
            println!("k100d will start at login");
        }
        AutostartAction::Off => {
            entry.disable()?;
            println!("k100d will no longer start at login");
        }
        AutostartAction::Status => println!("autostart: {}", if entry.is_enabled()? { "on" } else { "off" }),
    }
    Ok(())
}

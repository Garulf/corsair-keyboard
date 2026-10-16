mod diag;
mod effect;
mod launch;
mod remote;

use clap::{Parser, Subcommand};
use ipc::Request;

#[derive(Debug, Parser)]
#[command(name = "k100", version, about = "Control a Corsair K100 keyboard without iCUE")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show daemon and keyboard status
    Status,
    /// Manage profiles
    #[command(subcommand)]
    Profile(remote::ProfileCommand),
    /// Set brightness in percent
    Brightness { percent: u8 },
    /// Set the USB polling rate in Hz (125 to 8000)
    Polling { hz: u32 },
    /// Hand the keyboard back to its onboard lighting
    Release,
    /// Take the keyboard back after a release
    Reacquire,
    /// Start k100d in the background, or run it in the foreground
    Daemon {
        #[arg(long)]
        foreground: bool,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Put a lighting effect on top of the active profile ("off" removes it)
    Effect {
        name: String,
        #[arg(long)]
        color: Option<String>,
    },
    /// Turn WinLock on or off
    Winlock {
        #[arg(value_enum)]
        state: Switch,
    },
    /// List key names for profiles, with their keyboard index
    Keys,
    /// Print key, dial and state events until Ctrl-C
    Events,
    /// Low-level hardware checks that talk to the keyboard directly (stop k100d first)
    #[command(subcommand)]
    Diag(diag::Diag),
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Switch {
    On,
    Off,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Status => remote::status(),
        Command::Profile(command) => remote::profile(command),
        Command::Brightness { percent } => remote::simple(Request::SetBrightness { percent }),
        Command::Polling { hz } => remote::simple(Request::SetPollingRate { hz }),
        Command::Release => remote::simple(Request::Release),
        Command::Reacquire => remote::simple(Request::Reacquire),
        Command::Daemon { foreground, args } => launch::daemon(foreground, &args),
        Command::Effect { name, color } => effect::run(&name, color.as_deref()),
        Command::Winlock { state } => remote::simple(Request::SetWinLock { enabled: matches!(state, Switch::On) }),
        Command::Keys => {
            println!("{}", remote::key_list());
            Ok(())
        }
        Command::Events => remote::events(),
        Command::Diag(diag) => diag::run(diag),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn daemon_forwards_arguments_after_the_separator() {
        let cli = Cli::try_parse_from(["k100", "daemon", "--foreground", "--", "--config-dir", "x"]).unwrap();
        let Command::Daemon { foreground, args } = cli.command else { panic!("expected daemon") };
        assert!(foreground);
        assert_eq!(args, ["--config-dir", "x"]);
    }

    #[test]
    fn effect_takes_a_name_and_an_optional_colour() {
        let cli = Cli::try_parse_from(["k100", "effect", "solid", "--color", "#ff0000"]).unwrap();
        let Command::Effect { name, color } = cli.command else { panic!("expected effect") };
        assert_eq!((name.as_str(), color.as_deref()), ("solid", Some("#ff0000")));
    }
}

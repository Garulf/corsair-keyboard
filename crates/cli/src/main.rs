mod diag;
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
    /// Print key, dial and state events until Ctrl-C
    Events,
    /// Low-level hardware checks that talk to the keyboard directly (stop k100d first)
    #[command(subcommand)]
    Diag(diag::Diag),
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
        Command::Events => remote::events(),
        Command::Diag(diag) => diag::run(diag),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

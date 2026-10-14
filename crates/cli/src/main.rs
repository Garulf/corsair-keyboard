mod diag;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "k100", version, about = "Control a Corsair K100 keyboard without iCUE")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Low-level hardware checks that talk to the keyboard directly
    #[command(subcommand)]
    Diag(diag::Diag),
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Diag(diag) => diag::run(diag),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

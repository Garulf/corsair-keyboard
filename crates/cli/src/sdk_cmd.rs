use clap::{Args, Subcommand};
use ipc::Request;

use crate::remote;

#[derive(Debug, Args)]
pub struct SdkArgs {
    #[command(subcommand)]
    pub command: SdkAction,
}

#[derive(Debug, Subcommand)]
pub enum SdkAction {
    /// Refuse an iCUE SDK app by its exe name
    Block { exe: String },
    /// Allow an iCUE SDK app again
    Unblock { exe: String },
}

pub fn run(args: SdkArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (exe, blocked) = match args.command {
        SdkAction::Block { exe } => (exe, true),
        SdkAction::Unblock { exe } => (exe, false),
    };
    remote::simple(Request::SdkBlock { exe, blocked })
}

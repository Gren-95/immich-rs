//! immich-rs: a Rust rewrite of the Immich command line interface (`@immich/cli`).

mod albums;
mod api;
mod auth;
mod cli;
mod config;
mod crawl;
mod date;
mod duplicates;
mod error;
mod hash;
mod ignore;
mod multipart;
mod output;
mod pathutil;
mod queue;
mod server_info;
mod signal;
mod upload;
mod url;
mod watch;

use std::process::ExitCode;

use clap::error::ErrorKind;

use crate::cli::{Invocation, ParseError};
use crate::error::Error;

fn run() -> Result<(), Error> {
    let invocation = match cli::parse() {
        Ok(invocation) => invocation,
        Err(ParseError::Clap(error)) => {
            let _ = error.print();
            // commander exits with 0 for help and 1 for every usage error.
            return match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => Ok(()),
                _ => Err(Error::Exit(1)),
            };
        }
        Err(ParseError::Message(message)) => {
            eprintln!("{message}");
            return Err(Error::Exit(1));
        }
    };

    match invocation {
        Invocation::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Invocation::MissingCommand => {
            eprint!("{}", cli::render_help(None).unwrap_or_default());
            Err(Error::Exit(1))
        }
        Invocation::Help(command) => match cli::render_help(command.as_deref()) {
            Some(help) => {
                print!("{help}");
                Ok(())
            }
            None => {
                eprintln!("error: unknown command '{}'", command.unwrap_or_default());
                Err(Error::Exit(1))
            }
        },
        Invocation::Login { url, key, global } => auth::login(&url, &key, &global),
        Invocation::Logout(global) => auth::logout(&global),
        Invocation::ServerInfo(global) => server_info::server_info(&global),
        Invocation::Upload(global, options) => upload::upload(&global, *options),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Exit(code)) => ExitCode::from(code),
        Err(Error::Fatal(message)) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
    }
}

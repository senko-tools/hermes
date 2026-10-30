//! Hermes chat server.

use std::process::ExitCode;

fn main() -> ExitCode {
    hermes_server::cli::run()
}

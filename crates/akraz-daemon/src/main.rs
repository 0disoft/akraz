use std::env;
use std::process::ExitCode;

use akraz_core::RuntimeInputState;
use akraz_daemon::build_daemon_status;
use akraz_platform::FakePlatformAdapter;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        Some("--version") | Some("-V") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some("--status") => {
            print_status();
            ExitCode::SUCCESS
        }
        Some(argument) => {
            eprintln!("unknown argument: {argument}");
            ExitCode::from(2)
        }
        None => {
            print_status();
            ExitCode::SUCCESS
        }
    }
}

fn print_version() {
    println!("akraz-daemon {}", env!("CARGO_PKG_VERSION"));
}

fn print_status() {
    let state = RuntimeInputState::new();
    let platform = FakePlatformAdapter::default();
    let status = match build_daemon_status(&state, &platform) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("failed to build daemon status: {error}");
            return;
        }
    };

    println!("akraz-daemon {}", status.daemon_version);
    println!("mode: {:?}", status.mode);
    println!(
        "protocol: {}.{}",
        status.protocol.major, status.protocol.minor
    );
}

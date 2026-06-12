use std::env;
use std::process::ExitCode;

use akraz_core::RuntimeInputState;
use akraz_daemon::{build_daemon_status, build_permissions_probe};
use akraz_ipc::{JsonRpcSuccess, to_json_line};
use akraz_platform::FakePlatformAdapter;

const LOCAL_REQUEST_ID: &str = "local";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        Some("--version") | Some("-V") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some("status") => print_status(),
        Some("permissions") => match args.next().as_deref() {
            Some("probe") => print_permissions_probe(),
            Some(argument) => {
                eprintln!("unknown permissions command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing permissions command");
                ExitCode::from(2)
            }
        },
        Some(argument) => {
            eprintln!("unknown command: {argument}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("usage: akrazctl <status|permissions probe|--version>");
            ExitCode::from(2)
        }
    }
}

fn print_version() {
    println!("akrazctl {}", env!("CARGO_PKG_VERSION"));
}

fn print_status() -> ExitCode {
    let state = RuntimeInputState::new();
    let platform = FakePlatformAdapter::default();
    let status = match build_daemon_status(&state, &platform) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("failed to build daemon status: {error}");
            return ExitCode::FAILURE;
        }
    };

    print_json_success(status)
}

fn print_permissions_probe() -> ExitCode {
    let platform = FakePlatformAdapter::default();
    let probe = match build_permissions_probe(&platform) {
        Ok(probe) => probe,
        Err(error) => {
            eprintln!("failed to probe permissions: {error}");
            return ExitCode::FAILURE;
        }
    };

    print_json_success(probe)
}

fn print_json_success<T>(result: T) -> ExitCode
where
    T: serde::Serialize,
{
    let response = JsonRpcSuccess::new(LOCAL_REQUEST_ID, result);
    match to_json_line(&response) {
        Ok(line) => {
            print!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to encode JSON response: {error}");
            ExitCode::FAILURE
        }
    }
}

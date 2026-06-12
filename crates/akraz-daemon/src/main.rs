use std::env;
use std::process::ExitCode;

use akraz_core::RuntimeInputState;
use akraz_protocol::ProtocolVersion;

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
    let protocol = ProtocolVersion::CURRENT;

    println!("akraz-daemon {}", env!("CARGO_PKG_VERSION"));
    println!("mode: {:?}", state.mode());
    println!("protocol: {}.{}", protocol.major, protocol.minor);
}

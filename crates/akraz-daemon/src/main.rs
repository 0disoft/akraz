use std::env;
use std::fmt::{Display, Formatter};
use std::process::ExitCode;

use akraz_core::RuntimeInputState;
use akraz_daemon::{DaemonIpcRunConfig, DaemonIpcServer, build_daemon_status, serve_daemon_ipc};
use akraz_ipc::{IpcEndpoint, resolve_current_default_endpoint};
use akraz_platform::FakePlatformAdapter;

fn main() -> ExitCode {
    match parse_daemon_command(env::args().skip(1)) {
        Ok(DaemonCommand::Version) => {
            print_version();
            ExitCode::SUCCESS
        }
        Ok(DaemonCommand::Status) => {
            print_status();
            ExitCode::SUCCESS
        }
        Ok(DaemonCommand::Serve(options)) => run_daemon(options),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn print_version() {
    println!("akraz-daemon {}", env!("CARGO_PKG_VERSION"));
}

fn run_daemon(options: ServeOptions) -> ExitCode {
    let endpoint = match options.endpoint {
        Some(endpoint) => endpoint,
        None => match resolve_current_default_endpoint() {
            Ok(endpoint) => endpoint,
            Err(error) => {
                eprintln!("failed to resolve daemon IPC endpoint: {error}");
                return ExitCode::FAILURE;
            }
        },
    };
    let config = if options.once {
        DaemonIpcRunConfig::serve_requests(endpoint, 1)
    } else {
        DaemonIpcRunConfig::serve_forever(endpoint)
    };
    let server = DaemonIpcServer::new(RuntimeInputState::new(), FakePlatformAdapter::default());

    eprintln!("akraz-daemon listening at {}", config.endpoint());
    match serve_daemon_ipc(&config, &server) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("daemon IPC failed: {error}");
            ExitCode::FAILURE
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonCommand {
    Serve(ServeOptions),
    Status,
    Version,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ServeOptions {
    endpoint: Option<IpcEndpoint>,
    once: bool,
}

fn parse_daemon_command<I>(args: I) -> Result<DaemonCommand, DaemonUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(DaemonCommand::Serve(ServeOptions::default()));
    };

    match first.as_str() {
        "--version" | "-V" => reject_trailing_args(args, DaemonCommand::Version),
        "--status" => reject_trailing_args(args, DaemonCommand::Status),
        "--serve" => parse_serve_options(args),
        argument if argument.starts_with("--endpoint") || argument == "--once" => {
            parse_serve_options(std::iter::once(first).chain(args))
        }
        argument => Err(DaemonUsageError::UnknownArgument(argument.to_string())),
    }
}

fn reject_trailing_args<I>(
    args: I,
    command: DaemonCommand,
) -> Result<DaemonCommand, DaemonUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    if let Some(argument) = args.next() {
        Err(DaemonUsageError::UnknownArgument(argument))
    } else {
        Ok(command)
    }
}

fn parse_serve_options<I>(args: I) -> Result<DaemonCommand, DaemonUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut options = ServeOptions::default();
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if argument == "--once" {
            options.once = true;
        } else if let Some(value) = argument.strip_prefix("--endpoint=") {
            options.endpoint = Some(IpcEndpoint::manual(value).map_err(DaemonUsageError::from)?);
        } else if argument == "--endpoint" {
            let value = args.next().ok_or(DaemonUsageError::MissingEndpointValue)?;
            options.endpoint = Some(IpcEndpoint::manual(value).map_err(DaemonUsageError::from)?);
        } else {
            return Err(DaemonUsageError::UnknownArgument(argument));
        }
    }

    Ok(DaemonCommand::Serve(options))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonUsageError {
    MissingEndpointValue,
    InvalidEndpoint(String),
    UnknownArgument(String),
}

impl From<akraz_ipc::IpcEndpointError> for DaemonUsageError {
    fn from(error: akraz_ipc::IpcEndpointError) -> Self {
        Self::InvalidEndpoint(error.to_string())
    }
}

impl Display for DaemonUsageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEndpointValue => formatter.write_str("missing value for --endpoint"),
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
            Self::UnknownArgument(argument) => write!(formatter, "unknown argument: {argument}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use akraz_ipc::{IpcEndpoint, IpcEndpointKind};

    use super::{DaemonCommand, DaemonUsageError, ServeOptions, parse_daemon_command};

    #[test]
    fn default_command_serves_forever_on_default_endpoint() {
        assert_eq!(
            parse_daemon_command(std::iter::empty::<String>()),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: None,
                once: false,
            }))
        );
    }

    #[test]
    fn parses_serve_endpoint_and_once_options() {
        assert_eq!(
            parse_daemon_command(
                ["--serve", "--endpoint", "local-test", "--once"].map(String::from)
            ),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
                once: true,
            }))
        );
        assert_eq!(
            parse_daemon_command(["--endpoint=local-test", "--once"].map(String::from)),
            Ok(DaemonCommand::Serve(ServeOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
                once: true,
            }))
        );
    }

    #[test]
    fn parses_status_and_version_commands() {
        assert_eq!(
            parse_daemon_command(["--status"].map(String::from)),
            Ok(DaemonCommand::Status)
        );
        assert_eq!(
            parse_daemon_command(["--version"].map(String::from)),
            Ok(DaemonCommand::Version)
        );
    }

    #[test]
    fn rejects_invalid_daemon_options() {
        assert_eq!(
            parse_daemon_command(["--endpoint"].map(String::from)),
            Err(DaemonUsageError::MissingEndpointValue)
        );
        assert_eq!(
            parse_daemon_command(["--status", "--once"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--once".to_string()))
        );
        assert_eq!(
            parse_daemon_command(["--bad"].map(String::from)),
            Err(DaemonUsageError::UnknownArgument("--bad".to_string()))
        );
    }
}

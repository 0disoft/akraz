use std::env;
use std::fmt::{Display, Formatter};
use std::process::ExitCode;

use akraz_core::RuntimeInputState;
use akraz_daemon::{build_daemon_status, build_permissions_probe};
use akraz_ipc::{IpcEndpoint, JsonRpcSuccess, to_json_line};
use akraz_platform::FakePlatformAdapter;

const LOCAL_REQUEST_ID: &str = "local";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        Some("--version") | Some("-V") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some("status") => match parse_status_options(args) {
            Ok(options) => print_status(options),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(2)
            }
        },
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

fn print_status(options: StatusOptions) -> ExitCode {
    let _endpoint = options.endpoint;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusOptions {
    endpoint: Option<IpcEndpoint>,
}

fn parse_status_options<I>(args: I) -> Result<StatusOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut endpoint = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--endpoint=") {
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else if argument == "--endpoint" {
            let value = args.next().ok_or(CliUsageError::MissingEndpointValue)?;
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else {
            return Err(CliUsageError::UnknownStatusOption(argument));
        }
    }

    Ok(StatusOptions { endpoint })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliUsageError {
    MissingEndpointValue,
    InvalidEndpoint(String),
    UnknownStatusOption(String),
}

impl From<akraz_ipc::IpcEndpointError> for CliUsageError {
    fn from(error: akraz_ipc::IpcEndpointError) -> Self {
        Self::InvalidEndpoint(error.to_string())
    }
}

impl Display for CliUsageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEndpointValue => formatter.write_str("missing value for --endpoint"),
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
            Self::UnknownStatusOption(argument) => {
                write!(formatter, "unknown status option: {argument}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use akraz_ipc::{IpcEndpoint, IpcEndpointKind};

    use super::{CliUsageError, StatusOptions, parse_status_options};

    #[test]
    fn parses_status_endpoint_option() {
        assert_eq!(
            parse_status_options(["--endpoint", "local-test"].map(String::from)),
            Ok(StatusOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
            })
        );
        assert_eq!(
            parse_status_options(["--endpoint=local-test"].map(String::from)),
            Ok(StatusOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
            })
        );
    }

    #[test]
    fn rejects_invalid_status_options() {
        assert_eq!(
            parse_status_options(["--endpoint"].map(String::from)),
            Err(CliUsageError::MissingEndpointValue)
        );
        assert_eq!(
            parse_status_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownStatusOption("--bad".to_string()))
        );
    }
}

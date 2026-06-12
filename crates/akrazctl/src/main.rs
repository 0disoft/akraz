use std::env;
use std::fmt::{Display, Formatter};
use std::process::ExitCode;

use akraz_ipc::{
    DaemonStatusParams, IpcEndpoint, IpcEndpointError, JsonRpcRequest, METHOD_DAEMON_STATUS,
    METHOD_PERMISSIONS_PROBE, OsLocalIpcClient, PermissionsProbeParams, call_json_rpc,
    resolve_current_default_endpoint,
};

const LOCAL_REQUEST_ID: &str = "local";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);

    match args.next().as_deref() {
        Some("--version") | Some("-V") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some("status") => match parse_endpoint_options(args) {
            Ok(options) => print_status(options),
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(2)
            }
        },
        Some("permissions") => match args.next().as_deref() {
            Some("probe") => match parse_endpoint_options(args) {
                Ok(options) => print_permissions_probe(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
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

fn print_status(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_permissions_probe(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_PERMISSIONS_PROBE,
        PermissionsProbeParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_local_daemon_response<P>(
    endpoint: Option<IpcEndpoint>,
    request: &JsonRpcRequest<P>,
) -> ExitCode
where
    P: serde::Serialize,
{
    let client = match build_daemon_client(endpoint) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    match call_json_rpc(&client, request) {
        Ok(line) => {
            print!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to call daemon IPC: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_daemon_client(endpoint: Option<IpcEndpoint>) -> Result<OsLocalIpcClient, CliRuntimeError> {
    build_daemon_client_with_resolver(endpoint, resolve_current_default_endpoint)
}

fn build_daemon_client_with_resolver<F>(
    endpoint: Option<IpcEndpoint>,
    resolve_default_endpoint: F,
) -> Result<OsLocalIpcClient, CliRuntimeError>
where
    F: FnOnce() -> Result<IpcEndpoint, IpcEndpointError>,
{
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None => resolve_default_endpoint().map_err(CliRuntimeError::from)?,
    };

    Ok(OsLocalIpcClient::new(endpoint))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointOptions {
    endpoint: Option<IpcEndpoint>,
}

fn parse_endpoint_options<I>(args: I) -> Result<EndpointOptions, CliUsageError>
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

    Ok(EndpointOptions { endpoint })
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliRuntimeError {
    InvalidEndpoint(String),
}

impl From<akraz_ipc::IpcEndpointError> for CliRuntimeError {
    fn from(error: akraz_ipc::IpcEndpointError) -> Self {
        Self::InvalidEndpoint(error.to_string())
    }
}

impl Display for CliRuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use akraz_ipc::{
        IpcEndpoint, IpcEndpointError, IpcEndpointKind, JsonRpcRequest, LocalIpcClient,
    };

    use super::{
        CliRuntimeError, CliUsageError, EndpointOptions, LOCAL_REQUEST_ID, METHOD_DAEMON_STATUS,
        build_daemon_client_with_resolver, parse_endpoint_options,
    };

    #[test]
    fn parses_endpoint_option() {
        assert_eq!(
            parse_endpoint_options(["--endpoint", "local-test"].map(String::from)),
            Ok(EndpointOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
            })
        );
        assert_eq!(
            parse_endpoint_options(["--endpoint=local-test"].map(String::from)),
            Ok(EndpointOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
            })
        );
    }

    #[test]
    fn rejects_invalid_endpoint_options() {
        assert_eq!(
            parse_endpoint_options(["--endpoint"].map(String::from)),
            Err(CliUsageError::MissingEndpointValue)
        );
        assert_eq!(
            parse_endpoint_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownStatusOption("--bad".to_string()))
        );
    }

    #[test]
    fn status_request_uses_daemon_status_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DAEMON_STATUS,
            akraz_ipc::DaemonStatusParams::default(),
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_DAEMON_STATUS);
    }

    #[test]
    fn default_daemon_client_resolves_os_endpoint() {
        let endpoint = match IpcEndpoint::manual("resolved-endpoint") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let expected_endpoint = endpoint.clone();
        let client = match build_daemon_client_with_resolver(None, || Ok(endpoint)) {
            Ok(client) => client,
            Err(error) => panic!("expected daemon IPC client: {error}"),
        };

        assert_eq!(client.endpoint(), &expected_endpoint);
    }

    #[test]
    fn default_daemon_client_reports_endpoint_resolution_errors() {
        let error = build_daemon_client_with_resolver(None, || {
            Err(IpcEndpointError::UnsupportedOperatingSystem)
        });

        match error {
            Err(CliRuntimeError::InvalidEndpoint(message)) => {
                assert_eq!(message, "unsupported operating system for local IPC")
            }
            Ok(client) => panic!("expected endpoint resolution failure, got {client:?}"),
        }
    }

    #[test]
    fn explicit_endpoint_selects_os_ipc_client_without_resolving_default() {
        let endpoint = match IpcEndpoint::manual("explicit-endpoint") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let client = match build_daemon_client_with_resolver(Some(endpoint.clone()), || {
            Err(IpcEndpointError::UnsupportedOperatingSystem)
        }) {
            Ok(client) => client,
            Err(error) => panic!("expected daemon IPC client: {error}"),
        };

        assert_eq!(client.endpoint(), &endpoint);
    }
}

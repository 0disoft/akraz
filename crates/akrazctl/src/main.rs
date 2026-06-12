use std::env;
use std::fmt::{Display, Formatter};
use std::process::ExitCode;

use akraz_core::RuntimeInputState;
use akraz_daemon::DaemonIpcServer;
use akraz_ipc::{
    DaemonStatusParams, InProcessIpcClient, IpcEndpoint, JsonRpcRequest, METHOD_DAEMON_STATUS,
    METHOD_PERMISSIONS_PROBE, PermissionsProbeParams, call_json_rpc,
};
use akraz_platform::FakePlatformAdapter;

const LOCAL_REQUEST_ID: &str = "local";
const LOCAL_DIAGNOSTIC_ENDPOINT: &str = "in-process://akrazd";

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
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_permissions_probe() -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_PERMISSIONS_PROBE,
        PermissionsProbeParams::default(),
    );

    print_local_daemon_response(None, &request)
}

fn print_local_daemon_response<P>(
    endpoint: Option<IpcEndpoint>,
    request: &JsonRpcRequest<P>,
) -> ExitCode
where
    P: serde::Serialize,
{
    let client = match build_local_diagnostic_client(endpoint) {
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

fn build_local_diagnostic_client(
    endpoint: Option<IpcEndpoint>,
) -> Result<InProcessIpcClient<DaemonIpcServer<FakePlatformAdapter>>, CliRuntimeError> {
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None => IpcEndpoint::manual(LOCAL_DIAGNOSTIC_ENDPOINT).map_err(CliRuntimeError::from)?,
    };
    let server = DaemonIpcServer::new(RuntimeInputState::new(), FakePlatformAdapter::default());

    Ok(InProcessIpcClient::new(endpoint, server))
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
        DaemonStatus, IpcEndpoint, IpcEndpointKind, JsonRpcRequest, JsonRpcSuccess, LocalIpcClient,
        METHOD_DAEMON_STATUS, call_json_rpc,
    };

    use super::{
        CliUsageError, LOCAL_REQUEST_ID, StatusOptions, build_local_diagnostic_client,
        parse_status_options,
    };

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
    fn local_diagnostic_client_calls_daemon_through_ipc_transport() {
        let endpoint = match IpcEndpoint::manual("in-process://test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let client = match build_local_diagnostic_client(Some(endpoint.clone())) {
            Ok(client) => client,
            Err(error) => panic!("expected diagnostic client: {error}"),
        };
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_DAEMON_STATUS,
            akraz_ipc::DaemonStatusParams::default(),
        );

        let response_line = match call_json_rpc(&client, &request) {
            Ok(line) => line,
            Err(error) => panic!("expected daemon IPC response: {error}"),
        };
        let response: JsonRpcSuccess<DaemonStatus> = match serde_json::from_str(&response_line) {
            Ok(response) => response,
            Err(error) => panic!("expected daemon status response JSON: {error}"),
        };

        assert_eq!(client.endpoint(), &endpoint);
        assert_eq!(response.id, LOCAL_REQUEST_ID);
        assert_eq!(response.result.daemon_version, env!("CARGO_PKG_VERSION"));
    }
}

use std::env;
use std::fmt::{Display, Formatter};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use akraz_identity::{
    FileIdentityStore, IdentityDocumentError, IdentityStoreError, PairingIdentityDocument,
    TrustedPeerIdentity,
};
use akraz_ipc::{
    DaemonLogEntry, DaemonLogsTail, DaemonLogsTailParams, DaemonStatus, DaemonStatusParams,
    DiagnosticsKeyboardLayout, DiagnosticsKeyboardLayoutParams, DiagnosticsScreenTopology,
    DiagnosticsScreenTopologyParams, DiagnosticsSnapshot, InputReleaseAllParams, IpcCallError,
    IpcEndpoint, IpcEndpointError, IpcTransportError, JSONRPC_VERSION, JsonRpcFailure,
    JsonRpcRequest, JsonRpcSuccess, METHOD_DAEMON_LOGS_TAIL, METHOD_DAEMON_STATUS,
    METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT, METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
    METHOD_INPUT_RELEASE_ALL, METHOD_PERMISSIONS_PROBE, METHOD_SESSION_CONNECT,
    METHOD_SESSION_DISCONNECT, OsLocalIpcClient, PermissionsProbe, PermissionsProbeParams,
    SessionConnectParams, SessionDisconnectParams, build_diagnostics_latency_histogram,
    build_diagnostics_snapshot, build_diagnostics_support_bundle, call_json_rpc,
    resolve_current_default_endpoint,
};
use akraz_protocol::CapabilityFlags;

const LOCAL_REQUEST_ID: &str = "local";
const DEFAULT_IDENTITY_DISPLAY_NAME: &str = "Akraz Device";

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
        Some("diagnostics") => match args.next().as_deref() {
            Some("snapshot") => match parse_endpoint_options(args) {
                Ok(options) => print_diagnostics_snapshot(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some("bundle") => match parse_endpoint_options(args) {
                Ok(options) => print_diagnostics_bundle(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown diagnostics command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing diagnostics command");
                ExitCode::from(2)
            }
        },
        Some("daemon-args") => match parse_daemon_args_options(args) {
            Ok(options) => print_daemon_args(options),
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
        Some("input") => match args.next().as_deref() {
            Some("release-all") => match parse_endpoint_options(args) {
                Ok(options) => print_input_release_all(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown input command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing input command");
                ExitCode::from(2)
            }
        },
        Some("identity") => match args.next().as_deref() {
            Some("show") => match parse_identity_show_options(args) {
                Ok(options) => print_identity_show(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some("list") => match parse_identity_list_options(args) {
                Ok(options) => print_identity_list(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some("trust") => match parse_identity_trust_options(args) {
                Ok(options) => print_identity_trust(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some("forget") => match parse_identity_forget_options(args) {
                Ok(options) => print_identity_forget(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown identity command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing identity command");
                ExitCode::from(2)
            }
        },
        Some("session") => match args.next().as_deref() {
            Some("connect") => match parse_session_connect_options(args) {
                Ok(options) => print_session_connect(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some("disconnect") => match parse_endpoint_options(args) {
                Ok(options) => print_session_disconnect(options),
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            },
            Some(argument) => {
                eprintln!("unknown session command: {argument}");
                ExitCode::from(2)
            }
            None => {
                eprintln!("missing session command");
                ExitCode::from(2)
            }
        },
        Some(argument) => {
            eprintln!("unknown command: {argument}");
            ExitCode::from(2)
        }
        None => {
            eprintln!(
                "usage: akrazctl <status|diagnostics snapshot|diagnostics bundle|permissions probe|input release-all|identity show|identity list|identity trust|identity forget|session connect|session disconnect|daemon-args|--version>"
            );
            ExitCode::from(2)
        }
    }
}

fn print_version() {
    println!("akrazctl {}", env!("CARGO_PKG_VERSION"));
}

fn print_status(options: EndpointOptions) -> ExitCode {
    let request = daemon_status_request();
    print_local_daemon_response(options.endpoint, &request)
}

fn print_permissions_probe(options: EndpointOptions) -> ExitCode {
    let request = permissions_probe_request();
    print_local_daemon_response(options.endpoint, &request)
}

fn print_input_release_all(options: EndpointOptions) -> ExitCode {
    let request = input_release_all_request();
    print_local_daemon_response(options.endpoint, &request)
}

fn print_session_connect(options: SessionConnectOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_SESSION_CONNECT,
        SessionConnectParams {
            peer_id: options.peer_id,
            local_device_id: options.local_device_id,
            address: options.address,
        },
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_session_disconnect(options: EndpointOptions) -> ExitCode {
    let request = JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_SESSION_DISCONNECT,
        SessionDisconnectParams::default(),
    );

    print_local_daemon_response(options.endpoint, &request)
}

fn print_daemon_args(options: DaemonArgsOptions) -> ExitCode {
    println!("{}", format_daemon_command_line(&options));
    ExitCode::SUCCESS
}

fn print_diagnostics_snapshot(options: EndpointOptions) -> ExitCode {
    let client = match build_daemon_client(options.endpoint) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let snapshot = match collect_diagnostics_snapshot(&client) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    print_json_pretty(&snapshot)
}

fn print_diagnostics_bundle(options: EndpointOptions) -> ExitCode {
    let client = match build_daemon_client(options.endpoint) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let snapshot = match collect_diagnostics_snapshot(&client) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    print_json_pretty(&build_diagnostics_support_bundle(
        snapshot,
        collect_recent_daemon_logs(&client),
        "akrazctl",
        env!("CARGO_PKG_VERSION"),
    ))
}

fn collect_diagnostics_snapshot(client: &OsLocalIpcClient) -> Result<DiagnosticsSnapshot, String> {
    let mut latency_samples = Vec::new();
    let (status, status_latency) =
        call_daemon_json_rpc_timed::<DaemonStatus, _>(client, &daemon_status_request(), "status")?;
    latency_samples.push(status_latency);
    let (permissions, permissions_latency) = call_daemon_json_rpc_timed::<PermissionsProbe, _>(
        client,
        &permissions_probe_request(),
        "permissions probe",
    )?;
    latency_samples.push(permissions_latency);
    let screen_topology = call_daemon_json_rpc_timed::<DiagnosticsScreenTopology, _>(
        client,
        &diagnostics_screen_topology_request(),
        "screen topology",
    )
    .map(|(topology, latency)| {
        latency_samples.push(latency);
        topology
    })
    .ok();
    let keyboard_layout = call_daemon_json_rpc_timed::<DiagnosticsKeyboardLayout, _>(
        client,
        &diagnostics_keyboard_layout_request(),
        "keyboard layout",
    )
    .map(|(keyboard_layout, latency)| {
        latency_samples.push(latency);
        keyboard_layout
    })
    .ok();

    Ok(build_diagnostics_snapshot(
        status,
        permissions,
        screen_topology,
        keyboard_layout,
        build_diagnostics_latency_histogram(&latency_samples),
        "akrazctl",
        env!("CARGO_PKG_VERSION"),
    ))
}

fn collect_recent_daemon_logs(client: &OsLocalIpcClient) -> Vec<DaemonLogEntry> {
    call_daemon_json_rpc::<DaemonLogsTail, _>(
        client,
        &daemon_logs_tail_request(),
        "daemon logs tail",
    )
    .map(|tail| tail.entries)
    .unwrap_or_default()
}

fn print_identity_show(options: IdentityShowOptions) -> ExitCode {
    match build_pairing_identity_document(&options) {
        Ok(document) => print_json_pretty(&document),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_identity_list(options: IdentityListOptions) -> ExitCode {
    match list_trusted_peer_identities(&options) {
        Ok(result) => print_json_pretty(&result),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_identity_trust(options: IdentityTrustOptions) -> ExitCode {
    match trust_pairing_identity_document(&options) {
        Ok(result) => print_json_pretty(&result),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_identity_forget(options: IdentityForgetOptions) -> ExitCode {
    match forget_trusted_peer_identity(&options) {
        Ok(result) => print_json_pretty(&result),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_json_pretty<T>(value: &T) -> ExitCode
where
    T: serde::Serialize,
{
    match serde_json::to_string_pretty(value) {
        Ok(encoded) => {
            println!("{encoded}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to encode JSON output: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build_pairing_identity_document(
    options: &IdentityShowOptions,
) -> Result<PairingIdentityDocument, CliRuntimeError> {
    let store = FileIdentityStore::new(&options.identity_store);
    let identity = store
        .load_or_create(&options.identity_display_name)
        .map_err(|source| CliRuntimeError::IdentityStore {
            operation: "load identity",
            path: options.identity_store.clone(),
            source: Box::new(source),
        })?;

    Ok(PairingIdentityDocument::from_device_identity(
        identity.identity(),
        default_pairing_capabilities(),
    ))
}

fn list_trusted_peer_identities(
    options: &IdentityListOptions,
) -> Result<IdentityTrustedPeersResult, CliRuntimeError> {
    let store = FileIdentityStore::new(&options.identity_store);
    let peers = store
        .list_trusted_peers()
        .map_err(|source| CliRuntimeError::IdentityStore {
            operation: "list trusted peers",
            path: options.identity_store.clone(),
            source: Box::new(source),
        })?;

    Ok(IdentityTrustedPeersResult {
        peers: peers.into_iter().map(IdentityTrustedPeer::from).collect(),
    })
}

fn trust_pairing_identity_document(
    options: &IdentityTrustOptions,
) -> Result<IdentityTrustResult, CliRuntimeError> {
    let store = FileIdentityStore::new(&options.identity_store);
    store
        .load_or_create(&options.identity_display_name)
        .map_err(|source| CliRuntimeError::IdentityStore {
            operation: "load identity",
            path: options.identity_store.clone(),
            source: Box::new(source),
        })?;
    let contents =
        fs::read_to_string(&options.peer_file).map_err(|source| CliRuntimeError::ReadPeerFile {
            path: options.peer_file.clone(),
            source: Box::new(source),
        })?;
    let document: PairingIdentityDocument =
        serde_json::from_str(&contents).map_err(CliRuntimeError::DecodePairingDocument)?;
    let peer = document
        .into_trusted_peer_identity()
        .map_err(|source| CliRuntimeError::InvalidPairingDocument(Box::new(source)))?;

    store
        .save_trusted_peer(&peer)
        .map_err(|source| CliRuntimeError::IdentityStore {
            operation: "save trusted peer",
            path: options.identity_store.clone(),
            source: Box::new(source),
        })?;

    Ok(IdentityTrustResult {
        trusted: true,
        peer_id: peer.peer_id().to_string(),
        display_name: peer.display_name().to_string(),
        fingerprint: peer.fingerprint().to_string(),
        capabilities: peer.capabilities(),
    })
}

fn forget_trusted_peer_identity(
    options: &IdentityForgetOptions,
) -> Result<IdentityForgetResult, CliRuntimeError> {
    let store = FileIdentityStore::new(&options.identity_store);
    store
        .remove_trusted_peer(&options.peer_id)
        .map_err(|source| CliRuntimeError::IdentityStore {
            operation: "remove trusted peer",
            path: options.identity_store.clone(),
            source: Box::new(source),
        })?;

    Ok(IdentityForgetResult {
        forgotten: true,
        peer_id: options.peer_id.clone(),
    })
}

fn default_pairing_capabilities() -> CapabilityFlags {
    CapabilityFlags::POINTER | CapabilityFlags::KEYBOARD
}

fn daemon_status_request() -> JsonRpcRequest<DaemonStatusParams> {
    JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_STATUS,
        DaemonStatusParams::default(),
    )
}

fn daemon_logs_tail_request() -> JsonRpcRequest<DaemonLogsTailParams> {
    JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DAEMON_LOGS_TAIL,
        DaemonLogsTailParams::default(),
    )
}

fn permissions_probe_request() -> JsonRpcRequest<PermissionsProbeParams> {
    JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_PERMISSIONS_PROBE,
        PermissionsProbeParams::default(),
    )
}

fn diagnostics_screen_topology_request() -> JsonRpcRequest<DiagnosticsScreenTopologyParams> {
    JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY,
        DiagnosticsScreenTopologyParams::default(),
    )
}

fn diagnostics_keyboard_layout_request() -> JsonRpcRequest<DiagnosticsKeyboardLayoutParams> {
    JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT,
        DiagnosticsKeyboardLayoutParams::default(),
    )
}

fn input_release_all_request() -> JsonRpcRequest<InputReleaseAllParams> {
    JsonRpcRequest::new(
        LOCAL_REQUEST_ID,
        METHOD_INPUT_RELEASE_ALL,
        InputReleaseAllParams::default(),
    )
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
            eprintln!("{}", format_daemon_call_error(&error));
            ExitCode::FAILURE
        }
    }
}

fn call_daemon_json_rpc<T, P>(
    client: &OsLocalIpcClient,
    request: &JsonRpcRequest<P>,
    label: &str,
) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
    P: serde::Serialize,
{
    let line = call_json_rpc(client, request).map_err(|error| format_daemon_call_error(&error))?;
    parse_json_rpc_response(&line, label)
}

fn call_daemon_json_rpc_timed<T, P>(
    client: &OsLocalIpcClient,
    request: &JsonRpcRequest<P>,
    label: &str,
) -> Result<(T, u128), String>
where
    T: for<'de> serde::Deserialize<'de>,
    P: serde::Serialize,
{
    let started = Instant::now();
    let result = call_daemon_json_rpc(client, request, label)?;
    Ok((result, started.elapsed().as_micros()))
}

fn parse_json_rpc_response<T>(response_line: &str, label: &str) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let value: serde_json::Value = serde_json::from_str(response_line.trim_end())
        .map_err(|error| format!("daemon returned invalid JSON: {error}"))?;

    if value.get("error").is_some() {
        let failure: JsonRpcFailure = serde_json::from_value(value)
            .map_err(|error| format!("daemon returned an invalid error response: {error}"))?;
        return Err(failure.error.message);
    }

    let success: JsonRpcSuccess<T> = serde_json::from_value(value)
        .map_err(|error| format!("daemon returned an invalid {label} response: {error}"))?;
    if success.jsonrpc != JSONRPC_VERSION {
        return Err(format!(
            "daemon returned unsupported JSON-RPC version: {}",
            success.jsonrpc
        ));
    }
    if success.id != LOCAL_REQUEST_ID {
        return Err(format!(
            "daemon returned unexpected response id: {}",
            success.id
        ));
    }

    Ok(success.result)
}

fn format_daemon_call_error(error: &IpcCallError) -> String {
    match error {
        IpcCallError::Transport {
            source: IpcTransportError::EndpointUnavailable { endpoint, message },
        } => format!(
            "akraz daemon is not reachable at {endpoint}. Start akraz-daemon, or pass --endpoint to use a different IPC endpoint. Details: {message}"
        ),
        IpcCallError::Transport {
            source: IpcTransportError::RequestFailed { message },
        } => format!("akraz daemon IPC request failed. Details: {message}"),
        IpcCallError::Encode { source } => {
            format!("failed to encode daemon IPC request: {source}")
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonArgsOptions {
    capture_input: bool,
    edge_bindings: Vec<String>,
    peer_listen: Option<String>,
    peer_session: Option<String>,
    local_device_id: Option<String>,
    identity_store: Option<String>,
    identity_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionConnectOptions {
    endpoint: Option<IpcEndpoint>,
    peer_id: String,
    local_device_id: String,
    address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityShowOptions {
    identity_store: PathBuf,
    identity_display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityListOptions {
    identity_store: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityTrustOptions {
    identity_store: PathBuf,
    identity_display_name: String,
    peer_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityForgetOptions {
    identity_store: PathBuf,
    peer_id: String,
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct IdentityTrustedPeer {
    peer_id: String,
    display_name: String,
    fingerprint: String,
    capabilities: CapabilityFlags,
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct IdentityTrustedPeersResult {
    peers: Vec<IdentityTrustedPeer>,
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct IdentityTrustResult {
    trusted: bool,
    peer_id: String,
    display_name: String,
    fingerprint: String,
    capabilities: CapabilityFlags,
}

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct IdentityForgetResult {
    forgotten: bool,
    peer_id: String,
}

impl From<TrustedPeerIdentity> for IdentityTrustedPeer {
    fn from(peer: TrustedPeerIdentity) -> Self {
        Self {
            peer_id: peer.peer_id().to_string(),
            display_name: peer.display_name().to_string(),
            fingerprint: peer.fingerprint().to_string(),
            capabilities: peer.capabilities(),
        }
    }
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
            return Err(CliUsageError::UnknownEndpointOption(argument));
        }
    }

    Ok(EndpointOptions { endpoint })
}

fn parse_identity_show_options<I>(args: I) -> Result<IdentityShowOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut identity_store = None;
    let mut identity_display_name = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--identity-store=") {
            set_once_path_option(
                "--identity-store",
                &mut identity_store,
                normalize_path_arg("--identity-store", value)?,
            )?;
        } else if argument == "--identity-store" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue(
                    "--identity-store",
                ))?;
            let value = normalize_path_arg("--identity-store", &value)?;
            set_once_path_option("--identity-store", &mut identity_store, value)?;
        } else if let Some(value) = argument.strip_prefix("--identity-display-name=") {
            set_once_identity_string_option(
                "--identity-display-name",
                &mut identity_display_name,
                normalize_display_name_arg(value)?,
            )?;
        } else if argument == "--identity-display-name" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue(
                    "--identity-display-name",
                ))?;
            let value = normalize_display_name_arg(&value)?;
            set_once_identity_string_option(
                "--identity-display-name",
                &mut identity_display_name,
                value,
            )?;
        } else {
            return Err(CliUsageError::UnknownIdentityOption(argument));
        }
    }

    Ok(IdentityShowOptions {
        identity_store: identity_store.ok_or(CliUsageError::MissingIdentityStore)?,
        identity_display_name: identity_display_name
            .unwrap_or_else(|| DEFAULT_IDENTITY_DISPLAY_NAME.to_string()),
    })
}

fn parse_identity_list_options<I>(args: I) -> Result<IdentityListOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut identity_store = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--identity-store=") {
            set_once_path_option(
                "--identity-store",
                &mut identity_store,
                normalize_path_arg("--identity-store", value)?,
            )?;
        } else if argument == "--identity-store" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue(
                    "--identity-store",
                ))?;
            let value = normalize_path_arg("--identity-store", &value)?;
            set_once_path_option("--identity-store", &mut identity_store, value)?;
        } else {
            return Err(CliUsageError::UnknownIdentityOption(argument));
        }
    }

    Ok(IdentityListOptions {
        identity_store: identity_store.ok_or(CliUsageError::MissingIdentityStore)?,
    })
}

fn parse_identity_trust_options<I>(args: I) -> Result<IdentityTrustOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut identity_store = None;
    let mut identity_display_name = None;
    let mut peer_file = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--identity-store=") {
            set_once_path_option(
                "--identity-store",
                &mut identity_store,
                normalize_path_arg("--identity-store", value)?,
            )?;
        } else if argument == "--identity-store" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue(
                    "--identity-store",
                ))?;
            let value = normalize_path_arg("--identity-store", &value)?;
            set_once_path_option("--identity-store", &mut identity_store, value)?;
        } else if let Some(value) = argument.strip_prefix("--identity-display-name=") {
            set_once_identity_string_option(
                "--identity-display-name",
                &mut identity_display_name,
                normalize_display_name_arg(value)?,
            )?;
        } else if argument == "--identity-display-name" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue(
                    "--identity-display-name",
                ))?;
            let value = normalize_display_name_arg(&value)?;
            set_once_identity_string_option(
                "--identity-display-name",
                &mut identity_display_name,
                value,
            )?;
        } else if let Some(value) = argument.strip_prefix("--peer-file=") {
            set_once_path_option(
                "--peer-file",
                &mut peer_file,
                normalize_path_arg("--peer-file", value)?,
            )?;
        } else if argument == "--peer-file" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue("--peer-file"))?;
            let value = normalize_path_arg("--peer-file", &value)?;
            set_once_path_option("--peer-file", &mut peer_file, value)?;
        } else {
            return Err(CliUsageError::UnknownIdentityOption(argument));
        }
    }

    Ok(IdentityTrustOptions {
        identity_store: identity_store.ok_or(CliUsageError::MissingIdentityStore)?,
        identity_display_name: identity_display_name
            .unwrap_or_else(|| DEFAULT_IDENTITY_DISPLAY_NAME.to_string()),
        peer_file: peer_file.ok_or(CliUsageError::MissingIdentityPeerFile)?,
    })
}

fn parse_identity_forget_options<I>(args: I) -> Result<IdentityForgetOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut identity_store = None;
    let mut peer_id = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--identity-store=") {
            set_once_path_option(
                "--identity-store",
                &mut identity_store,
                normalize_path_arg("--identity-store", value)?,
            )?;
        } else if argument == "--identity-store" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue(
                    "--identity-store",
                ))?;
            let value = normalize_path_arg("--identity-store", &value)?;
            set_once_path_option("--identity-store", &mut identity_store, value)?;
        } else if let Some(value) = argument.strip_prefix("--peer-id=") {
            set_once_identity_string_option(
                "--peer-id",
                &mut peer_id,
                normalize_identity_peer_id_arg(value)?,
            )?;
        } else if argument == "--peer-id" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingIdentityOptionValue("--peer-id"))?;
            let value = normalize_identity_peer_id_arg(&value)?;
            set_once_identity_string_option("--peer-id", &mut peer_id, value)?;
        } else {
            return Err(CliUsageError::UnknownIdentityOption(argument));
        }
    }

    Ok(IdentityForgetOptions {
        identity_store: identity_store.ok_or(CliUsageError::MissingIdentityStore)?,
        peer_id: peer_id.ok_or(CliUsageError::MissingIdentityPeerId)?,
    })
}

fn parse_session_connect_options<I>(args: I) -> Result<SessionConnectOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut endpoint = None;
    let mut peer_id = None;
    let mut local_device_id = None;
    let mut address = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--endpoint=") {
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else if argument == "--endpoint" {
            let value = args.next().ok_or(CliUsageError::MissingEndpointValue)?;
            endpoint = Some(IpcEndpoint::manual(value).map_err(CliUsageError::from)?);
        } else if let Some(value) = argument.strip_prefix("--peer-id=") {
            set_once_daemon_option(
                "--peer-id",
                &mut peer_id,
                normalize_peer_id("--peer-id", value)?,
            )?;
        } else if argument == "--peer-id" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingSessionOptionValue("--peer-id"))?;
            let value = normalize_peer_id("--peer-id", &value)?;
            set_once_daemon_option("--peer-id", &mut peer_id, value)?;
        } else if let Some(value) = argument.strip_prefix("--local-device-id=") {
            set_once_daemon_option(
                "--local-device-id",
                &mut local_device_id,
                normalize_local_device_id_arg(value)?,
            )?;
        } else if argument == "--local-device-id" {
            let value = args.next().ok_or(CliUsageError::MissingSessionOptionValue(
                "--local-device-id",
            ))?;
            let value = normalize_local_device_id_arg(&value)?;
            set_once_daemon_option("--local-device-id", &mut local_device_id, value)?;
        } else if let Some(value) = argument.strip_prefix("--address=") {
            set_once_daemon_option(
                "--address",
                &mut address,
                normalize_socket_addr_arg("--address", value)?,
            )?;
        } else if argument == "--address" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingSessionOptionValue("--address"))?;
            let value = normalize_socket_addr_arg("--address", &value)?;
            set_once_daemon_option("--address", &mut address, value)?;
        } else {
            return Err(CliUsageError::UnknownSessionOption(argument));
        }
    }

    Ok(SessionConnectOptions {
        endpoint,
        peer_id: peer_id.ok_or(CliUsageError::MissingSessionConnectPeerId)?,
        local_device_id: local_device_id
            .ok_or(CliUsageError::MissingSessionConnectLocalDeviceId)?,
        address: address.ok_or(CliUsageError::MissingSessionConnectAddress)?,
    })
}

fn parse_daemon_args_options<I>(args: I) -> Result<DaemonArgsOptions, CliUsageError>
where
    I: IntoIterator<Item = String>,
{
    let mut options = DaemonArgsOptions {
        capture_input: false,
        edge_bindings: Vec::new(),
        peer_listen: None,
        peer_session: None,
        local_device_id: None,
        identity_store: None,
        identity_display_name: None,
    };
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        if argument == "--capture-input" {
            options.capture_input = true;
        } else if let Some(value) = argument.strip_prefix("--edge-binding=") {
            options
                .edge_bindings
                .push(normalize_edge_binding_arg(value)?);
        } else if argument == "--edge-binding" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--edge-binding"))?;
            options
                .edge_bindings
                .push(normalize_edge_binding_arg(&value)?);
        } else if let Some(value) = argument.strip_prefix("--peer-listen=") {
            set_once_daemon_option(
                "--peer-listen",
                &mut options.peer_listen,
                normalize_socket_addr_arg("--peer-listen", value)?,
            )?;
        } else if argument == "--peer-listen" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--peer-listen"))?;
            let value = normalize_socket_addr_arg("--peer-listen", &value)?;
            set_once_daemon_option("--peer-listen", &mut options.peer_listen, value)?;
        } else if let Some(value) = argument.strip_prefix("--peer-session=") {
            set_once_daemon_option(
                "--peer-session",
                &mut options.peer_session,
                normalize_peer_session_arg(value)?,
            )?;
        } else if argument == "--peer-session" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--peer-session"))?;
            let value = normalize_peer_session_arg(&value)?;
            set_once_daemon_option("--peer-session", &mut options.peer_session, value)?;
        } else if let Some(value) = argument.strip_prefix("--local-device-id=") {
            set_once_daemon_option(
                "--local-device-id",
                &mut options.local_device_id,
                normalize_local_device_id_arg(value)?,
            )?;
        } else if argument == "--local-device-id" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--local-device-id"))?;
            let value = normalize_local_device_id_arg(&value)?;
            set_once_daemon_option("--local-device-id", &mut options.local_device_id, value)?;
        } else if let Some(value) = argument.strip_prefix("--identity-store=") {
            set_once_daemon_option(
                "--identity-store",
                &mut options.identity_store,
                normalize_identity_store_arg(value)?,
            )?;
        } else if argument == "--identity-store" {
            let value = args
                .next()
                .ok_or(CliUsageError::MissingDaemonOptionValue("--identity-store"))?;
            let value = normalize_identity_store_arg(&value)?;
            set_once_daemon_option("--identity-store", &mut options.identity_store, value)?;
        } else if let Some(value) = argument.strip_prefix("--identity-display-name=") {
            set_once_daemon_option(
                "--identity-display-name",
                &mut options.identity_display_name,
                normalize_identity_display_name_arg(value)?,
            )?;
        } else if argument == "--identity-display-name" {
            let value = args.next().ok_or(CliUsageError::MissingDaemonOptionValue(
                "--identity-display-name",
            ))?;
            let value = normalize_identity_display_name_arg(&value)?;
            set_once_daemon_option(
                "--identity-display-name",
                &mut options.identity_display_name,
                value,
            )?;
        } else {
            return Err(CliUsageError::UnknownDaemonArgsOption(argument));
        }
    }

    if (options.peer_listen.is_some() || options.peer_session.is_some())
        && options.identity_store.is_none()
    {
        return Err(CliUsageError::PeerTransportRequiresIdentityStore);
    }

    Ok(options)
}

fn set_once_daemon_option(
    option_name: &'static str,
    target: &mut Option<String>,
    value: String,
) -> Result<(), CliUsageError> {
    if target.is_some() {
        return Err(CliUsageError::DuplicateDaemonOption(option_name));
    }

    *target = Some(value);
    Ok(())
}

fn set_once_path_option(
    option_name: &'static str,
    target: &mut Option<PathBuf>,
    value: PathBuf,
) -> Result<(), CliUsageError> {
    if target.is_some() {
        return Err(CliUsageError::DuplicateIdentityOption(option_name));
    }

    *target = Some(value);
    Ok(())
}

fn set_once_identity_string_option(
    option_name: &'static str,
    target: &mut Option<String>,
    value: String,
) -> Result<(), CliUsageError> {
    if target.is_some() {
        return Err(CliUsageError::DuplicateIdentityOption(option_name));
    }

    *target = Some(value);
    Ok(())
}

fn normalize_edge_binding_arg(value: &str) -> Result<String, CliUsageError> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: "--edge-binding",
            value: value.to_string(),
            reason: "expected <local-edge>:<peer-id>:<remote-edge>".to_string(),
        });
    }

    let local_edge = normalize_screen_edge("--edge-binding", parts[0])?;
    let peer_id = normalize_peer_id("--edge-binding", parts[1])?;
    let remote_edge = normalize_screen_edge("--edge-binding", parts[2])?;

    Ok(format!("{local_edge}:{peer_id}:{remote_edge}"))
}

fn normalize_screen_edge(option_name: &'static str, value: &str) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg(option_name, value)?;
    match value.to_ascii_lowercase().as_str() {
        "left" => Ok("left".to_string()),
        "right" => Ok("right".to_string()),
        "top" => Ok("top".to_string()),
        "bottom" => Ok("bottom".to_string()),
        _ => Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value,
            reason: "edge must be one of left, right, top, bottom".to_string(),
        }),
    }
}

fn normalize_peer_id(option_name: &'static str, value: &str) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg(option_name, value)?;
    if value.contains(':') || value.contains('@') {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value,
            reason: "peer id must not contain ':' or '@'".to_string(),
        });
    }

    Ok(value)
}

fn normalize_socket_addr_arg(
    option_name: &'static str,
    value: &str,
) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg(option_name, value)?;
    if value.parse::<SocketAddr>().is_err() {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value,
            reason: "expected <ip>:<port> socket address".to_string(),
        });
    }

    Ok(value)
}

fn normalize_peer_session_arg(value: &str) -> Result<String, CliUsageError> {
    let value = normalize_shell_safe_arg("--peer-session", value)?;
    let (peer_id, address) =
        value
            .split_once('@')
            .ok_or_else(|| CliUsageError::InvalidDaemonOptionValue {
                option: "--peer-session",
                value: value.clone(),
                reason: "expected <peer-id>@<ip>:<port>".to_string(),
            })?;
    if address.contains('@') {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: "--peer-session",
            value,
            reason: "expected exactly one '@' separator".to_string(),
        });
    }

    let peer_id = normalize_peer_id("--peer-session", peer_id)?;
    let address = normalize_socket_addr_arg("--peer-session", address)?;

    Ok(format!("{peer_id}@{address}"))
}

fn normalize_local_device_id_arg(value: &str) -> Result<String, CliUsageError> {
    normalize_shell_safe_arg("--local-device-id", value)
}

fn normalize_identity_store_arg(value: &str) -> Result<String, CliUsageError> {
    normalize_shell_safe_arg("--identity-store", value)
}

fn normalize_identity_display_name_arg(value: &str) -> Result<String, CliUsageError> {
    normalize_shell_safe_arg("--identity-display-name", value)
}

fn normalize_path_arg(option_name: &'static str, value: &str) -> Result<PathBuf, CliUsageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CliUsageError::InvalidIdentityOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "path must not be empty".to_string(),
        });
    }

    Ok(PathBuf::from(value))
}

fn normalize_display_name_arg(value: &str) -> Result<String, CliUsageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CliUsageError::InvalidIdentityOptionValue {
            option: "--identity-display-name",
            value: value.to_string(),
            reason: "display name must not be empty".to_string(),
        });
    }

    Ok(value.to_string())
}

fn normalize_identity_peer_id_arg(value: &str) -> Result<String, CliUsageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CliUsageError::InvalidIdentityOptionValue {
            option: "--peer-id",
            value: value.to_string(),
            reason: "peer id must not be empty".to_string(),
        });
    }
    if value.contains(':') || value.contains('@') {
        return Err(CliUsageError::InvalidIdentityOptionValue {
            option: "--peer-id",
            value: value.to_string(),
            reason: "peer id must not contain ':' or '@'".to_string(),
        });
    }

    Ok(value.to_string())
}

fn normalize_shell_safe_arg(
    option_name: &'static str,
    value: &str,
) -> Result<String, CliUsageError> {
    if value.is_empty() {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "value must not be empty".to_string(),
        });
    }
    if value.trim() != value {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "value must not start or end with whitespace".to_string(),
        });
    }
    if value.chars().any(char::is_whitespace) {
        return Err(CliUsageError::InvalidDaemonOptionValue {
            option: option_name,
            value: value.to_string(),
            reason: "value must not contain whitespace".to_string(),
        });
    }

    Ok(value.to_string())
}

fn format_daemon_command_line(options: &DaemonArgsOptions) -> String {
    let mut command = vec!["akraz-daemon".to_string(), "--serve".to_string()];
    if options.capture_input {
        command.push("--capture-input".to_string());
    }
    for edge_binding in &options.edge_bindings {
        command.push("--edge-binding".to_string());
        command.push(edge_binding.clone());
    }
    if let Some(peer_listen) = &options.peer_listen {
        command.push("--peer-listen".to_string());
        command.push(peer_listen.clone());
    }
    if let Some(local_device_id) = &options.local_device_id {
        command.push("--local-device-id".to_string());
        command.push(local_device_id.clone());
    }
    if let Some(identity_store) = &options.identity_store {
        command.push("--identity-store".to_string());
        command.push(identity_store.clone());
    }
    if let Some(identity_display_name) = &options.identity_display_name {
        command.push("--identity-display-name".to_string());
        command.push(identity_display_name.clone());
    }
    if let Some(peer_session) = &options.peer_session {
        command.push("--peer-session".to_string());
        command.push(peer_session.clone());
    }

    command.join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliUsageError {
    MissingEndpointValue,
    MissingDaemonOptionValue(&'static str),
    MissingIdentityOptionValue(&'static str),
    MissingSessionOptionValue(&'static str),
    MissingIdentityStore,
    MissingIdentityPeerFile,
    MissingIdentityPeerId,
    MissingSessionConnectPeerId,
    MissingSessionConnectLocalDeviceId,
    MissingSessionConnectAddress,
    DuplicateDaemonOption(&'static str),
    DuplicateIdentityOption(&'static str),
    PeerTransportRequiresIdentityStore,
    InvalidEndpoint(String),
    InvalidDaemonOptionValue {
        option: &'static str,
        value: String,
        reason: String,
    },
    InvalidIdentityOptionValue {
        option: &'static str,
        value: String,
        reason: String,
    },
    UnknownEndpointOption(String),
    UnknownIdentityOption(String),
    UnknownSessionOption(String),
    UnknownDaemonArgsOption(String),
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
            Self::MissingDaemonOptionValue(option) => {
                write!(formatter, "missing value for {option}")
            }
            Self::MissingIdentityOptionValue(option) => {
                write!(formatter, "missing value for {option}")
            }
            Self::MissingSessionOptionValue(option) => {
                write!(formatter, "missing value for {option}")
            }
            Self::MissingIdentityStore => {
                formatter.write_str("identity command requires --identity-store")
            }
            Self::MissingIdentityPeerFile => {
                formatter.write_str("identity trust requires --peer-file")
            }
            Self::MissingIdentityPeerId => {
                formatter.write_str("identity forget requires --peer-id")
            }
            Self::MissingSessionConnectPeerId => {
                formatter.write_str("session connect requires --peer-id")
            }
            Self::MissingSessionConnectLocalDeviceId => {
                formatter.write_str("session connect requires --local-device-id")
            }
            Self::MissingSessionConnectAddress => {
                formatter.write_str("session connect requires --address")
            }
            Self::DuplicateDaemonOption(option) => {
                write!(formatter, "{option} can only be provided once")
            }
            Self::DuplicateIdentityOption(option) => {
                write!(formatter, "{option} can only be provided once")
            }
            Self::PeerTransportRequiresIdentityStore => {
                formatter.write_str("--peer-listen and --peer-session require --identity-store")
            }
            Self::InvalidEndpoint(error) => write!(formatter, "invalid endpoint: {error}"),
            Self::InvalidDaemonOptionValue {
                option,
                value,
                reason,
            } => {
                write!(formatter, "invalid value for {option}: {value} ({reason})")
            }
            Self::InvalidIdentityOptionValue {
                option,
                value,
                reason,
            } => {
                write!(formatter, "invalid value for {option}: {value} ({reason})")
            }
            Self::UnknownEndpointOption(argument) => {
                write!(formatter, "unknown endpoint option: {argument}")
            }
            Self::UnknownIdentityOption(argument) => {
                write!(formatter, "unknown identity option: {argument}")
            }
            Self::UnknownSessionOption(argument) => {
                write!(formatter, "unknown session option: {argument}")
            }
            Self::UnknownDaemonArgsOption(argument) => {
                write!(formatter, "unknown daemon-args option: {argument}")
            }
        }
    }
}

#[derive(Debug)]
enum CliRuntimeError {
    InvalidEndpoint(String),
    IdentityStore {
        operation: &'static str,
        path: PathBuf,
        source: Box<IdentityStoreError>,
    },
    ReadPeerFile {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
    DecodePairingDocument(serde_json::Error),
    InvalidPairingDocument(Box<IdentityDocumentError>),
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
            Self::IdentityStore {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} at {}: {source}",
                path.display()
            ),
            Self::ReadPeerFile { path, source } => {
                write!(
                    formatter,
                    "failed to read peer identity file {}: {source}",
                    path.display()
                )
            }
            Self::DecodePairingDocument(source) => {
                write!(formatter, "failed to decode peer identity JSON: {source}")
            }
            Self::InvalidPairingDocument(source) => {
                write!(formatter, "invalid peer identity document: {source}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use akraz_identity::FileIdentityStore;
    use akraz_ipc::{
        ControlModeSnapshot, DaemonStatus, DiagnosticsKeyboardLayout, DiagnosticsMonitorSnapshot,
        DiagnosticsScreenTopology, IpcCallError, IpcEndpoint, IpcEndpointError, IpcEndpointKind,
        IpcPlatformCapabilities, IpcTransportError, JsonRpcError, JsonRpcFailure, JsonRpcRequest,
        JsonRpcSuccess, LocalIpcClient, LogicalPointSnapshot, LogicalRectSnapshot, PeerStatus,
        PermissionIssue, PermissionsProbe, ProtocolVersionSnapshot, build_diagnostics_snapshot,
    };

    use super::{
        CliRuntimeError, CliUsageError, DaemonArgsOptions, EndpointOptions, IdentityForgetOptions,
        IdentityListOptions, IdentityShowOptions, IdentityTrustOptions, LOCAL_REQUEST_ID,
        METHOD_DAEMON_LOGS_TAIL, METHOD_DAEMON_STATUS, METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT,
        METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY, METHOD_SESSION_CONNECT, METHOD_SESSION_DISCONNECT,
        SessionConnectOptions, build_daemon_client_with_resolver, build_pairing_identity_document,
        daemon_logs_tail_request, daemon_status_request, default_pairing_capabilities,
        diagnostics_keyboard_layout_request, diagnostics_screen_topology_request,
        forget_trusted_peer_identity, format_daemon_call_error, format_daemon_command_line,
        input_release_all_request, list_trusted_peer_identities, parse_daemon_args_options,
        parse_endpoint_options, parse_identity_forget_options, parse_identity_list_options,
        parse_identity_show_options, parse_identity_trust_options, parse_json_rpc_response,
        parse_session_connect_options, permissions_probe_request, trust_pairing_identity_document,
    };
    use akraz_ipc::{
        JSONRPC_ERROR_PARSE, JSONRPC_VERSION, METHOD_INPUT_RELEASE_ALL, METHOD_PERMISSIONS_PROBE,
    };

    fn monitor_snapshots() -> Vec<DiagnosticsMonitorSnapshot> {
        vec![DiagnosticsMonitorSnapshot {
            id: "primary".to_string(),
            bounds: LogicalRectSnapshot {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            scale_factor_percent: Some(100),
            is_primary: true,
        }]
    }

    fn keyboard_layout() -> DiagnosticsKeyboardLayout {
        DiagnosticsKeyboardLayout {
            source: "foregroundWindowThread".to_string(),
            layout_id: "0x0000000004120412".to_string(),
            language_id: "0x0412".to_string(),
            layout_name: Some("00000412".to_string()),
        }
    }

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
            Err(CliUsageError::UnknownEndpointOption("--bad".to_string()))
        );
    }

    #[test]
    fn parses_identity_show_options() {
        assert_eq!(
            parse_identity_show_options(
                [
                    "--identity-store",
                    "akraz identity.json",
                    "--identity-display-name=Windows Desktop"
                ]
                .map(String::from)
            ),
            Ok(IdentityShowOptions {
                identity_store: PathBuf::from("akraz identity.json"),
                identity_display_name: "Windows Desktop".to_string(),
            })
        );
        assert_eq!(
            parse_identity_show_options(["--identity-store=akraz-identity.json"].map(String::from)),
            Ok(IdentityShowOptions {
                identity_store: PathBuf::from("akraz-identity.json"),
                identity_display_name: "Akraz Device".to_string(),
            })
        );
    }

    #[test]
    fn parses_identity_trust_options() {
        assert_eq!(
            parse_identity_trust_options(
                [
                    "--identity-store",
                    "akraz-identity.json",
                    "--peer-file",
                    "peer identity.json",
                ]
                .map(String::from)
            ),
            Ok(IdentityTrustOptions {
                identity_store: PathBuf::from("akraz-identity.json"),
                identity_display_name: "Akraz Device".to_string(),
                peer_file: PathBuf::from("peer identity.json"),
            })
        );
    }

    #[test]
    fn parses_identity_list_options() {
        assert_eq!(
            parse_identity_list_options(
                ["--identity-store", "akraz identity.json"].map(String::from)
            ),
            Ok(IdentityListOptions {
                identity_store: PathBuf::from("akraz identity.json"),
            })
        );
        assert_eq!(
            parse_identity_list_options(["--identity-store=akraz-identity.json"].map(String::from)),
            Ok(IdentityListOptions {
                identity_store: PathBuf::from("akraz-identity.json"),
            })
        );
    }

    #[test]
    fn parses_identity_forget_options() {
        assert_eq!(
            parse_identity_forget_options(
                [
                    "--identity-store",
                    "akraz-identity.json",
                    "--peer-id=linux-laptop",
                ]
                .map(String::from)
            ),
            Ok(IdentityForgetOptions {
                identity_store: PathBuf::from("akraz-identity.json"),
                peer_id: "linux-laptop".to_string(),
            })
        );
    }

    #[test]
    fn rejects_invalid_identity_options() {
        assert_eq!(
            parse_identity_show_options(["--identity-display-name", "Device"].map(String::from)),
            Err(CliUsageError::MissingIdentityStore)
        );
        assert_eq!(
            parse_identity_show_options(["--identity-store", ""].map(String::from)),
            Err(CliUsageError::InvalidIdentityOptionValue {
                option: "--identity-store",
                value: "".to_string(),
                reason: "path must not be empty".to_string(),
            })
        );
        assert_eq!(
            parse_identity_trust_options(
                ["--identity-store", "akraz-identity.json"].map(String::from)
            ),
            Err(CliUsageError::MissingIdentityPeerFile)
        );
        assert_eq!(
            parse_identity_list_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownIdentityOption("--bad".to_string()))
        );
        assert_eq!(
            parse_identity_forget_options(
                ["--identity-store", "akraz-identity.json"].map(String::from)
            ),
            Err(CliUsageError::MissingIdentityPeerId)
        );
        assert_eq!(
            parse_identity_forget_options(
                ["--identity-store", "akraz-identity.json", "--peer-id", "",].map(String::from)
            ),
            Err(CliUsageError::InvalidIdentityOptionValue {
                option: "--peer-id",
                value: "".to_string(),
                reason: "peer id must not be empty".to_string(),
            })
        );
        assert_eq!(
            parse_identity_trust_options(
                [
                    "--identity-store",
                    "akraz-identity.json",
                    "--peer-file",
                    "peer.json",
                    "--peer-file",
                    "other-peer.json",
                ]
                .map(String::from)
            ),
            Err(CliUsageError::DuplicateIdentityOption("--peer-file"))
        );
        assert_eq!(
            parse_identity_show_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownIdentityOption("--bad".to_string()))
        );
    }

    #[test]
    fn identity_show_exports_public_pairing_document_without_secret_key() {
        let path = unique_identity_path("show");
        let options = IdentityShowOptions {
            identity_store: path.clone(),
            identity_display_name: "Windows Desktop".to_string(),
        };

        let document = build_pairing_identity_document(&options).expect("pairing document");
        let encoded = serde_json::to_string(&document).expect("pairing document JSON");
        let store = FileIdentityStore::new(&path);
        let stored = store
            .load_or_create("Ignored Name")
            .expect("load stored identity");

        assert_eq!(document.device_id(), stored.identity().device_id());
        assert_eq!(document.display_name(), "Windows Desktop");
        assert_eq!(document.fingerprint(), stored.identity().fingerprint());
        assert!(!encoded.contains("identitySecretKey"));

        remove_identity_path(path);
    }

    #[test]
    fn identity_trust_imports_peer_pairing_document() {
        let local_path = unique_identity_path("trust-local");
        let peer_path = unique_identity_path("trust-peer");
        let peer_file = unique_identity_path("trust-peer-document");
        let peer_document = build_pairing_identity_document(&IdentityShowOptions {
            identity_store: peer_path.clone(),
            identity_display_name: "Linux Laptop".to_string(),
        })
        .expect("peer pairing document");
        fs::write(
            &peer_file,
            serde_json::to_string_pretty(&peer_document).expect("peer pairing JSON"),
        )
        .expect("write peer pairing document");

        let result = trust_pairing_identity_document(&IdentityTrustOptions {
            identity_store: local_path.clone(),
            identity_display_name: "Windows Desktop".to_string(),
            peer_file: peer_file.clone(),
        })
        .expect("trusted peer import");
        let trusted = FileIdentityStore::new(&local_path)
            .load_trusted_peer(peer_document.device_id())
            .expect("loaded trusted peer");

        assert!(result.trusted);
        assert_eq!(result.peer_id, peer_document.device_id());
        assert_eq!(trusted.identity().display_name(), "Linux Laptop");
        assert_eq!(
            trusted.identity().fingerprint(),
            peer_document.fingerprint()
        );

        remove_identity_path(local_path);
        remove_identity_path(peer_path);
        remove_identity_path(peer_file);
    }

    #[test]
    fn identity_list_outputs_trusted_peer_metadata() {
        let local_path = unique_identity_path("list-local");
        let peer_path = unique_identity_path("list-peer");
        let peer_file = unique_identity_path("list-peer-document");
        let peer_document = build_pairing_identity_document(&IdentityShowOptions {
            identity_store: peer_path.clone(),
            identity_display_name: "Linux Laptop".to_string(),
        })
        .expect("peer pairing document");
        fs::write(
            &peer_file,
            serde_json::to_string_pretty(&peer_document).expect("peer pairing JSON"),
        )
        .expect("write peer pairing document");
        trust_pairing_identity_document(&IdentityTrustOptions {
            identity_store: local_path.clone(),
            identity_display_name: "Windows Desktop".to_string(),
            peer_file: peer_file.clone(),
        })
        .expect("trusted peer import");

        let result = list_trusted_peer_identities(&IdentityListOptions {
            identity_store: local_path.clone(),
        })
        .expect("listed trusted peers");

        assert_eq!(result.peers.len(), 1);
        assert_eq!(result.peers[0].peer_id, peer_document.device_id());
        assert_eq!(result.peers[0].display_name, "Linux Laptop");
        assert_eq!(result.peers[0].fingerprint, peer_document.fingerprint());
        assert_eq!(result.peers[0].capabilities, default_pairing_capabilities());

        remove_identity_path(local_path);
        remove_identity_path(peer_path);
        remove_identity_path(peer_file);
    }

    #[test]
    fn identity_forget_removes_trusted_peer() {
        let local_path = unique_identity_path("forget-local");
        let peer_path = unique_identity_path("forget-peer");
        let peer_file = unique_identity_path("forget-peer-document");
        let peer_document = build_pairing_identity_document(&IdentityShowOptions {
            identity_store: peer_path.clone(),
            identity_display_name: "Linux Laptop".to_string(),
        })
        .expect("peer pairing document");
        fs::write(
            &peer_file,
            serde_json::to_string_pretty(&peer_document).expect("peer pairing JSON"),
        )
        .expect("write peer pairing document");
        trust_pairing_identity_document(&IdentityTrustOptions {
            identity_store: local_path.clone(),
            identity_display_name: "Windows Desktop".to_string(),
            peer_file: peer_file.clone(),
        })
        .expect("trusted peer import");

        let result = forget_trusted_peer_identity(&IdentityForgetOptions {
            identity_store: local_path.clone(),
            peer_id: peer_document.device_id().to_string(),
        })
        .expect("forgot trusted peer");

        assert!(result.forgotten);
        assert_eq!(result.peer_id, peer_document.device_id());
        assert!(
            list_trusted_peer_identities(&IdentityListOptions {
                identity_store: local_path.clone()
            })
            .expect("listed trusted peers")
            .peers
            .is_empty()
        );

        remove_identity_path(local_path);
        remove_identity_path(peer_path);
        remove_identity_path(peer_file);
    }

    #[test]
    fn parses_session_connect_options() {
        assert_eq!(
            parse_session_connect_options(
                [
                    "--endpoint",
                    "local-test",
                    "--peer-id",
                    "linux-laptop",
                    "--local-device-id",
                    "windows-desktop",
                    "--address",
                    "127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(SessionConnectOptions {
                endpoint: Some(IpcEndpoint {
                    kind: IpcEndpointKind::Manual,
                    address: "local-test".to_string(),
                }),
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            })
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--peer-id=linux-laptop",
                    "--local-device-id=windows-desktop",
                    "--address=127.0.0.1:24888",
                ]
                .map(String::from)
            ),
            Ok(SessionConnectOptions {
                endpoint: None,
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            })
        );
    }

    #[test]
    fn rejects_invalid_session_connect_options() {
        assert_eq!(
            parse_session_connect_options(["--peer-id"].map(String::from)),
            Err(CliUsageError::MissingSessionOptionValue("--peer-id"))
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--local-device-id",
                    "windows-desktop",
                    "--address",
                    "127.0.0.1:24888"
                ]
                .map(String::from)
            ),
            Err(CliUsageError::MissingSessionConnectPeerId)
        );
        assert_eq!(
            parse_session_connect_options(
                ["--peer-id", "linux-laptop", "--address", "127.0.0.1:24888"].map(String::from)
            ),
            Err(CliUsageError::MissingSessionConnectLocalDeviceId)
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--peer-id",
                    "linux-laptop",
                    "--local-device-id",
                    "windows-desktop"
                ]
                .map(String::from)
            ),
            Err(CliUsageError::MissingSessionConnectAddress)
        );
        assert_eq!(
            parse_session_connect_options(
                [
                    "--peer-id",
                    "linux-laptop",
                    "--local-device-id",
                    "windows-desktop",
                    "--address",
                    "bad-address",
                ]
                .map(String::from)
            ),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--address",
                value: "bad-address".to_string(),
                reason: "expected <ip>:<port> socket address".to_string(),
            })
        );
        assert_eq!(
            parse_session_connect_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownSessionOption("--bad".to_string()))
        );
    }

    #[test]
    fn rejects_peer_session_daemon_args_without_identity_store() {
        let options = parse_daemon_args_options(
            [
                "--capture-input",
                "--edge-binding",
                "RIGHT:linux-laptop:LEFT",
                "--local-device-id",
                "windows-desktop",
                "--peer-session",
                "linux-laptop@127.0.0.1:24888",
            ]
            .map(String::from),
        );

        assert_eq!(
            options,
            Err(CliUsageError::PeerTransportRequiresIdentityStore)
        );
    }

    #[test]
    fn parses_identity_store_daemon_args() {
        let options = parse_daemon_args_options(
            [
                "--identity-store",
                "akraz-identity.json",
                "--identity-display-name=Windows-Desktop",
                "--peer-listen=0.0.0.0:24888",
                "--peer-session=linux-laptop@127.0.0.1:24888",
            ]
            .map(String::from),
        );

        assert_eq!(
            options,
            Ok(DaemonArgsOptions {
                capture_input: false,
                edge_bindings: Vec::new(),
                peer_listen: Some("0.0.0.0:24888".to_string()),
                peer_session: Some("linux-laptop@127.0.0.1:24888".to_string()),
                local_device_id: None,
                identity_store: Some("akraz-identity.json".to_string()),
                identity_display_name: Some("Windows-Desktop".to_string()),
            })
        );
    }

    #[test]
    fn rejects_peer_listen_daemon_args_without_identity_store() {
        let options = parse_daemon_args_options(["--peer-listen=0.0.0.0:24888"].map(String::from));

        assert_eq!(
            options,
            Err(CliUsageError::PeerTransportRequiresIdentityStore)
        );
    }

    #[test]
    fn formats_manual_daemon_command_line() {
        let options = DaemonArgsOptions {
            capture_input: true,
            edge_bindings: vec!["right:linux-laptop:left".to_string()],
            peer_listen: Some("127.0.0.1:24887".to_string()),
            peer_session: Some("linux-laptop@127.0.0.1:24888".to_string()),
            local_device_id: Some("windows-desktop".to_string()),
            identity_store: Some("akraz-identity.json".to_string()),
            identity_display_name: Some("Windows-Desktop".to_string()),
        };

        assert_eq!(
            format_daemon_command_line(&options),
            "akraz-daemon --serve --capture-input --edge-binding right:linux-laptop:left --peer-listen 127.0.0.1:24887 --local-device-id windows-desktop --identity-store akraz-identity.json --identity-display-name Windows-Desktop --peer-session linux-laptop@127.0.0.1:24888"
        );
    }

    #[test]
    fn rejects_invalid_manual_daemon_args() {
        assert_eq!(
            parse_daemon_args_options(["--edge-binding"].map(String::from)),
            Err(CliUsageError::MissingDaemonOptionValue("--edge-binding"))
        );
        assert_eq!(
            parse_daemon_args_options(["--peer-listen", "not-an-address"].map(String::from)),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--peer-listen",
                value: "not-an-address".to_string(),
                reason: "expected <ip>:<port> socket address".to_string(),
            })
        );
        assert_eq!(
            parse_daemon_args_options(
                ["--peer-session", "linux-laptop@127.0.0.1:24888"].map(String::from)
            ),
            Err(CliUsageError::PeerTransportRequiresIdentityStore)
        );
        assert_eq!(
            parse_daemon_args_options(
                [
                    "--local-device-id",
                    "windows-desktop",
                    "--peer-session",
                    "linux-laptop@bad-address",
                ]
                .map(String::from)
            ),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--peer-session",
                value: "bad-address".to_string(),
                reason: "expected <ip>:<port> socket address".to_string(),
            })
        );
        assert_eq!(
            parse_daemon_args_options(["--edge-binding", "east:peer:left"].map(String::from)),
            Err(CliUsageError::InvalidDaemonOptionValue {
                option: "--edge-binding",
                value: "east".to_string(),
                reason: "edge must be one of left, right, top, bottom".to_string(),
            })
        );
        assert_eq!(
            parse_daemon_args_options(
                [
                    "--peer-listen",
                    "127.0.0.1:24887",
                    "--peer-listen=127.0.0.1:24888"
                ]
                .map(String::from)
            ),
            Err(CliUsageError::DuplicateDaemonOption("--peer-listen"))
        );
        assert_eq!(
            parse_daemon_args_options(["--bad"].map(String::from)),
            Err(CliUsageError::UnknownDaemonArgsOption("--bad".to_string()))
        );
    }

    #[test]
    fn status_request_uses_daemon_status_ipc_method() {
        let request = daemon_status_request();

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_DAEMON_STATUS);
    }

    #[test]
    fn daemon_logs_tail_request_uses_daemon_logs_tail_ipc_method() {
        let request = daemon_logs_tail_request();

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_DAEMON_LOGS_TAIL);
        assert_eq!(request.params.limit, None);
    }

    #[test]
    fn permissions_probe_request_uses_permissions_probe_ipc_method() {
        let request = permissions_probe_request();

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_PERMISSIONS_PROBE);
    }

    #[test]
    fn diagnostics_screen_topology_request_uses_diagnostics_ipc_method() {
        let request = diagnostics_screen_topology_request();

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_DIAGNOSTICS_SCREEN_TOPOLOGY);
    }

    #[test]
    fn diagnostics_keyboard_layout_request_uses_diagnostics_ipc_method() {
        let request = diagnostics_keyboard_layout_request();

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_DIAGNOSTICS_KEYBOARD_LAYOUT);
    }

    #[test]
    fn release_all_request_uses_input_release_all_ipc_method() {
        let request = input_release_all_request();

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_INPUT_RELEASE_ALL);
    }

    #[test]
    fn diagnostics_snapshot_redacts_peer_identifiers_and_sensitive_sections() {
        let capabilities = IpcPlatformCapabilities {
            can_capture_pointer: true,
            can_capture_keyboard: true,
            can_inject_pointer: true,
            can_inject_keyboard: false,
        };
        let status = DaemonStatus {
            daemon_version: "0.4.47".to_string(),
            mode: ControlModeSnapshot::Remote,
            protocol: ProtocolVersionSnapshot { major: 1, minor: 4 },
            peers: vec![
                PeerStatus {
                    peer_id: "linux-laptop-secret-id".to_string(),
                    display_name: "Alice Linux Laptop".to_string(),
                    connected: true,
                    local_device_id: Some("windows-desktop-secret-id".to_string()),
                    address: Some("127.0.0.1:24888".to_string()),
                },
                PeerStatus {
                    peer_id: "windows-desktop-secret-id".to_string(),
                    display_name: "Windows Desktop".to_string(),
                    connected: false,
                    local_device_id: None,
                    address: None,
                },
            ],
            capabilities: capabilities.clone(),
        };
        let permissions = PermissionsProbe {
            adapter_name: "windows".to_string(),
            capabilities,
            issues: vec![PermissionIssue {
                code: "inject-keyboard-unavailable".to_string(),
                message: "keyboard injection is unavailable".to_string(),
            }],
        };
        let topology = DiagnosticsScreenTopology {
            pointer_position: LogicalPointSnapshot { x: 1919, y: 540 },
            virtual_screen_bounds: LogicalRectSnapshot {
                x: -1920,
                y: 0,
                width: 3840,
                height: 1080,
            },
            monitors: monitor_snapshots(),
        };

        let snapshot = build_diagnostics_snapshot(
            status,
            permissions,
            Some(topology),
            Some(keyboard_layout()),
            None,
            "akrazctl",
            "0.4.47",
        );
        let encoded = serde_json::to_string(&snapshot).expect("diagnostics snapshot JSON");

        assert_eq!(snapshot.schema_version, "akraz.diagnostics.snapshot/v1");
        assert_eq!(snapshot.daemon.peer_count, 2);
        assert_eq!(snapshot.daemon.connected_peer_count, 1);
        assert_eq!(snapshot.permissions.adapter_name, "windows");
        assert_eq!(
            snapshot.screen_topology,
            Some(DiagnosticsScreenTopology {
                pointer_position: LogicalPointSnapshot { x: 1919, y: 540 },
                virtual_screen_bounds: LogicalRectSnapshot {
                    x: -1920,
                    y: 0,
                    width: 3840,
                    height: 1080,
                },
                monitors: monitor_snapshots(),
            })
        );
        assert_eq!(snapshot.keyboard_layout, Some(keyboard_layout()));
        assert_eq!(snapshot.privacy, Default::default());
        assert_eq!(
            snapshot.unavailable_sections,
            vec!["recentLogs".to_string(), "latencyHistogram".to_string()]
        );
        assert!(!encoded.contains("linux-laptop-secret-id"));
        assert!(!encoded.contains("windows-desktop-secret-id"));
        assert!(!encoded.contains("Alice Linux Laptop"));
        assert!(!encoded.contains("Windows Desktop"));
        assert!(!encoded.contains("privateKey"));
        assert!(!encoded.contains("clipboard"));
    }

    #[test]
    fn diagnostics_snapshot_marks_screen_topology_unavailable_when_absent() {
        let capabilities = IpcPlatformCapabilities {
            can_capture_pointer: true,
            can_capture_keyboard: true,
            can_inject_pointer: true,
            can_inject_keyboard: true,
        };
        let status = DaemonStatus {
            daemon_version: "0.4.47".to_string(),
            mode: ControlModeSnapshot::Local,
            protocol: ProtocolVersionSnapshot { major: 1, minor: 4 },
            peers: Vec::new(),
            capabilities: capabilities.clone(),
        };
        let permissions = PermissionsProbe {
            adapter_name: "unsupported".to_string(),
            capabilities,
            issues: Vec::new(),
        };

        let snapshot =
            build_diagnostics_snapshot(status, permissions, None, None, None, "akrazctl", "0.4.47");
        let encoded = serde_json::to_string(&snapshot).expect("diagnostics snapshot JSON");

        assert_eq!(snapshot.screen_topology, None);
        assert_eq!(
            snapshot.unavailable_sections,
            vec![
                "recentLogs".to_string(),
                "screenTopology".to_string(),
                "keyboardLayout".to_string(),
                "latencyHistogram".to_string()
            ]
        );
        assert!(!encoded.contains("pointerPosition"));
        assert!(!encoded.contains("virtualScreenBounds"));
    }

    #[test]
    fn parses_json_rpc_response_success_and_failure_envelopes() {
        let success = JsonRpcSuccess::new("local", ProtocolVersionSnapshot { major: 1, minor: 4 });
        let success_line = serde_json::to_string(&success).expect("success JSON");
        assert_eq!(
            parse_json_rpc_response::<ProtocolVersionSnapshot>(&success_line, "protocol"),
            Ok(ProtocolVersionSnapshot { major: 1, minor: 4 })
        );

        let failure = JsonRpcFailure::new(
            Some("local".to_string()),
            JsonRpcError::new(JSONRPC_ERROR_PARSE, "boom"),
        );
        let failure_line = serde_json::to_string(&failure).expect("failure JSON");
        assert_eq!(
            parse_json_rpc_response::<ProtocolVersionSnapshot>(&failure_line, "protocol"),
            Err("boom".to_string())
        );
    }

    #[test]
    fn rejects_mismatched_json_rpc_response_metadata() {
        let wrong_id = serde_json::json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": "other",
            "result": { "major": 1, "minor": 4 }
        })
        .to_string();
        assert_eq!(
            parse_json_rpc_response::<ProtocolVersionSnapshot>(&wrong_id, "protocol"),
            Err("daemon returned unexpected response id: other".to_string())
        );

        let wrong_version = serde_json::json!({
            "jsonrpc": "1.0",
            "id": LOCAL_REQUEST_ID,
            "result": { "major": 1, "minor": 4 }
        })
        .to_string();
        assert_eq!(
            parse_json_rpc_response::<ProtocolVersionSnapshot>(&wrong_version, "protocol"),
            Err("daemon returned unsupported JSON-RPC version: 1.0".to_string())
        );
    }

    #[test]
    fn session_connect_request_uses_session_connect_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_SESSION_CONNECT,
            akraz_ipc::SessionConnectParams {
                peer_id: "linux-laptop".to_string(),
                local_device_id: "windows-desktop".to_string(),
                address: "127.0.0.1:24888".to_string(),
            },
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_SESSION_CONNECT);
    }

    #[test]
    fn session_disconnect_request_uses_session_disconnect_ipc_method() {
        let request = JsonRpcRequest::new(
            LOCAL_REQUEST_ID,
            METHOD_SESSION_DISCONNECT,
            akraz_ipc::SessionDisconnectParams::default(),
        );

        assert_eq!(request.id, LOCAL_REQUEST_ID);
        assert_eq!(request.method, METHOD_SESSION_DISCONNECT);
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
            Err(other) => panic!("expected endpoint resolution failure, got {other:?}"),
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

    #[test]
    fn daemon_call_error_reports_unreachable_endpoint_with_next_action() {
        let endpoint = match IpcEndpoint::manual("local-test") {
            Ok(endpoint) => endpoint,
            Err(error) => panic!("expected endpoint: {error}"),
        };
        let error = IpcCallError::Transport {
            source: IpcTransportError::endpoint_unavailable(
                endpoint,
                "No process is on the other end of the pipe.",
            ),
        };

        assert_eq!(
            format_daemon_call_error(&error),
            "akraz daemon is not reachable at local-test. Start akraz-daemon, or pass --endpoint to use a different IPC endpoint. Details: No process is on the other end of the pipe."
        );
    }

    #[test]
    fn daemon_call_error_reports_request_failures_as_ipc_failures() {
        let error = IpcCallError::Transport {
            source: IpcTransportError::request_failed("pipe closed before a response line"),
        };

        assert_eq!(
            format_daemon_call_error(&error),
            "akraz daemon IPC request failed. Details: pipe closed before a response line"
        );
    }

    fn unique_identity_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "akrazctl-{label}-{}-{nanos}.json",
            std::process::id()
        ))
    }

    fn remove_identity_path(path: PathBuf) {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove {}: {error}", path.display()),
        }
    }
}

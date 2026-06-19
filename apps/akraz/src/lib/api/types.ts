export type ControlMode = "Local" | "EnteringRemote" | "Remote" | "LeavingRemote" | "Suspended";

export interface ProtocolVersion {
  major: number;
  minor: number;
}

export interface PeerStatus {
  peerId: string;
  displayName: string;
  connected: boolean;
}

export interface PlatformCapabilities {
  canCapturePointer: boolean;
  canCaptureKeyboard: boolean;
  canInjectPointer: boolean;
  canInjectKeyboard: boolean;
}

export interface DaemonStatus {
  daemonVersion: string;
  mode: ControlMode;
  protocol: ProtocolVersion;
  peers: PeerStatus[];
  capabilities: PlatformCapabilities;
}

export interface PermissionIssue {
  code: string;
  message: string;
}

export interface PermissionsProbe {
  adapterName: string;
  capabilities: PlatformCapabilities;
  issues: PermissionIssue[];
}

export interface LogicalPointSnapshot {
  x: number;
  y: number;
}

export interface LogicalRectSnapshot {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface DiagnosticsMonitorSnapshot {
  id: string;
  bounds: LogicalRectSnapshot;
  scaleFactorPercent: number | null;
  isPrimary: boolean;
}

export interface DiagnosticsScreenTopology {
  pointerPosition: LogicalPointSnapshot;
  virtualScreenBounds: LogicalRectSnapshot;
  monitors: DiagnosticsMonitorSnapshot[];
}

export interface DiagnosticsKeyboardLayout {
  source: string;
  layoutId: string;
  languageId: string;
  layoutName?: string;
}

export interface DiagnosticsLatencyHistogram {
  sampleCount: number;
  averageMicros: number;
  p95Micros: number;
  p99Micros: number;
}

export interface DiagnosticsDaemonSnapshot {
  daemonVersion: string;
  mode: ControlMode;
  protocol: ProtocolVersion;
  peerCount: number;
  connectedPeerCount: number;
  capabilities: PlatformCapabilities;
}

export interface DiagnosticsPermissionsSnapshot {
  adapterName: string;
  capabilities: PlatformCapabilities;
  issues: PermissionIssue[];
}

export interface DiagnosticsPrivacySnapshot {
  includesActualKeyInput: boolean;
  includesTextInput: boolean;
  includesClipboard: boolean;
  includesPrivateKeys: boolean;
  includesFullPeerPublicKeys: boolean;
  includesFullFilePaths: boolean;
}

export interface DiagnosticsSnapshot {
  schemaVersion: string;
  generatedBy: string;
  toolVersion: string;
  daemon: DiagnosticsDaemonSnapshot;
  permissions: DiagnosticsPermissionsSnapshot;
  screenTopology?: DiagnosticsScreenTopology;
  keyboardLayout?: DiagnosticsKeyboardLayout;
  latencyHistogram?: DiagnosticsLatencyHistogram;
  privacy: DiagnosticsPrivacySnapshot;
  unavailableSections: string[];
}

export type DaemonLogLevel = "Info" | "Warn" | "Error";

export interface DaemonLogEntry {
  sequence: number;
  level: DaemonLogLevel;
  event: string;
  message: string;
}

export interface DiagnosticsSupportBundle {
  schemaVersion: string;
  generatedBy: string;
  toolVersion: string;
  snapshot?: DiagnosticsSnapshot;
  daemonLifecycle?: DaemonLifecycleSnapshot;
  recentLogs: DaemonLogEntry[];
  previousDaemonCrash?: DaemonCrashMarker;
  includedSections: string[];
  unavailableSections: string[];
  privacy: DiagnosticsPrivacySnapshot;
}

export interface DaemonCrashMarker {
  schemaVersion: string;
  processRole: string;
  daemonVersion: string;
  reason: string;
  panicMessageClass: string;
  panicLocation?: DaemonPanicLocation;
  recordedAtUnixMillis: number;
  privacy: DaemonCrashMarkerPrivacy;
}

export interface DaemonPanicLocation {
  fileName: string;
  line: number;
  column: number;
}

export interface DaemonCrashMarkerPrivacy {
  includesSecretValues: boolean;
  includesFullFilePaths: boolean;
  includesInputPayload: boolean;
}

export type DaemonLifecyclePhase =
  | "not_running"
  | "starting"
  | "running"
  | "unreachable"
  | "failed";

export interface DaemonLifecycleSnapshot {
  phase: DaemonLifecyclePhase;
  status: DaemonStatus | null;
  detail: string | null;
  managedPid: number | null;
  previousCrash?: DaemonCrashMarker;
}

export type ScreenEdge = "left" | "right" | "top" | "bottom";

export interface ScreenEdgeBinding {
  localEdge: ScreenEdge;
  peerId: string;
  remoteEdge: ScreenEdge;
}

export interface ManualPeerAddressSetting {
  peerId: string;
  address: string;
}

export interface LayoutSettings {
  edgeBindings: ScreenEdgeBinding[];
}

export interface DaemonStartOptions {
  captureInput?: boolean;
  peerListenAddress?: string;
  edgeBindings?: ScreenEdgeBinding[];
}

export interface SessionConnectParams {
  peerId: string;
  localDeviceId: string;
  address: string;
}

export interface SessionStatus {
  peerId: string;
  localDeviceId: string;
  address: string;
  connected: boolean;
}

export interface SessionConnectResult {
  connected: boolean;
  session: SessionStatus;
}

export interface SessionDisconnectResult {
  disconnected: boolean;
  session: SessionStatus | null;
  mode: ControlMode;
}

export interface PairingIdentityDocument {
  kind: string;
  version: number;
  deviceId: string;
  displayName: string;
  identityPublicKey: string;
  fingerprint: string;
  capabilities: number;
}

export interface IdentityShowResult {
  deviceId: string;
  displayName: string;
  fingerprint: string;
  capabilities: number;
  document: PairingIdentityDocument;
  documentJson: string;
}

export interface IdentityTrustParams {
  peerDocumentJson: string;
}

export interface IdentityTrustedPeer {
  peerId: string;
  displayName: string;
  fingerprint: string;
  capabilities: number;
}

export interface IdentityTrustedPeersResult {
  peers: IdentityTrustedPeer[];
}

export interface IdentityTrustResult extends IdentityTrustedPeer {
  trusted: boolean;
}

export interface IdentityForgetTrustedParams {
  peerId: string;
}

export interface IdentityForgetTrustedResult {
  forgotten: boolean;
  peerId: string;
}

export interface AppSettings {
  captureInput: boolean;
  peerListenAddress: string;
  edgeBindings: ScreenEdgeBinding[];
  manualPeerAddresses: ManualPeerAddressSetting[];
}

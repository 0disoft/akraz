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

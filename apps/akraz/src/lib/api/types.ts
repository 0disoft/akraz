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

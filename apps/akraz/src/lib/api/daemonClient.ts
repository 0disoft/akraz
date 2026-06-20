import { invoke } from "@tauri-apps/api/core";

import type {
  DaemonLifecycleSnapshot,
  DaemonStartOptions,
  DiagnosticsSnapshot,
  DiagnosticsScreenTopology,
  DiagnosticsSupportBundle,
  PairingAcceptParams,
  PairingAcceptResult,
  PairingRejectParams,
  PairingRejectResult,
  PairingStartParams,
  PairingStartResult,
  PermissionsProbe,
  SessionConnectParams,
  SessionDiscoveryCandidatesResult,
} from "./types";

export const daemonClient = {
  status(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_status");
  },

  acknowledgeCrash(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_crash_acknowledge");
  },

  probePermissions(): Promise<PermissionsProbe> {
    return invoke<PermissionsProbe>("permissions_probe");
  },

  screenTopology(): Promise<DiagnosticsScreenTopology> {
    return invoke<DiagnosticsScreenTopology>("screen_topology_probe");
  },

  diagnosticsSnapshot(): Promise<DiagnosticsSnapshot> {
    return invoke<DiagnosticsSnapshot>("diagnostics_snapshot");
  },

  diagnosticsSupportBundle(): Promise<DiagnosticsSupportBundle> {
    return invoke<DiagnosticsSupportBundle>("diagnostics_support_bundle");
  },

  start(options: DaemonStartOptions = {}): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_start", { options });
  },

  stop(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_stop");
  },

  connectSession(params: SessionConnectParams): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("session_connect", { params });
  },

  sessionDiscoveryCandidates(): Promise<SessionDiscoveryCandidatesResult> {
    return invoke<SessionDiscoveryCandidatesResult>("session_discovery_candidates");
  },

  startPairing(params: PairingStartParams): Promise<PairingStartResult> {
    return invoke<PairingStartResult>("pairing_start", { params });
  },

  acceptPairing(params: PairingAcceptParams): Promise<PairingAcceptResult> {
    return invoke<PairingAcceptResult>("pairing_accept", { params });
  },

  rejectPairing(params: PairingRejectParams): Promise<PairingRejectResult> {
    return invoke<PairingRejectResult>("pairing_reject", { params });
  },

  disconnectSession(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("session_disconnect");
  },

  releaseAllInputs(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("input_release_all");
  },
};

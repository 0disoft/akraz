import { invoke } from "@tauri-apps/api/core";

import type {
  DaemonLifecycleSnapshot,
  DaemonStartOptions,
  DiagnosticsSnapshot,
  DiagnosticsScreenTopology,
  DiagnosticsSupportBundle,
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

  disconnectSession(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("session_disconnect");
  },

  releaseAllInputs(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("input_release_all");
  },
};

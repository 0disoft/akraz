import { invoke } from "@tauri-apps/api/core";

import type {
  DaemonLifecycleSnapshot,
  DaemonStartOptions,
  DiagnosticsSnapshot,
  DiagnosticsSupportBundle,
  PermissionsProbe,
  SessionConnectParams,
} from "./types";

export const daemonClient = {
  status(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_status");
  },

  probePermissions(): Promise<PermissionsProbe> {
    return invoke<PermissionsProbe>("permissions_probe");
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

  disconnectSession(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("session_disconnect");
  },

  releaseAllInputs(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("input_release_all");
  },
};

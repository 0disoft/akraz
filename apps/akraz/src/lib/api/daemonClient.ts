import { invoke } from "@tauri-apps/api/core";

import type { DaemonLifecycleSnapshot, DaemonStartOptions, SessionConnectParams } from "./types";

export const daemonClient = {
  status(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_status");
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
};

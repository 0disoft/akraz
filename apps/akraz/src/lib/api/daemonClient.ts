import { invoke } from "@tauri-apps/api/core";

import type { DaemonLifecycleSnapshot } from "./types";

export const daemonClient = {
  status(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_status");
  },

  start(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_start");
  },

  stop(): Promise<DaemonLifecycleSnapshot> {
    return invoke<DaemonLifecycleSnapshot>("daemon_stop");
  },
};

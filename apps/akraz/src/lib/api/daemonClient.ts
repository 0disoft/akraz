import { invoke } from "@tauri-apps/api/core";

import type { DaemonLifecycleSnapshot, DaemonStartOptions } from "./types";

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
};

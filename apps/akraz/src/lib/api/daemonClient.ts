import { invoke } from "@tauri-apps/api/core";

import type { DaemonStatus } from "./types";

export const daemonClient = {
  status(): Promise<DaemonStatus> {
    return invoke<DaemonStatus>("daemon_status");
  },
};

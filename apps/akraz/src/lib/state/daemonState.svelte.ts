import { daemonClient } from "../api/daemonClient";
import type { DaemonStatus } from "../api/types";

export class DaemonState {
  status = $state<DaemonStatus | null>(null);
  isLoading = $state(false);
  lastError = $state<string | null>(null);

  async refresh() {
    this.isLoading = true;
    this.lastError = null;

    try {
      this.status = await daemonClient.status();
    } catch (error) {
      this.status = null;
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.isLoading = false;
    }
  }
}

export const daemonState = new DaemonState();

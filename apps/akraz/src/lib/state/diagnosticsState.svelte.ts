import { daemonClient } from "../api/daemonClient";
import type { DiagnosticsSnapshot } from "../api/types";

type DiagnosticsOperation = "snapshot";

export class DiagnosticsState {
  snapshot = $state<DiagnosticsSnapshot | null>(null);
  operation = $state<DiagnosticsOperation | null>(null);
  lastError = $state<string | null>(null);

  get isBusy(): boolean {
    return this.operation !== null;
  }

  async refresh() {
    this.operation = "snapshot";
    this.lastError = null;

    try {
      this.snapshot = await daemonClient.diagnosticsSnapshot();
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }
}

export const diagnosticsState = new DiagnosticsState();

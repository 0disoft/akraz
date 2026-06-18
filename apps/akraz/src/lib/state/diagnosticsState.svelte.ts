import { daemonClient } from "../api/daemonClient";
import type { DiagnosticsSnapshot, DiagnosticsSupportBundle } from "../api/types";

type DiagnosticsOperation = "snapshot" | "bundle";

export class DiagnosticsState {
  snapshot = $state<DiagnosticsSnapshot | null>(null);
  bundle = $state<DiagnosticsSupportBundle | null>(null);
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
      this.bundle = null;
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }

  async refreshBundle() {
    this.operation = "bundle";
    this.lastError = null;

    try {
      this.bundle = await daemonClient.diagnosticsSupportBundle();
      this.snapshot = this.bundle.snapshot;
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }
}

export const diagnosticsState = new DiagnosticsState();

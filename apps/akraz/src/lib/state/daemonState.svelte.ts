import { daemonClient } from "../api/daemonClient";
import type { DaemonLifecycleSnapshot, DaemonStartOptions, DaemonStatus } from "../api/types";

type DaemonOperation = "refresh" | "start" | "stop";

export class DaemonState {
  snapshot = $state<DaemonLifecycleSnapshot | null>(null);
  operation = $state<DaemonOperation | null>(null);
  lastError = $state<string | null>(null);

  get status(): DaemonStatus | null {
    return this.snapshot?.status ?? null;
  }

  get isBusy(): boolean {
    return this.operation !== null;
  }

  async refresh() {
    await this.run("refresh", () => daemonClient.status());
  }

  async start(options: DaemonStartOptions = {}) {
    await this.run("start", () => daemonClient.start(options));
  }

  async stop() {
    await this.run("stop", () => daemonClient.stop());
  }

  private async run(operation: DaemonOperation, action: () => Promise<DaemonLifecycleSnapshot>) {
    this.operation = operation;
    this.lastError = null;
    try {
      this.snapshot = await action();
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }
}

export const daemonState = new DaemonState();

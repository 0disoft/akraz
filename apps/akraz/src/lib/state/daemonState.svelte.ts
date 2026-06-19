import { daemonClient } from "../api/daemonClient";
import type {
  DaemonLifecycleSnapshot,
  DaemonStartOptions,
  DaemonStatus,
  SessionConnectParams,
} from "../api/types";

type DaemonOperation =
  | "refresh"
  | "acknowledgeCrash"
  | "start"
  | "stop"
  | "connectSession"
  | "disconnectSession"
  | "releaseAllInputs";

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

  async acknowledgeCrash() {
    await this.run("acknowledgeCrash", () => daemonClient.acknowledgeCrash());
  }

  async start(options: DaemonStartOptions = {}) {
    await this.run("start", () => daemonClient.start(options));
  }

  async stop() {
    await this.run("stop", () => daemonClient.stop());
  }

  async connectSession(params: SessionConnectParams) {
    await this.run("connectSession", () => daemonClient.connectSession(params));
  }

  async disconnectSession() {
    await this.run("disconnectSession", () => daemonClient.disconnectSession());
  }

  async releaseAllInputs() {
    await this.run("releaseAllInputs", () => daemonClient.releaseAllInputs());
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

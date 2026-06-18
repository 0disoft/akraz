import { daemonClient } from "../api/daemonClient";
import type { PermissionsProbe } from "../api/types";

type PermissionOperation = "probe";

export class PermissionState {
  probe = $state<PermissionsProbe | null>(null);
  operation = $state<PermissionOperation | null>(null);
  lastError = $state<string | null>(null);

  get isBusy() {
    return this.operation !== null;
  }

  async refresh() {
    this.operation = "probe";
    this.lastError = null;

    try {
      this.probe = await daemonClient.probePermissions();
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }
}

export const permissionState = new PermissionState();

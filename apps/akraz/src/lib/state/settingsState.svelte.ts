import { layoutClient } from "../api/layoutClient";
import { settingsClient } from "../api/settingsClient";
import type { AppSettings, DaemonStartOptions, ScreenEdgeBinding } from "../api/types";
import { manualPeerAddress, updateManualPeerAddress } from "../settings/manualPeerAddresses";

type SettingsOperation = "load" | "save";

function defaultSettings(): AppSettings {
  return {
    captureInput: false,
    peerListenAddress: "",
    edgeBindings: [],
    manualPeerAddresses: [],
  };
}

export class SettingsState {
  settings = $state<AppSettings>(defaultSettings());
  operation = $state<SettingsOperation | null>(null);
  lastError = $state<string | null>(null);
  saved = $state(false);

  get isBusy(): boolean {
    return this.operation !== null;
  }

  get startOptions(): DaemonStartOptions {
    const peerListenAddress = this.settings.peerListenAddress.trim();
    const options: DaemonStartOptions = {
      captureInput: this.settings.captureInput,
    };
    if (peerListenAddress.length > 0) {
      options.peerListenAddress = peerListenAddress;
    }

    return options;
  }

  updateCaptureInput(captureInput: boolean) {
    this.settings.captureInput = captureInput;
    this.saved = false;
  }

  updatePeerListenAddress(peerListenAddress: string) {
    this.settings.peerListenAddress = peerListenAddress;
    this.saved = false;
  }

  replaceEdgeBindings(edgeBindings: ScreenEdgeBinding[]) {
    this.settings.edgeBindings = edgeBindings;
  }

  manualPeerAddress(peerId: string): string {
    return manualPeerAddress(this.settings.manualPeerAddresses, peerId);
  }

  updateManualPeerAddress(peerId: string, address: string) {
    const nextManualPeerAddresses = updateManualPeerAddress(
      this.settings.manualPeerAddresses,
      peerId,
      address,
    );
    if (nextManualPeerAddresses === this.settings.manualPeerAddresses) {
      return;
    }

    this.settings.manualPeerAddresses = nextManualPeerAddresses;
    this.saved = false;
  }

  async load(): Promise<AppSettings | null> {
    let loadedSettings: AppSettings | null = null;
    await this.run("load", async () => {
      loadedSettings = await settingsClient.load();
      this.settings = loadedSettings;
      this.saved = false;
    });

    return loadedSettings;
  }

  async save(): Promise<AppSettings | null> {
    let savedSettings: AppSettings | null = null;
    await this.run("save", async () => {
      const layout = await layoutClient.get();
      savedSettings = await settingsClient.save({
        ...this.settings,
        edgeBindings: layout.edgeBindings,
      });
      this.settings = savedSettings;
      this.saved = true;
    });

    return savedSettings;
  }

  private async run(operation: SettingsOperation, action: () => Promise<void>) {
    this.operation = operation;
    this.lastError = null;
    try {
      await action();
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
    } finally {
      this.operation = null;
    }
  }
}

export const settingsState = new SettingsState();

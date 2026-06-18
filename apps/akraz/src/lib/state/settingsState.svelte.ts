import { settingsClient } from "../api/settingsClient";
import type { AppSettings, DaemonStartOptions, ScreenEdge, ScreenEdgeBinding } from "../api/types";

type SettingsOperation = "load" | "save";

function defaultSettings(): AppSettings {
  return {
    captureInput: false,
    peerListenAddress: "",
    edgeBindings: [],
    manualPeerAddresses: [],
  };
}

function defaultEdgeBinding(): ScreenEdgeBinding {
  return {
    localEdge: "right",
    peerId: "",
    remoteEdge: "left",
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
      edgeBindings: this.settings.edgeBindings,
    };
    if (peerListenAddress.length > 0) {
      options.peerListenAddress = peerListenAddress;
    }

    return options;
  }

  addEdgeBinding() {
    this.settings.edgeBindings = [...this.settings.edgeBindings, defaultEdgeBinding()];
    this.saved = false;
  }

  removeEdgeBinding(index: number) {
    this.settings.edgeBindings = this.settings.edgeBindings.filter(
      (_, itemIndex) => itemIndex !== index,
    );
    this.saved = false;
  }

  updateCaptureInput(captureInput: boolean) {
    this.settings.captureInput = captureInput;
    this.saved = false;
  }

  updatePeerListenAddress(peerListenAddress: string) {
    this.settings.peerListenAddress = peerListenAddress;
    this.saved = false;
  }

  updateEdgeBinding(index: number, field: keyof ScreenEdgeBinding, value: string) {
    this.settings.edgeBindings = this.settings.edgeBindings.map((binding, itemIndex) => {
      if (itemIndex !== index) {
        return binding;
      }

      return {
        ...binding,
        [field]: field === "peerId" ? value : (value as ScreenEdge),
      };
    });
    this.saved = false;
  }

  manualPeerAddress(peerId: string): string {
    const normalizedPeerId = peerId.trim();
    return (
      this.settings.manualPeerAddresses.find((entry) => entry.peerId === normalizedPeerId)
        ?.address ?? ""
    );
  }

  updateManualPeerAddress(peerId: string, address: string) {
    const normalizedPeerId = peerId.trim();
    if (normalizedPeerId.length === 0) {
      return;
    }

    const normalizedAddress = address.trim();
    const remaining = this.settings.manualPeerAddresses.filter(
      (entry) => entry.peerId !== normalizedPeerId,
    );
    this.settings.manualPeerAddresses =
      normalizedAddress.length === 0
        ? remaining
        : [...remaining, { peerId: normalizedPeerId, address: normalizedAddress }];
    this.saved = false;
  }

  async load() {
    await this.run("load", async () => {
      this.settings = await settingsClient.load();
      this.saved = false;
    });
  }

  async save() {
    await this.run("save", async () => {
      this.settings = await settingsClient.save(this.settings);
      this.saved = true;
    });
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

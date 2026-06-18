import { identityClient } from "../api/identityClient";
import type { IdentityShowResult, IdentityTrustResult } from "../api/types";

type IdentityOperation = "load" | "trust";

export class IdentityState {
  local = $state<IdentityShowResult | null>(null);
  trusted = $state<IdentityTrustResult | null>(null);
  peerDocumentJson = $state("");
  operation = $state<IdentityOperation | null>(null);
  lastError = $state<string | null>(null);

  get isBusy(): boolean {
    return this.operation !== null;
  }

  get peerDocumentReady(): boolean {
    return this.peerDocumentJson.trim().length > 0;
  }

  updatePeerDocumentJson(peerDocumentJson: string) {
    this.peerDocumentJson = peerDocumentJson;
  }

  async load() {
    await this.run("load", async () => {
      this.local = await identityClient.show();
    });
  }

  async trust() {
    await this.run("trust", async () => {
      this.trusted = await identityClient.trust({
        peerDocumentJson: this.peerDocumentJson,
      });
      this.peerDocumentJson = "";
    });
  }

  private async run(operation: IdentityOperation, action: () => Promise<void>) {
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

export const identityState = new IdentityState();

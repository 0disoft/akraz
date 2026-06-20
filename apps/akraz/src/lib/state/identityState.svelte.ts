import { identityClient } from "../api/identityClient";
import type { IdentityShowResult, IdentityTrustedPeer, IdentityTrustResult } from "../api/types";

type IdentityOperation = "load" | "trust" | "forget";

export class IdentityState {
  local = $state<IdentityShowResult | null>(null);
  trusted = $state<IdentityTrustResult | null>(null);
  trustedPeers = $state<IdentityTrustedPeer[]>([]);
  peerDocumentJson = $state("");
  operation = $state<IdentityOperation | null>(null);
  forgettingPeerId = $state<string | null>(null);
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
      this.trustedPeers = (await identityClient.listTrusted()).peers;
    });
  }

  async trust() {
    await this.trustDocument(this.peerDocumentJson, true);
  }

  async trustPeerDocumentJson(peerDocumentJson: string) {
    await this.trustDocument(peerDocumentJson, false);
  }

  upsertTrusted(peer: IdentityTrustedPeer) {
    this.trustedPeers = upsertTrustedPeer(this.trustedPeers, peer);
  }

  private async trustDocument(peerDocumentJson: string, clearPeerDocumentJson: boolean) {
    await this.run("trust", async () => {
      this.trusted = await identityClient.trust({
        peerDocumentJson,
      });
      this.trustedPeers = upsertTrustedPeer(this.trustedPeers, this.trusted);
      if (clearPeerDocumentJson) {
        this.peerDocumentJson = "";
      }
    });
  }

  async forget(peerId: string) {
    this.forgettingPeerId = peerId;
    await this.run("forget", async () => {
      const result = await identityClient.forgetTrusted({ peerId });
      this.trustedPeers = this.trustedPeers.filter((peer) => peer.peerId !== result.peerId);
      if (this.trusted?.peerId === result.peerId) {
        this.trusted = null;
      }
    });
    this.forgettingPeerId = null;
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

function upsertTrustedPeer(
  peers: IdentityTrustedPeer[],
  peer: IdentityTrustedPeer,
): IdentityTrustedPeer[] {
  const next = peers.filter((existing) => existing.peerId !== peer.peerId);
  next.push(peer);
  return next;
}

export const identityState = new IdentityState();

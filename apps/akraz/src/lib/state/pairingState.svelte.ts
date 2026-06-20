import { daemonClient } from "../api/daemonClient";
import type { PairingAcceptResult, PairingRejectResult, PairingStartResult } from "../api/types";

type PairingOperation = "start" | "accept" | "reject";

export class PairingState {
  pending = $state<PairingStartResult | null>(null);
  accepted = $state<PairingAcceptResult | null>(null);
  operation = $state<PairingOperation | null>(null);
  lastError = $state<string | null>(null);

  get isBusy(): boolean {
    return this.operation !== null;
  }

  async start(peerDocumentJson: string): Promise<PairingStartResult | null> {
    const normalizedPeerDocumentJson = peerDocumentJson.trim();
    if (normalizedPeerDocumentJson.length === 0) {
      this.lastError = "상대 기기 정보가 필요해.";
      this.pending = null;
      this.accepted = null;
      return null;
    }

    this.operation = "start";
    this.lastError = null;
    this.pending = null;
    this.accepted = null;
    try {
      this.pending = await daemonClient.startPairing({
        peerDocumentJson: normalizedPeerDocumentJson,
      });
      return this.pending;
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
      return null;
    } finally {
      this.operation = null;
    }
  }

  async accept(): Promise<PairingAcceptResult | null> {
    const pendingPairing = this.pending;
    if (pendingPairing === null) {
      this.lastError = "확인할 기기가 없어.";
      return null;
    }

    this.operation = "accept";
    this.lastError = null;
    try {
      const result = await daemonClient.acceptPairing({
        peerId: pendingPairing.peerId,
        verificationCode: pendingPairing.verificationCode,
      });
      this.pending = null;
      this.accepted = result;
      return result;
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
      return null;
    } finally {
      this.operation = null;
    }
  }

  async reject(): Promise<PairingRejectResult | null> {
    const pendingPairing = this.pending;
    if (pendingPairing === null) {
      this.lastError = "취소할 기기가 없어.";
      return null;
    }

    this.operation = "reject";
    this.lastError = null;
    try {
      const result = await daemonClient.rejectPairing({
        peerId: pendingPairing.peerId,
      });
      this.pending = null;
      this.accepted = null;
      return result;
    } catch (error) {
      this.lastError = error instanceof Error ? error.message : String(error);
      return null;
    } finally {
      this.operation = null;
    }
  }

  clear() {
    this.pending = null;
    this.accepted = null;
    this.lastError = null;
  }
}

export const pairingState = new PairingState();

import type { IdentityTrustedPeer, ManualPeerAddressSetting, PeerStatus } from "../api/types";
import { manualPeerAddress } from "../settings/manualPeerAddresses";

export interface ManualConnectionCandidatesInput {
  trustedPeers: IdentityTrustedPeer[];
  manualPeerAddresses: ManualPeerAddressSetting[];
  peerStatuses: PeerStatus[];
  localDeviceId: string | null;
  draftLocalDeviceId: string;
}

export interface ManualConnectionCandidate {
  peerId: string;
  displayName: string;
  fingerprint: string;
  capabilities: number;
  address: string;
  localDeviceId: string;
  connected: boolean;
  ready: boolean;
}

export function buildManualConnectionCandidates(
  input: ManualConnectionCandidatesInput,
): ManualConnectionCandidate[] {
  const identityLocalDeviceId = input.localDeviceId?.trim() ?? "";
  const localDeviceId =
    identityLocalDeviceId.length > 0 ? identityLocalDeviceId : input.draftLocalDeviceId.trim();
  const connectedPeerIds = new Set(
    input.peerStatuses
      .filter((peer) => peer.connected)
      .map((peer) => peer.peerId.trim())
      .filter((peerId) => peerId.length > 0),
  );

  return input.trustedPeers.flatMap((peer) => {
    const peerId = peer.peerId.trim();
    if (peerId.length === 0) {
      return [];
    }

    const address = manualPeerAddress(input.manualPeerAddresses, peerId).trim();
    const connected = connectedPeerIds.has(peerId);

    return [
      {
        peerId,
        displayName: peer.displayName.trim() || peerId,
        fingerprint: peer.fingerprint,
        capabilities: peer.capabilities,
        address,
        localDeviceId,
        connected,
        ready: !connected && address.length > 0 && localDeviceId.length > 0,
      },
    ];
  });
}

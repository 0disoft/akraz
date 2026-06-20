import type {
  IdentityTrustedPeer,
  ManualPeerAddressSetting,
  PeerStatus,
  SessionDiscoveryCandidate,
} from "../api/types";
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

export interface ConnectionCandidatesInput extends ManualConnectionCandidatesInput {
  discoveryCandidates: SessionDiscoveryCandidate[];
}

export type ConnectionCandidateSource = "discovery" | "manual";

export interface ConnectionCandidate {
  peerId: string;
  displayName: string;
  fingerprint?: string;
  capabilities: number;
  address: string;
  localDeviceId: string;
  connected: boolean;
  trusted: boolean;
  ready: boolean;
  source: ConnectionCandidateSource;
  peerDocumentJson?: string;
  buildVersion?: string;
}

export function buildManualConnectionCandidates(
  input: ManualConnectionCandidatesInput,
): ManualConnectionCandidate[] {
  const localDeviceId = normalizeLocalDeviceId(input);
  const connectedPeerIds = connectedPeerIdSet(input.peerStatuses);

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

export function buildConnectionCandidates(input: ConnectionCandidatesInput): ConnectionCandidate[] {
  const localDeviceId = normalizeLocalDeviceId(input);
  const connectedPeerIds = connectedPeerIdSet(input.peerStatuses);
  const trustedPeersById = new Map(
    input.trustedPeers
      .map((peer) => [peer.peerId.trim(), peer] as const)
      .filter(([peerId]) => peerId.length > 0),
  );
  const discoveryPeerIds = new Set<string>();
  const discoveryCandidates = input.discoveryCandidates.flatMap((candidate) => {
    const peerId = candidate.peerId.trim();
    const address = candidate.address.trim();
    if (peerId.length === 0) {
      return [];
    }

    discoveryPeerIds.add(peerId);
    const trustedPeer = trustedPeersById.get(peerId);
    const trusted = candidate.trusted || trustedPeer !== undefined;
    const connected = connectedPeerIds.has(peerId);
    const displayName = candidate.displayName.trim() || trustedPeer?.displayName.trim() || peerId;
    const fingerprint = candidate.fingerprint ?? trustedPeer?.fingerprint;
    const connectionCandidate: ConnectionCandidate = {
      peerId,
      displayName,
      capabilities: candidate.capabilities,
      address,
      localDeviceId,
      connected,
      trusted,
      ready: trusted && !connected && address.length > 0 && localDeviceId.length > 0,
      source: "discovery",
      buildVersion: candidate.buildVersion,
    };

    if (fingerprint !== undefined) {
      connectionCandidate.fingerprint = fingerprint;
    }
    if (candidate.peerDocumentJson !== undefined) {
      connectionCandidate.peerDocumentJson = candidate.peerDocumentJson;
    }

    return [connectionCandidate];
  });

  const manualCandidates = buildManualConnectionCandidates(input)
    .filter((candidate) => !discoveryPeerIds.has(candidate.peerId))
    .map(
      (candidate): ConnectionCandidate => ({
        peerId: candidate.peerId,
        displayName: candidate.displayName,
        fingerprint: candidate.fingerprint,
        capabilities: candidate.capabilities,
        address: candidate.address,
        localDeviceId: candidate.localDeviceId,
        connected: candidate.connected,
        trusted: true,
        ready: candidate.ready,
        source: "manual",
      }),
    );

  return [...discoveryCandidates, ...manualCandidates];
}

function normalizeLocalDeviceId(input: ManualConnectionCandidatesInput): string {
  const identityLocalDeviceId = input.localDeviceId?.trim() ?? "";
  return identityLocalDeviceId.length > 0 ? identityLocalDeviceId : input.draftLocalDeviceId.trim();
}

function connectedPeerIdSet(peerStatuses: PeerStatus[]): Set<string> {
  return new Set(
    peerStatuses
      .filter((peer) => peer.connected)
      .map((peer) => peer.peerId.trim())
      .filter((peerId) => peerId.length > 0),
  );
}

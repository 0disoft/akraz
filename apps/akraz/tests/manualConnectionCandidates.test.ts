import { describe, expect, test } from "bun:test";

import {
  buildConnectionCandidates,
  buildManualConnectionCandidates,
} from "../src/lib/session/manualConnectionCandidates";
import type {
  IdentityTrustedPeer,
  ManualPeerAddressSetting,
  PeerStatus,
  SessionDiscoveryCandidate,
} from "../src/lib/api/types";

function trustedPeer(peerId: string, displayName = "Linux Laptop"): IdentityTrustedPeer {
  return {
    peerId,
    displayName,
    fingerprint: `AKRZ-${peerId}`,
    capabilities: 3,
  };
}

function discoveryCandidate(
  peerId: string,
  overrides: Partial<SessionDiscoveryCandidate> = {},
): SessionDiscoveryCandidate {
  return {
    peerId,
    displayName: "Linux Laptop",
    fingerprint: `AKRZ-${peerId}`,
    trusted: true,
    address: "127.0.0.1:4456",
    buildVersion: "0.5.7",
    capabilities: 3,
    ...overrides,
  };
}

describe("manual connection candidates", () => {
  test("builds candidates from trusted peers and saved manual addresses", () => {
    const manualPeerAddresses: ManualPeerAddressSetting[] = [
      { peerId: "linux-laptop", address: " 127.0.0.1:4455 " },
    ];

    expect(
      buildManualConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses,
        peerStatuses: [],
        localDeviceId: " windows-desktop ",
        draftLocalDeviceId: "",
      }),
    ).toEqual([
      {
        peerId: "linux-laptop",
        displayName: "Linux Laptop",
        fingerprint: "AKRZ-linux-laptop",
        capabilities: 3,
        address: "127.0.0.1:4455",
        localDeviceId: "windows-desktop",
        connected: false,
        ready: true,
      },
    ]);
  });

  test("uses the draft local device id when identity has not loaded", () => {
    expect(
      buildManualConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses: [{ peerId: "linux-laptop", address: "127.0.0.1:4455" }],
        peerStatuses: [],
        localDeviceId: null,
        draftLocalDeviceId: "manual-local",
      })[0]?.localDeviceId,
    ).toBe("manual-local");
  });

  test("uses the draft local device id when identity is blank", () => {
    expect(
      buildManualConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses: [{ peerId: "linux-laptop", address: "127.0.0.1:4455" }],
        peerStatuses: [],
        localDeviceId: " ",
        draftLocalDeviceId: "manual-local",
      })[0]?.ready,
    ).toBe(true);
  });

  test("keeps trusted peers visible when their address is missing", () => {
    expect(
      buildManualConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop", "")],
        manualPeerAddresses: [],
        peerStatuses: [],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      }),
    ).toEqual([
      {
        peerId: "linux-laptop",
        displayName: "linux-laptop",
        fingerprint: "AKRZ-linux-laptop",
        capabilities: 3,
        address: "",
        localDeviceId: "windows-desktop",
        connected: false,
        ready: false,
      },
    ]);
  });

  test("marks connected peers as already unavailable for a new manual session", () => {
    const peerStatuses: PeerStatus[] = [
      {
        peerId: "linux-laptop",
        displayName: "Linux Laptop",
        connected: true,
      },
    ];

    expect(
      buildManualConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses: [{ peerId: "linux-laptop", address: "127.0.0.1:4455" }],
        peerStatuses,
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      })[0],
    ).toMatchObject({
      connected: true,
      ready: false,
    });
  });

  test("ignores malformed trusted peer ids before they reach session inputs", () => {
    expect(
      buildManualConnectionCandidates({
        trustedPeers: [trustedPeer(" ")],
        manualPeerAddresses: [{ peerId: " ", address: "127.0.0.1:4455" }],
        peerStatuses: [],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      }),
    ).toEqual([]);
  });

  test("prefers discovered trusted candidates over saved manual addresses", () => {
    expect(
      buildConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses: [{ peerId: "linux-laptop", address: "127.0.0.1:4455" }],
        discoveryCandidates: [discoveryCandidate("linux-laptop")],
        peerStatuses: [],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      }),
    ).toEqual([
      {
        peerId: "linux-laptop",
        displayName: "Linux Laptop",
        fingerprint: "AKRZ-linux-laptop",
        capabilities: 3,
        address: "127.0.0.1:4456",
        localDeviceId: "windows-desktop",
        connected: false,
        trusted: true,
        ready: true,
        source: "discovery",
        buildVersion: "0.5.7",
      },
    ]);
  });

  test("falls back to saved manual candidates when discovery is empty", () => {
    expect(
      buildConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses: [{ peerId: "linux-laptop", address: "127.0.0.1:4455" }],
        discoveryCandidates: [],
        peerStatuses: [],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      })[0],
    ).toMatchObject({
      peerId: "linux-laptop",
      address: "127.0.0.1:4455",
      trusted: true,
      ready: true,
      source: "manual",
    });
  });

  test("keeps untrusted discovery candidates visible but unavailable", () => {
    expect(
      buildConnectionCandidates({
        trustedPeers: [],
        manualPeerAddresses: [],
        discoveryCandidates: [
          discoveryCandidate("new-peer", {
            displayName: "New Peer",
            fingerprint: undefined,
            trusted: false,
          }),
        ],
        peerStatuses: [],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      })[0],
    ).toMatchObject({
      peerId: "new-peer",
      displayName: "New Peer",
      trusted: false,
      ready: false,
      source: "discovery",
    });
  });

  test("carries registerable discovery documents without marking candidates ready", () => {
    expect(
      buildConnectionCandidates({
        trustedPeers: [],
        manualPeerAddresses: [],
        discoveryCandidates: [
          discoveryCandidate("new-peer", {
            fingerprint: "AKRZ-new-peer",
            peerDocumentJson: '{"kind":"akraz.peerIdentity"}',
            trusted: false,
          }),
        ],
        peerStatuses: [],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      })[0],
    ).toMatchObject({
      peerId: "new-peer",
      fingerprint: "AKRZ-new-peer",
      peerDocumentJson: '{"kind":"akraz.peerIdentity"}',
      trusted: false,
      ready: false,
      source: "discovery",
    });
  });

  test("marks connected discovery candidates as unavailable for another session", () => {
    expect(
      buildConnectionCandidates({
        trustedPeers: [trustedPeer("linux-laptop")],
        manualPeerAddresses: [],
        discoveryCandidates: [discoveryCandidate("linux-laptop")],
        peerStatuses: [
          {
            peerId: "linux-laptop",
            displayName: "Linux Laptop",
            connected: true,
          },
        ],
        localDeviceId: "windows-desktop",
        draftLocalDeviceId: "",
      })[0],
    ).toMatchObject({
      connected: true,
      ready: false,
      source: "discovery",
    });
  });
});

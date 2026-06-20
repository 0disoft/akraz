import { describe, expect, test } from "bun:test";

import { buildManualConnectionCandidates } from "../src/lib/session/manualConnectionCandidates";
import type {
  IdentityTrustedPeer,
  ManualPeerAddressSetting,
  PeerStatus,
} from "../src/lib/api/types";

function trustedPeer(peerId: string, displayName = "Linux Laptop"): IdentityTrustedPeer {
  return {
    peerId,
    displayName,
    fingerprint: `AKRZ-${peerId}`,
    capabilities: 3,
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
});

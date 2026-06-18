import { describe, expect, test } from "bun:test";

import {
  analyzeLayoutMismatch,
  firstLayoutDaemonStartBlockingIssue,
  isUsableScreenTopology,
} from "../src/lib/layout/layoutMismatch";
import type {
  DiagnosticsScreenTopology,
  IdentityTrustedPeer,
  LayoutSettings,
} from "../src/lib/api/types";

const topology: DiagnosticsScreenTopology = {
  pointerPosition: { x: 960, y: 540 },
  virtualScreenBounds: { x: 0, y: 0, width: 1920, height: 1080 },
};

const trustedPeer: IdentityTrustedPeer = {
  peerId: "linux-laptop",
  displayName: "Linux laptop",
  fingerprint: "SHA256:example",
  capabilities: 11,
};

function layout(edgeBindings: LayoutSettings["edgeBindings"]): LayoutSettings {
  return { edgeBindings };
}

describe("layout mismatch analysis", () => {
  test("reports an empty layout as actionable and topology-limited", () => {
    const report = analyzeLayoutMismatch({
      layout: layout([]),
      trustedPeers: [trustedPeer],
      topology: null,
    });

    expect(report.status).toBe("needs-action");
    expect(report.issues.map((issue) => issue.code)).toEqual([
      "missing-binding",
      "missing-topology",
    ]);
    expect(report.validBindingCount).toBe(0);
    expect(report.hasUsableTopology).toBe(false);
  });

  test("detects empty, unknown, and duplicate local edges once", () => {
    const report = analyzeLayoutMismatch({
      layout: layout([
        { localEdge: "left", peerId: " ", remoteEdge: "right" },
        { localEdge: "right", peerId: "unknown-peer", remoteEdge: "left" },
        { localEdge: "right", peerId: trustedPeer.peerId, remoteEdge: "left" },
      ]),
      trustedPeers: [trustedPeer],
      topology,
    });

    expect(report.status).toBe("needs-action");
    expect(report.issues.map((issue) => issue.code)).toEqual([
      "empty-peer-id",
      "unknown-peer",
      "duplicate-local-edge",
    ]);
    expect(report.validBindingCount).toBe(0);
  });

  test("treats a trusted peer without topology as limited", () => {
    const report = analyzeLayoutMismatch({
      layout: layout([{ localEdge: "right", peerId: trustedPeer.peerId, remoteEdge: "left" }]),
      trustedPeers: [trustedPeer],
      topology: null,
    });

    expect(report).toMatchObject({
      status: "limited",
      validBindingCount: 1,
      bindingCount: 1,
      trustedPeerCount: 1,
      hasUsableTopology: false,
    });
    expect(report.issues.map((issue) => issue.code)).toEqual(["missing-topology"]);
  });

  test("allows daemon start for empty or topology-limited layouts", () => {
    const emptyReport = analyzeLayoutMismatch({
      layout: layout([]),
      trustedPeers: [trustedPeer],
      topology: null,
    });
    const limitedReport = analyzeLayoutMismatch({
      layout: layout([{ localEdge: "right", peerId: trustedPeer.peerId, remoteEdge: "left" }]),
      trustedPeers: [trustedPeer],
      topology: null,
    });

    expect(firstLayoutDaemonStartBlockingIssue(emptyReport)).toBeNull();
    expect(firstLayoutDaemonStartBlockingIssue(limitedReport)).toBeNull();
  });

  test("blocks daemon start when saved bindings cannot resolve to one trusted edge", () => {
    const report = analyzeLayoutMismatch({
      layout: layout([
        { localEdge: "right", peerId: "unknown-peer", remoteEdge: "left" },
        { localEdge: "right", peerId: trustedPeer.peerId, remoteEdge: "left" },
      ]),
      trustedPeers: [trustedPeer],
      topology,
    });

    expect(firstLayoutDaemonStartBlockingIssue(report)).toMatchObject({
      code: "unknown-peer",
      message: "신뢰 목록에 없는 기기가 배치에 남아 있어.",
    });
  });

  test("accepts trusted peer bindings with usable topology", () => {
    const report = analyzeLayoutMismatch({
      layout: layout([{ localEdge: "right", peerId: trustedPeer.peerId, remoteEdge: "left" }]),
      trustedPeers: [trustedPeer],
      topology,
    });

    expect(report).toMatchObject({
      status: "ready",
      validBindingCount: 1,
      bindingCount: 1,
      trustedPeerCount: 1,
      hasUsableTopology: true,
    });
    expect(report.issues).toEqual([]);
  });

  test("rejects non-finite or empty screen bounds", () => {
    expect(isUsableScreenTopology(topology)).toBe(true);
    expect(
      isUsableScreenTopology({
        pointerPosition: { x: 0, y: 0 },
        virtualScreenBounds: { x: 0, y: 0, width: 0, height: 1080 },
      }),
    ).toBe(false);
    expect(
      isUsableScreenTopology({
        pointerPosition: { x: 0, y: 0 },
        virtualScreenBounds: { x: Number.NaN, y: 0, width: 1920, height: 1080 },
      }),
    ).toBe(false);
  });
});

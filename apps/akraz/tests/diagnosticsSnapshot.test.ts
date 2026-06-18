import { describe, expect, test } from "bun:test";

import {
  formatDiagnosticsSupportBundle,
  includedSectionsSummary,
  formatDiagnosticsSnapshot,
  screenTopologySummary,
  unavailableSectionsSummary,
} from "../src/lib/diagnostics/diagnosticsSnapshot";
import type { DiagnosticsSnapshot, DiagnosticsSupportBundle } from "../src/lib/api/types";

function snapshotFixture(): DiagnosticsSnapshot {
  return {
    schemaVersion: "akraz.diagnostics.snapshot/v1",
    generatedBy: "akraz-app",
    toolVersion: "0.4.49",
    daemon: {
      daemonVersion: "0.4.49",
      mode: "Local",
      protocol: { major: 1, minor: 4 },
      peerCount: 0,
      connectedPeerCount: 0,
      capabilities: {
        canCapturePointer: true,
        canCaptureKeyboard: true,
        canInjectPointer: true,
        canInjectKeyboard: true,
      },
    },
    permissions: {
      adapterName: "windows",
      capabilities: {
        canCapturePointer: true,
        canCaptureKeyboard: true,
        canInjectPointer: true,
        canInjectKeyboard: true,
      },
      issues: [],
    },
    screenTopology: {
      pointerPosition: { x: 120, y: 80 },
      virtualScreenBounds: { x: 0, y: 0, width: 1920, height: 1080 },
    },
    privacy: {
      includesActualKeyInput: false,
      includesTextInput: false,
      includesClipboard: false,
      includesPrivateKeys: false,
      includesFullPeerPublicKeys: false,
      includesFullFilePaths: false,
    },
    unavailableSections: ["recentLogs", "latencyHistogram"],
  };
}

function bundleFixture(): DiagnosticsSupportBundle {
  const snapshot = snapshotFixture();
  return {
    schemaVersion: "akraz.diagnostics.supportBundle/v1",
    generatedBy: "akraz-app",
    toolVersion: "0.4.49",
    snapshot,
    includedSections: ["daemon", "permissions", "screenTopology"],
    unavailableSections: snapshot.unavailableSections,
    privacy: snapshot.privacy,
  };
}

describe("diagnostics snapshot helpers", () => {
  test("formats stable pretty JSON", () => {
    const formatted = formatDiagnosticsSnapshot(snapshotFixture());

    expect(formatted).toContain('"schemaVersion": "akraz.diagnostics.snapshot/v1"');
    expect(formatted).toContain('"screenTopology": {');
  });

  test("summarizes screen topology and unavailable sections", () => {
    expect(screenTopologySummary(snapshotFixture())).toBe("1920x1080 @ 0,0");
    expect(unavailableSectionsSummary(snapshotFixture())).toBe("recentLogs, latencyHistogram");
    expect(includedSectionsSummary(bundleFixture())).toBe("daemon, permissions, screenTopology");
  });

  test("summarizes missing optional data", () => {
    const snapshot = snapshotFixture();
    delete snapshot.screenTopology;
    snapshot.unavailableSections = [];

    expect(screenTopologySummary(snapshot)).toBe("확인 안 됨");
    expect(unavailableSectionsSummary(snapshot)).toBe("없음");
  });

  test("formats support bundle without adding sensitive fields", () => {
    const formatted = formatDiagnosticsSupportBundle(bundleFixture());

    expect(formatted).toContain('"schemaVersion": "akraz.diagnostics.supportBundle/v1"');
    expect(formatted).toContain('"snapshot": {');
    expect(formatted).not.toContain("privateKey");
    expect(formatted).not.toContain("identitySecretKey");
    expect(formatted).not.toContain("actualKeyInput");
    expect(formatted).not.toContain("textInput");
  });
});

import { readFileSync } from "node:fs";

import { describe, expect, test } from "bun:test";

import {
  formatDiagnosticsSupportBundle,
  includedSectionsSummary,
  formatDiagnosticsSnapshot,
  formatRecentLogEntry,
  latencySummary,
  recentLogsSummary,
  screenTopologySummary,
  unavailableSectionsSummary,
} from "../src/lib/diagnostics/diagnosticsSnapshot";
import type { DiagnosticsSnapshot, DiagnosticsSupportBundle } from "../src/lib/api/types";

const appPackage = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
) as {
  version: string;
};

function snapshotFixture(): DiagnosticsSnapshot {
  return {
    schemaVersion: "akraz.diagnostics.snapshot/v1",
    generatedBy: "akraz-app",
    toolVersion: appPackage.version,
    daemon: {
      daemonVersion: appPackage.version,
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
    latencyHistogram: {
      sampleCount: 3,
      averageMicros: 450,
      p95Micros: 900,
      p99Micros: 900,
    },
    privacy: {
      includesActualKeyInput: false,
      includesTextInput: false,
      includesClipboard: false,
      includesPrivateKeys: false,
      includesFullPeerPublicKeys: false,
      includesFullFilePaths: false,
    },
    unavailableSections: ["recentLogs"],
  };
}

function bundleFixture(): DiagnosticsSupportBundle {
  const snapshot = snapshotFixture();
  return {
    schemaVersion: "akraz.diagnostics.supportBundle/v1",
    generatedBy: "akraz-app",
    toolVersion: appPackage.version,
    snapshot,
    recentLogs: [
      {
        sequence: 1,
        level: "Info",
        event: "daemon.status",
        message: "Daemon status requested.",
      },
    ],
    includedSections: ["daemon", "permissions", "screenTopology", "latencyHistogram", "recentLogs"],
    unavailableSections: [],
    privacy: snapshot.privacy,
  };
}

describe("diagnostics snapshot helpers", () => {
  test("formats stable pretty JSON", () => {
    const formatted = formatDiagnosticsSnapshot(snapshotFixture());

    expect(formatted).toContain('"schemaVersion": "akraz.diagnostics.snapshot/v1"');
    expect(formatted).toContain('"screenTopology": {');
    expect(formatted).toContain('"latencyHistogram": {');
  });

  test("summarizes screen topology, latency, and sections", () => {
    expect(screenTopologySummary(snapshotFixture())).toBe("1920x1080 @ 0,0");
    expect(latencySummary(snapshotFixture())).toBe("평균 0.45ms · p95 0.90ms · p99 0.90ms");
    expect(unavailableSectionsSummary(snapshotFixture())).toBe("recentLogs");
    expect(recentLogsSummary(bundleFixture())).toBe("1개");
    expect(formatRecentLogEntry(bundleFixture().recentLogs[0])).toBe(
      "#1 · Info · daemon.status · Daemon status requested.",
    );
    expect(includedSectionsSummary(bundleFixture())).toBe(
      "daemon, permissions, screenTopology, latencyHistogram, recentLogs",
    );
  });

  test("summarizes missing optional data", () => {
    const snapshot = snapshotFixture();
    delete snapshot.screenTopology;
    delete snapshot.latencyHistogram;
    snapshot.unavailableSections = [];

    expect(screenTopologySummary(snapshot)).toBe("확인 안 됨");
    expect(latencySummary(snapshot)).toBe("확인 안 됨");
    expect(unavailableSectionsSummary(snapshot)).toBe("없음");
  });

  test("formats support bundle without adding sensitive fields", () => {
    const formatted = formatDiagnosticsSupportBundle(bundleFixture());

    expect(formatted).toContain('"schemaVersion": "akraz.diagnostics.supportBundle/v1"');
    expect(formatted).toContain('"snapshot": {');
    expect(formatted).toContain('"recentLogs": [');
    expect(formatted).not.toContain("privateKey");
    expect(formatted).not.toContain("identitySecretKey");
    expect(formatted).not.toContain("actualKeyInput");
    expect(formatted).not.toContain("textInput");
  });
});

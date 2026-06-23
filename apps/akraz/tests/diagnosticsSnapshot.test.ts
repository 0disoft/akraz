import { readFileSync } from "node:fs";

import { describe, expect, test } from "bun:test";

import {
  formatDiagnosticsSupportBundle,
  includedSectionsSummary,
  formatDiagnosticsSnapshot,
  formatRecentLogEntry,
  keyboardLayoutSummary,
  latencySummary,
  previousDaemonCrashSummary,
  recentLogsSummary,
  runtimeEnvironmentSummary,
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
    runtimeEnvironment: {
      os: "windows",
      family: "windows",
      arch: "x86_64",
      sessionType: "windows-desktop",
      desktopEnvironment: "explorer",
    },
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
      monitors: [
        {
          id: "primary",
          bounds: { x: 0, y: 0, width: 1920, height: 1080 },
          scaleFactorPercent: 100,
          isPrimary: true,
        },
      ],
    },
    keyboardLayout: {
      source: "foregroundWindowThread",
      layoutId: "0x0000000004120412",
      languageId: "0x0412",
      layoutName: "00000412",
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
    runtimeEnvironment: snapshot.runtimeEnvironment,
    snapshot,
    recentLogs: [
      {
        sequence: 1,
        level: "Info",
        event: "daemon.status",
        message: "Daemon status requested.",
      },
    ],
    includedSections: [
      "runtimeEnvironment",
      "daemon",
      "permissions",
      "screenTopology",
      "keyboardLayout",
      "latencyHistogram",
      "recentLogs",
    ],
    unavailableSections: [],
    privacy: snapshot.privacy,
  };
}

function previousCrashFixture() {
  return {
    schemaVersion: "akraz.daemonCrashMarker/v1",
    processRole: "akraz-daemon",
    daemonVersion: appPackage.version,
    reason: "panic",
    panicMessageClass: "stringPayload",
    recordedAtUnixMillis: 123456,
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesInputPayload: false,
    },
  };
}

function lifecycleOnlyBundleFixture(): DiagnosticsSupportBundle {
  const previousCrash = previousCrashFixture();
  return {
    schemaVersion: "akraz.diagnostics.supportBundle/v1",
    generatedBy: "akraz-app",
    toolVersion: appPackage.version,
    runtimeEnvironment: {
      os: "windows",
      family: "windows",
      arch: "x86_64",
      sessionType: "windows-desktop",
      desktopEnvironment: "explorer",
    },
    daemonLifecycle: {
      phase: "not_running",
      status: null,
      detail: "Akraz needs the previous daemon crash to be reviewed before starting again.",
      managedPid: null,
      previousCrash,
    },
    previousDaemonCrash: previousCrash,
    recentLogs: [],
    includedSections: ["runtimeEnvironment", "daemonLifecycle", "previousDaemonCrash"],
    unavailableSections: [
      "daemon",
      "permissions",
      "screenTopology",
      "keyboardLayout",
      "latencyHistogram",
      "recentLogs",
    ],
    privacy: snapshotFixture().privacy,
  };
}

describe("diagnostics snapshot helpers", () => {
  test("formats stable pretty JSON", () => {
    const formatted = formatDiagnosticsSnapshot(snapshotFixture());

    expect(formatted).toContain('"schemaVersion": "akraz.diagnostics.snapshot/v1"');
    expect(formatted).toContain('"screenTopology": {');
    expect(formatted).toContain('"keyboardLayout": {');
    expect(formatted).toContain('"latencyHistogram": {');
  });

  test("summarizes screen topology, keyboard layout, latency, and sections", () => {
    expect(screenTopologySummary(snapshotFixture())).toBe("1920x1080 @ 0,0");
    expect(keyboardLayoutSummary(snapshotFixture())).toBe("0x0412 · 00000412 · 0x0000000004120412");
    expect(latencySummary(snapshotFixture())).toBe("평균 0.45ms · p95 0.90ms · p99 0.90ms");
    expect(runtimeEnvironmentSummary(snapshotFixture().runtimeEnvironment)).toBe(
      "windows/x86_64 · windows-desktop · explorer",
    );
    expect(unavailableSectionsSummary(snapshotFixture())).toBe("recentLogs");
    expect(recentLogsSummary(bundleFixture())).toBe("1개");
    expect(previousDaemonCrashSummary(bundleFixture())).toBe("없음");
    expect(formatRecentLogEntry(bundleFixture().recentLogs[0])).toBe(
      "#1 · Info · daemon.status · Daemon status requested.",
    );
    expect(includedSectionsSummary(bundleFixture())).toBe(
      "runtimeEnvironment, daemon, permissions, screenTopology, keyboardLayout, latencyHistogram, recentLogs",
    );
  });

  test("summarizes previous daemon crash markers", () => {
    const bundle = bundleFixture();
    bundle.previousDaemonCrash = {
      schemaVersion: "akraz.daemonCrashMarker/v1",
      processRole: "akraz-daemon",
      daemonVersion: appPackage.version,
      reason: "panic",
      panicMessageClass: "stringPayload",
      panicLocation: {
        fileName: "main.rs",
        line: 42,
        column: 9,
      },
      recordedAtUnixMillis: 123456,
      privacy: {
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesInputPayload: false,
      },
    };

    expect(previousDaemonCrashSummary(bundle)).toBe(`panic · v${appPackage.version} · main.rs:42`);
  });

  test("summarizes missing optional data", () => {
    const snapshot = snapshotFixture();
    delete snapshot.screenTopology;
    delete snapshot.keyboardLayout;
    delete snapshot.latencyHistogram;
    snapshot.unavailableSections = [];

    expect(screenTopologySummary(snapshot)).toBe("확인 안 됨");
    expect(keyboardLayoutSummary(snapshot)).toBe("확인 안 됨");
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

  test("formats lifecycle-only support bundle without a daemon snapshot", () => {
    const bundle = lifecycleOnlyBundleFixture();
    const formatted = formatDiagnosticsSupportBundle(bundle);

    expect(bundle.snapshot).toBeUndefined();
    expect(formatted).toContain('"daemonLifecycle": {');
    expect(formatted).toContain('"previousDaemonCrash": {');
    expect(formatted).not.toContain('"snapshot": {');
    expect(includedSectionsSummary(bundle)).toBe(
      "runtimeEnvironment, daemonLifecycle, previousDaemonCrash",
    );
    expect(previousDaemonCrashSummary(bundle)).toBe(`panic · v${appPackage.version}`);
  });
});

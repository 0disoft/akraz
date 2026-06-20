import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  evaluateUpdaterConfigPreflight,
  exitCodeForUpdaterConfigPreflight,
  parseUpdaterConfigPreflightArgs,
  writeUpdaterConfigPreflightOutputFile,
} from "../scripts/smoke-updater-config-preflight.mjs";

const completeUpdaterConfig = {
  bundle: {
    createUpdaterArtifacts: true,
  },
  plugins: {
    updater: {
      pubkey: "akraz-public-updater-key-content",
      endpoints: ["https://updates.example.com/{{target}}/{{arch}}/{{current_version}}"],
      windows: {
        installMode: "passive",
      },
    },
  },
};

describe("updater config preflight", () => {
  test("reports missing updater config without leaking endpoint or key values", () => {
    const report = evaluateUpdaterConfigPreflight({
      bundle: {
        active: true,
      },
    });

    expect(report.ready).toBe(false);
    expect(report.privacy.includesSecretValues).toBe(false);
    expect(report.privacy.includesFullFilePaths).toBe(false);
    expect(report.privacy.includesEndpointValues).toBe(false);
    expect(report.checks.map((check) => check.status)).toEqual([
      "missing",
      "missing",
      "missing",
      "pass",
      "pass",
    ]);
  });

  test("accepts production-safe updater configuration", () => {
    const report = evaluateUpdaterConfigPreflight(completeUpdaterConfig);
    const formatted = JSON.stringify(report);

    expect(report.ready).toBe(true);
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
    expect(formatted).not.toContain("akraz-public-updater-key-content");
    expect(formatted).not.toContain("updates.example.com");
  });

  test("rejects insecure endpoint and dangerous transport settings", () => {
    const report = evaluateUpdaterConfigPreflight({
      ...completeUpdaterConfig,
      plugins: {
        updater: {
          ...completeUpdaterConfig.plugins.updater,
          endpoints: ["http://updates.example.com/latest.json"],
          dangerousInsecureTransportProtocol: true,
        },
      },
    });
    const endpointsCheck = report.checks.find((check) => check.id === "updaterEndpoints");
    const transportCheck = report.checks.find((check) => check.id === "updaterDangerousTransport");

    expect(report.ready).toBe(false);
    expect(endpointsCheck?.status).toBe("invalid");
    expect(transportCheck?.status).toBe("invalid");
    expect(JSON.stringify(report)).not.toContain("http://updates.example.com");
  });

  test("rejects path-like, private, and placeholder updater public keys", () => {
    const pathReport = evaluateUpdaterConfigPreflight({
      ...completeUpdaterConfig,
      plugins: {
        updater: {
          ...completeUpdaterConfig.plugins.updater,
          pubkey: "C:\\Users\\cherr\\.tauri\\akraz.key",
        },
      },
    });
    const privateReport = evaluateUpdaterConfigPreflight({
      ...completeUpdaterConfig,
      plugins: {
        updater: {
          ...completeUpdaterConfig.plugins.updater,
          pubkey: "-----BEGIN PRIVATE KEY-----",
        },
      },
    });
    const placeholderReport = evaluateUpdaterConfigPreflight({
      ...completeUpdaterConfig,
      plugins: {
        updater: {
          ...completeUpdaterConfig.plugins.updater,
          pubkey: "CONTENT FROM PUBLICKEY.PEM",
        },
      },
    });

    expect(pathReport.ready).toBe(false);
    expect(privateReport.ready).toBe(false);
    expect(placeholderReport.ready).toBe(false);
    expect(pathReport.checks.find((check) => check.id === "updaterPubkey")?.status).toBe("invalid");
    expect(privateReport.checks.find((check) => check.id === "updaterPubkey")?.status).toBe(
      "invalid",
    );
    expect(placeholderReport.checks.find((check) => check.id === "updaterPubkey")?.status).toBe(
      "invalid",
    );
  });

  test("rejects unsupported Windows updater install mode", () => {
    const report = evaluateUpdaterConfigPreflight({
      ...completeUpdaterConfig,
      plugins: {
        updater: {
          ...completeUpdaterConfig.plugins.updater,
          windows: {
            installMode: "silent",
          },
        },
      },
    });
    const installModeCheck = report.checks.find(
      (check) => check.id === "updaterWindowsInstallMode",
    );

    expect(report.ready).toBe(false);
    expect(installModeCheck?.status).toBe("invalid");
  });

  test("uses expect-missing mode for repositories without updater publication config", () => {
    const missingReport = evaluateUpdaterConfigPreflight({
      bundle: {
        active: true,
      },
    });
    const invalidReport = evaluateUpdaterConfigPreflight({
      ...completeUpdaterConfig,
      plugins: {
        updater: {
          ...completeUpdaterConfig.plugins.updater,
          endpoints: ["latest.json"],
        },
      },
    });
    const readyReport = evaluateUpdaterConfigPreflight(completeUpdaterConfig);

    expect(parseUpdaterConfigPreflightArgs(["--expect-missing"]).expectMissing).toBe(true);
    expect(exitCodeForUpdaterConfigPreflight(missingReport, { expectMissing: true })).toBe(0);
    expect(exitCodeForUpdaterConfigPreflight(invalidReport, { expectMissing: true })).toBe(1);
    expect(exitCodeForUpdaterConfigPreflight(readyReport, { expectMissing: true })).toBe(1);
    expect(exitCodeForUpdaterConfigPreflight(readyReport)).toBe(0);
  });

  test("parses output file arguments and writes atomic JSON evidence", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-updater-preflight-out-"));
    const outputFile = join(tempDir, "nested", "updater.json");

    try {
      expect(
        parseUpdaterConfigPreflightArgs(["--expect-missing", "--out-file", outputFile]),
      ).toEqual({
        expectMissing: true,
        outFile: outputFile,
      });

      const report = evaluateUpdaterConfigPreflight({
        bundle: {
          active: true,
        },
      });
      const written = writeUpdaterConfigPreflightOutputFile(outputFile, report);

      expect(written).toBe(outputFile);
      expect(readFileSync(outputFile, "utf8").endsWith("\n")).toBe(true);
      expect(JSON.parse(readFileSync(outputFile, "utf8"))).toEqual(report);
      expect(() => parseUpdaterConfigPreflightArgs(["--out-file"])).toThrow(
        "--out-file requires a non-empty value",
      );
      expect(() => parseUpdaterConfigPreflightArgs(["--unknown"])).toThrow(
        "unknown updater config preflight argument: --unknown",
      );
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });
});

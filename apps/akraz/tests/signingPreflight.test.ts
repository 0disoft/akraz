import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { describe, expect, test } from "bun:test";

import {
  evaluateSigningPreflight,
  exitCodeForSigningPreflight,
  parseSigningPreflightArgs,
} from "../scripts/smoke-signing-preflight.mjs";

describe("signing preflight", () => {
  test("reports missing release signing inputs without secret values", () => {
    const report = evaluateSigningPreflight({});
    const formatted = JSON.stringify(report);

    expect(report.ready).toBe(false);
    expect(report.privacy.includesSecretValues).toBe(false);
    expect(report.privacy.includesFullFilePaths).toBe(false);
    expect(report.checks.map((check) => check.status)).toEqual([
      "missing",
      "missing",
      "missing",
      "missing",
    ]);
    expect(formatted).toContain("TAURI_SIGNING_PRIVATE_KEY");
    expect(formatted).not.toContain("super-secret");
  });

  test("accepts complete updater and Windows installer signing inputs", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-signing-preflight-"));
    const certificatePath = join(tempDir, "codesign.pfx");

    try {
      writeFileSync(certificatePath, "placeholder certificate fixture");

      const report = evaluateSigningPreflight({
        TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
        AKRAZ_WINDOWS_SIGNING_CERT_PATH: certificatePath,
        AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(true);
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain(certificatePath);
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });

  test("rejects a missing certificate path without printing the path", () => {
    const certificatePath = join(tmpdir(), "missing-akraz-codesign.pfx");
    const report = evaluateSigningPreflight({
      TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
      AKRAZ_WINDOWS_SIGNING_CERT_PATH: certificatePath,
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
    });
    const certificateCheck = report.checks.find(
      (check) => check.id === "windowsSigningCertificate",
    );

    expect(report.ready).toBe(false);
    expect(certificateCheck?.status).toBe("invalid");
    expect(JSON.stringify(report)).not.toContain(certificatePath);
  });

  test("uses expect-missing mode for no-secret smoke environments", () => {
    const missingReport = evaluateSigningPreflight({});
    const invalidReport = evaluateSigningPreflight({
      TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
      AKRAZ_WINDOWS_SIGNING_CERT_PATH: join(tmpdir(), "missing-akraz-codesign.pfx"),
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
    });
    const readyReport = evaluateSigningPreflight({
      TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
      AKRAZ_WINDOWS_SIGNING_CERT_BASE64: "super-secret-cert-data",
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
    });

    expect(parseSigningPreflightArgs(["--expect-missing"]).expectMissing).toBe(true);
    expect(exitCodeForSigningPreflight(missingReport, { expectMissing: true })).toBe(0);
    expect(exitCodeForSigningPreflight(invalidReport, { expectMissing: true })).toBe(1);
    expect(exitCodeForSigningPreflight(readyReport, { expectMissing: true })).toBe(1);
    expect(exitCodeForSigningPreflight(readyReport)).toBe(0);
  });
});

import { spawnSync } from "node:child_process";
import { describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  DEFAULT_DURATION_MS,
  SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION,
  WINDOWS_MVP_SOAK_SCHEMA_VERSION,
  WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
  assertSoakSummaryHealthy,
  buildScenarioFailure,
  buildSoakQaEvidence,
  buildSoakSummary,
  collectScenarioMetrics,
  createEmptySoakMetrics,
  mergeSoakMetrics,
  parseLastJsonObject,
  parseSoakOptions,
  selectSoakScenarios,
  writeSoakSummaryReportFile,
} from "../scripts/windows-mvp-soak-report.mjs";
import { listWindowsMvpSoakEvidenceQaCaseIds } from "../scripts/windows-mvp-qa-plan.mjs";

function runAppPackageScript(scriptName, args) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Windows MVP soak reporting", () => {
  test("defaults to the planned two hour soak duration", () => {
    const options = parseSoakOptions([]);

    expect(DEFAULT_DURATION_MS).toBe(120 * 60 * 1000);
    expect(options.durationMs).toBe(DEFAULT_DURATION_MS);
    expect(options.maxCycles).toBe(Number.POSITIVE_INFINITY);
  });

  test("selects named scenarios and rejects unknown scenarios", () => {
    const options = parseSoakOptions([
      "--scenario",
      "peer-session",
      "--max-cycles",
      "3",
      "--report-file",
      "soak-report.json",
    ]);
    const selected = selectSoakScenarios(options);

    expect(selected.map((scenario) => scenario.name)).toEqual(["peer-session"]);
    expect(options.maxCycles).toBe(3);
    expect(options.reportFile).toBe("soak-report.json");
    expect(() => selectSoakScenarios(parseSoakOptions(["--scenario", "missing"]))).toThrow(
      "unknown soak scenario",
    );
  });

  test("lists soak scenarios through the app package script", () => {
    const result = runAppPackageScript("smoke:windows-mvp-soak", ["--list"]);
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.scenarios).toEqual([
      "loopback-transport",
      "peer-session",
      "peer-session-executor",
      "remote-control-loopback",
      "runtime-recovery",
      "tcp-transport",
      "session-connect-lifecycle",
    ]);
  });

  test("extracts the last JSON report from noisy scenario output", () => {
    const report = parseLastJsonObject(
      'first line\n{"old":true}\nScenario passed.\n{"final":true}\n',
    );

    expect(report).toEqual({ final: true });
  });

  test("extracts a pretty printed final JSON report from noisy scenario output", () => {
    const finalReport = {
      final: true,
      nested: {
        releaseAllCount: 1,
      },
      injectedInputs: [{ kind: "pointerMoved", deltaX: 8, deltaY: 2 }],
    };
    const report = parseLastJsonObject(
      `first line\n{"old":true}\nScenario passed.\n${JSON.stringify(
        finalReport,
        null,
        2,
      )}\nWindows MVP soak passed.\n`,
    );

    expect(report).toEqual(finalReport);
  });

  test("returns undefined when scenario output has no JSON object", () => {
    expect(parseLastJsonObject("first line\nScenario passed.\n")).toBeUndefined();
  });

  test("turns transport smoke reports into stuck-input metrics", () => {
    const metrics = collectScenarioMetrics("loopback-transport", {
      commands: [
        { kind: "startRemoteSession" },
        { kind: "forwardInput" },
        { kind: "releaseAllInputs" },
        { kind: "stopRemoteSession" },
      ],
    });

    expect(metrics.remoteSessionStarts).toBe(1);
    expect(metrics.forwardedInputCommands).toBe(1);
    expect(metrics.releaseAllCommands).toBe(1);
    expect(metrics.remoteSessionStops).toBe(1);
    expect(metrics.stuckInputSuspicions).toBe(0);
  });

  test("flags forwarded input without a release signal", () => {
    const metrics = collectScenarioMetrics("broken-transport", {
      commands: [{ kind: "startRemoteSession" }, { kind: "forwardInput" }],
    });

    expect(metrics.stuckInputSuspicions).toBe(2);
  });

  test("counts peer executor and lifecycle cleanup evidence", () => {
    const merged = createEmptySoakMetrics();
    mergeSoakMetrics(
      merged,
      collectScenarioMetrics("peer-session-executor", {
        outcomes: [
          { kind: "remoteSessionStarted" },
          { kind: "inputForwarded" },
          { kind: "inputsReleased" },
          { kind: "remoteSessionStopped" },
        ],
        injectedInputs: [{ kind: "pointerMoved" }],
        releaseAllCount: 1,
      }),
    );
    mergeSoakMetrics(
      merged,
      collectScenarioMetrics("runtime-recovery", {
        systemResume: {
          recovered: true,
        },
        inputCaptureIdleWatchdog: {
          recovered: true,
        },
      }),
    );
    mergeSoakMetrics(
      merged,
      collectScenarioMetrics("session-connect-lifecycle", {
        schemaVersion: SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION,
        connected: true,
        reconnected: true,
        disconnected: true,
        connectCount: 2,
        disconnectCount: 2,
        inputReleaseAllCount: 1,
        finalPeerCount: 0,
      }),
    );

    expect(merged.forwardedInputOutcomes).toBe(1);
    expect(merged.injectedInputEvents).toBe(1);
    expect(merged.releaseAllOutcomes).toBe(2);
    expect(merged.platformReleaseAllCalls).toBe(1);
    expect(merged.runtimeResumeRecoveries).toBe(1);
    expect(merged.inputCaptureIdleWatchdogRecoveries).toBe(1);
    expect(merged.sessionConnects).toBe(2);
    expect(merged.sessionDisconnects).toBe(2);
    expect(merged.sessionReconnects).toBe(1);
    expect(merged.stuckInputSuspicions).toBe(0);
  });

  test("builds a schemaed summary and rejects unhealthy outcomes", () => {
    const startedAt = new Date("2026-06-20T00:00:00.000Z");
    const finishedAt = new Date("2026-06-20T00:00:01.000Z");
    const options = parseSoakOptions(["--duration-ms", "1000", "--max-cycles", "1"]);
    const summary = buildSoakSummary({
      completedCycles: 1,
      completedRuns: 1,
      failures: [],
      finishedAt,
      metrics: { ...createEmptySoakMetrics(), scenarioPasses: 1 },
      options,
      scenarios: [{ name: "peer-session" }],
      startedAt,
    });

    expect(summary.schemaVersion).toBe(WINDOWS_MVP_SOAK_SCHEMA_VERSION);
    expect(summary.elapsedMs).toBe(1000);
    expect(summary.metrics.scenarioPasses).toBe(1);
    expect(summary.qaEvidence).toMatchObject({
      supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
      supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
      status: "insufficient",
    });
    expect(summary.qaEvidence.blockers).toContain("remoteSessionStartMissing");
    expect(() => assertSoakSummaryHealthy(summary)).not.toThrow();

    const failed = {
      ...summary,
      failures: [
        buildScenarioFailure({
          cycle: 1,
          elapsedMs: 250,
          exitCode: 1,
          scenario: "peer-session",
          signal: null,
          timedOut: false,
        }),
      ],
    };
    expect(() => assertSoakSummaryHealthy(failed)).toThrow("peer-session");
  });

  test("maps soak metrics to Windows MVP QA evidence status", () => {
    const passingMetrics = {
      ...createEmptySoakMetrics(),
      scenarioPasses: 5,
      remoteSessionStarts: 2,
      remoteSessionStops: 2,
      forwardedInputOutcomes: 3,
      injectedInputEvents: 2,
      releaseAllOutcomes: 2,
      platformReleaseAllCalls: 1,
      runtimeResumeRecoveries: 1,
      inputCaptureIdleWatchdogRecoveries: 1,
      sessionConnects: 1,
      sessionDisconnects: 1,
      sessionReconnects: 1,
    };

    expect(buildSoakQaEvidence(passingMetrics)).toEqual({
      supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
      supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
      status: "pass",
      blockers: [],
    });

    expect(buildSoakQaEvidence(createEmptySoakMetrics())).toMatchObject({
      status: "insufficient",
      blockers: [
        "scenarioPassesMissing",
        "remoteSessionStartMissing",
        "remoteSessionStopMissing",
        "remoteInputMissing",
        "releaseSignalMissing",
        "sessionReconnectMissing",
        "runtimeResumeRecoveryMissing",
        "inputCaptureIdleWatchdogMissing",
      ],
    });

    expect(
      buildSoakQaEvidence(
        {
          ...passingMetrics,
          stuckInputSuspicions: 1,
        },
        [
          buildScenarioFailure({
            cycle: 1,
            elapsedMs: 250,
            exitCode: 1,
            scenario: "peer-session",
            signal: null,
            timedOut: false,
          }),
        ],
      ),
    ).toMatchObject({
      status: "failed",
      blockers: ["scenarioFailures", "stuckInputSuspicions"],
    });
  });

  test("keeps supported QA evidence coverage derived from the QA plan", () => {
    expect(WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS).toEqual(listWindowsMvpSoakEvidenceQaCaseIds());
  });

  test("writes soak summaries atomically with a trailing newline", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-soak-report-test-"));
    const reportFile = join(tempDir, "nested", "report.json");
    const staleTempFile = join(tempDir, "nested", ".report.json.stale.tmp");
    const summary = buildSoakSummary({
      completedCycles: 1,
      completedRuns: 1,
      failures: [],
      finishedAt: new Date("2026-06-20T00:00:01.000Z"),
      metrics: { ...createEmptySoakMetrics(), scenarioPasses: 1 },
      options: parseSoakOptions(["--duration-ms", "1000", "--max-cycles", "1"]),
      scenarios: [{ name: "peer-session-executor" }],
      startedAt: new Date("2026-06-20T00:00:00.000Z"),
    });

    try {
      const writtenFile = writeSoakSummaryReportFile(reportFile, summary);
      const reportText = readFileSync(reportFile, "utf8");

      expect(writtenFile).toBe(reportFile);
      expect(reportText.endsWith("\n")).toBe(true);
      expect(JSON.parse(reportText)).toEqual(summary);

      writeFileSync(staleTempFile, "stale", "utf8");
      writeSoakSummaryReportFile(reportFile, { ...summary, completedRuns: 2 });
      expect(JSON.parse(readFileSync(reportFile, "utf8")).completedRuns).toBe(2);
      expect(existsSync(staleTempFile)).toBe(true);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });
});

import { describe, expect, test } from "bun:test";

import {
  DEFAULT_DURATION_MS,
  SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION,
  WINDOWS_MVP_SOAK_SCHEMA_VERSION,
  assertSoakSummaryHealthy,
  buildScenarioFailure,
  buildSoakSummary,
  collectScenarioMetrics,
  createEmptySoakMetrics,
  mergeSoakMetrics,
  parseLastJsonObject,
  parseSoakOptions,
  selectSoakScenarios,
} from "../scripts/windows-mvp-soak-report.mjs";

describe("Windows MVP soak reporting", () => {
  test("defaults to the planned two hour soak duration", () => {
    const options = parseSoakOptions([]);

    expect(DEFAULT_DURATION_MS).toBe(120 * 60 * 1000);
    expect(options.durationMs).toBe(DEFAULT_DURATION_MS);
    expect(options.maxCycles).toBe(Number.POSITIVE_INFINITY);
  });

  test("selects named scenarios and rejects unknown scenarios", () => {
    const options = parseSoakOptions(["--scenario", "peer-session", "--max-cycles", "3"]);
    const selected = selectSoakScenarios(options);

    expect(selected.map((scenario) => scenario.name)).toEqual(["peer-session"]);
    expect(options.maxCycles).toBe(3);
    expect(() => selectSoakScenarios(parseSoakOptions(["--scenario", "missing"]))).toThrow(
      "unknown soak scenario",
    );
  });

  test("extracts the last JSON report from noisy scenario output", () => {
    const report = parseLastJsonObject(
      'first line\n{"old":true}\nScenario passed.\n{"final":true}\n',
    );

    expect(report).toEqual({ final: true });
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
      collectScenarioMetrics("session-connect-lifecycle", {
        schemaVersion: SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION,
        connected: true,
        disconnected: true,
        finalPeerCount: 0,
      }),
    );

    expect(merged.forwardedInputOutcomes).toBe(1);
    expect(merged.injectedInputEvents).toBe(1);
    expect(merged.releaseAllOutcomes).toBe(1);
    expect(merged.platformReleaseAllCalls).toBe(1);
    expect(merged.sessionConnects).toBe(1);
    expect(merged.sessionDisconnects).toBe(1);
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
});

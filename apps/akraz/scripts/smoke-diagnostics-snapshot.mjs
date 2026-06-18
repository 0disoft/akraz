import { execFileSync, spawn, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));
const extension = process.platform === "win32" ? ".exe" : "";
const daemonExecutable = join(workspaceRoot, "target", "debug", `akraz-daemon${extension}`);
const ctlExecutable = join(workspaceRoot, "target", "debug", `akrazctl${extension}`);
const endpoint = smokeEndpoint();

execFileSync("cargo", ["build", "-p", "akraz-daemon", "-p", "akrazctl"], {
  cwd: workspaceRoot,
  stdio: "inherit",
});

const daemon = startDaemon(["--serve", "--endpoint", endpoint]);

try {
  await waitForDaemonStatus(endpoint);
  const snapshot = runCtlJson(["diagnostics", "snapshot", "--endpoint", endpoint]);
  assertDiagnosticsSnapshot(snapshot);
  const bundle = runCtlJson(["diagnostics", "bundle", "--endpoint", endpoint]);
  assertDiagnosticsBundle(bundle, snapshot);
  console.log("Diagnostics snapshot smoke passed.");
} finally {
  stopDaemon(daemon.process);
  await waitForDaemonExit(daemon.process);
}

function smokeEndpoint() {
  if (process.platform === "win32") {
    return `\\\\.\\pipe\\akrazd-diagnostics-smoke-${process.pid}-${Date.now()}`;
  }

  return join(
    process.env.XDG_RUNTIME_DIR || process.env.TMPDIR || tmpdir(),
    `akrazd-diagnostics-smoke-${process.pid}-${Date.now()}.sock`,
  );
}

function startDaemon(args) {
  const child = spawn(daemonExecutable, args, {
    cwd: workspaceRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => process.stdout.write(`[daemon] ${chunk}`));
  child.stderr.on("data", (chunk) => process.stderr.write(`[daemon] ${chunk}`));

  return { process: child };
}

async function waitForDaemonStatus(targetEndpoint, attempt = 0, lastError = "") {
  if (attempt >= 80) {
    throw new Error(`diagnostics snapshot smoke could not reach daemon: ${lastError}`);
  }

  const result = spawnSync(ctlExecutable, ["status", "--endpoint", targetEndpoint], {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (result.status === 0) {
    return parseJsonRpc(result.stdout, "daemon status");
  }

  await new Promise((resolve) => setTimeout(resolve, 50));
  return waitForDaemonStatus(
    targetEndpoint,
    attempt + 1,
    result.stderr || result.stdout || String(result.error || "unknown error"),
  );
}

function runCtlJson(args) {
  const result = spawnSync(ctlExecutable, args, {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (result.stdout) {
    process.stdout.write(result.stdout);
  }
  if (result.stderr) {
    process.stderr.write(result.stderr);
  }
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`akrazctl ${args.join(" ")} failed with exit code ${result.status}`);
  }

  return JSON.parse(result.stdout);
}

function parseJsonRpc(output, label) {
  const line = output.split(/\r?\n/).findLast((entry) => entry.trimStart().startsWith("{"));
  if (!line) {
    throw new Error(`${label} did not emit a JSON-RPC response`);
  }

  const response = JSON.parse(line);
  if (response.jsonrpc !== "2.0") {
    throw new Error(`${label} returned unexpected JSON-RPC version`);
  }
  if (response.error) {
    throw new Error(`${label} returned error: ${response.error.message}`);
  }

  return response;
}

function assertDiagnosticsSnapshot(snapshot) {
  if (snapshot.schemaVersion !== "akraz.diagnostics.snapshot/v1") {
    throw new Error("diagnostics snapshot reported an unexpected schema version");
  }
  if (snapshot.generatedBy !== "akrazctl") {
    throw new Error("diagnostics snapshot reported an unexpected generator");
  }
  if (snapshot.toolVersion !== appPackage.version) {
    throw new Error(
      `diagnostics snapshot reported tool version ${snapshot.toolVersion}, expected ${appPackage.version}`,
    );
  }
  if (snapshot.daemon?.daemonVersion !== appPackage.version) {
    throw new Error(
      `diagnostics snapshot reported daemon version ${snapshot.daemon?.daemonVersion}, expected ${appPackage.version}`,
    );
  }
  if (snapshot.daemon.mode !== "Local") {
    throw new Error(`diagnostics snapshot expected daemon mode Local, got ${snapshot.daemon.mode}`);
  }
  if (typeof snapshot.daemon.protocol?.major !== "number") {
    throw new Error("diagnostics snapshot reported an invalid protocol major version");
  }
  if (typeof snapshot.daemon.protocol?.minor !== "number") {
    throw new Error("diagnostics snapshot reported an invalid protocol minor version");
  }
  if (snapshot.daemon.peerCount !== 0 || snapshot.daemon.connectedPeerCount !== 0) {
    throw new Error("diagnostics snapshot reported unexpected peer counts for an empty daemon");
  }

  assertCapabilities(snapshot.daemon.capabilities, "daemon");
  assertCapabilities(snapshot.permissions?.capabilities, "permissions");
  assertPermissionIssues(snapshot.permissions);
  assertOptionalScreenTopology(snapshot);
  assertLatencyHistogram(snapshot.latencyHistogram);
  assertPrivacy(snapshot.privacy);
  assertUnavailableSections(snapshot);
  assertNoSensitiveFields(snapshot);
}

function assertDiagnosticsBundle(bundle, snapshot) {
  if (bundle.schemaVersion !== "akraz.diagnostics.supportBundle/v1") {
    throw new Error("diagnostics bundle reported an unexpected schema version");
  }
  if (bundle.generatedBy !== "akrazctl") {
    throw new Error("diagnostics bundle reported an unexpected generator");
  }
  if (bundle.toolVersion !== appPackage.version) {
    throw new Error(
      `diagnostics bundle reported tool version ${bundle.toolVersion}, expected ${appPackage.version}`,
    );
  }
  assertBundleSnapshot(bundle.snapshot, snapshot);
  const expectedIncludedSections = ["daemon", "permissions"];
  if (bundle.snapshot.screenTopology) {
    expectedIncludedSections.push("screenTopology");
  }
  if (bundle.snapshot.latencyHistogram) {
    expectedIncludedSections.push("latencyHistogram");
  }
  if (bundle.recentLogs.length > 0) {
    expectedIncludedSections.push("recentLogs");
  }
  assertStringList(bundle.includedSections, expectedIncludedSections, "included sections");
  assertStringList(
    bundle.unavailableSections,
    expectedUnavailableSections(bundle.snapshot, true),
    "unavailable sections",
  );
  assertRecentLogs(bundle.recentLogs);
  assertPrivacy(bundle.privacy);
  assertNoSensitiveFields(bundle);
}

function assertCapabilities(capabilities, label) {
  const keys = ["canCapturePointer", "canCaptureKeyboard", "canInjectPointer", "canInjectKeyboard"];
  for (const key of keys) {
    if (typeof capabilities?.[key] !== "boolean") {
      throw new Error(`diagnostics snapshot reported invalid ${label} ${key}`);
    }
  }
}

function assertBundleSnapshot(bundleSnapshot, previousSnapshot) {
  if (bundleSnapshot.schemaVersion !== previousSnapshot.schemaVersion) {
    throw new Error("diagnostics bundle snapshot schema drifted from snapshot output");
  }
  if (bundleSnapshot.generatedBy !== previousSnapshot.generatedBy) {
    throw new Error("diagnostics bundle snapshot generator drifted from snapshot output");
  }
  if (bundleSnapshot.toolVersion !== previousSnapshot.toolVersion) {
    throw new Error("diagnostics bundle snapshot tool version drifted from snapshot output");
  }
  if (bundleSnapshot.daemon?.daemonVersion !== previousSnapshot.daemon?.daemonVersion) {
    throw new Error("diagnostics bundle snapshot daemon version drifted from snapshot output");
  }
  if (bundleSnapshot.daemon?.protocol?.major !== previousSnapshot.daemon?.protocol?.major) {
    throw new Error("diagnostics bundle snapshot protocol major drifted from snapshot output");
  }
  if (bundleSnapshot.daemon?.protocol?.minor !== previousSnapshot.daemon?.protocol?.minor) {
    throw new Error("diagnostics bundle snapshot protocol minor drifted from snapshot output");
  }
  assertCapabilities(bundleSnapshot.daemon?.capabilities, "bundle daemon");
  assertCapabilities(bundleSnapshot.permissions?.capabilities, "bundle permissions");
  assertOptionalScreenTopology(bundleSnapshot);
  assertLatencyHistogram(bundleSnapshot.latencyHistogram);
}

function assertPermissionIssues(permissions) {
  if (typeof permissions?.adapterName !== "string" || permissions.adapterName.length === 0) {
    throw new Error("diagnostics snapshot reported an invalid permission adapter");
  }
  if (!Array.isArray(permissions.issues)) {
    throw new Error("diagnostics snapshot reported invalid permission issues");
  }

  const expectedCodes = new Set();
  const capabilities = permissions.capabilities;
  if (!capabilities.canCapturePointer) {
    expectedCodes.add("capture_pointer_unavailable");
  }
  if (!capabilities.canCaptureKeyboard) {
    expectedCodes.add("capture_keyboard_unavailable");
  }
  if (!capabilities.canInjectPointer) {
    expectedCodes.add("inject_pointer_unavailable");
  }
  if (!capabilities.canInjectKeyboard) {
    expectedCodes.add("inject_keyboard_unavailable");
  }

  const reportedCodes = new Set();
  for (const issue of permissions.issues) {
    if (typeof issue?.code !== "string" || issue.code.length === 0) {
      throw new Error("diagnostics snapshot reported a permission issue without a code");
    }
    if (typeof issue.message !== "string" || issue.message.length === 0) {
      throw new Error("diagnostics snapshot reported a permission issue without a message");
    }
    reportedCodes.add(issue.code);
  }

  if (reportedCodes.size !== expectedCodes.size) {
    throw new Error("diagnostics snapshot reported permission issue count drift");
  }
  for (const code of expectedCodes) {
    if (!reportedCodes.has(code)) {
      throw new Error(`diagnostics snapshot did not report missing permission code ${code}`);
    }
  }
}

function assertScreenTopology(screenTopology) {
  const pointerPosition = screenTopology?.pointerPosition;
  if (typeof pointerPosition?.x !== "number" || typeof pointerPosition.y !== "number") {
    throw new Error("diagnostics snapshot reported an invalid pointer position");
  }

  const bounds = screenTopology.virtualScreenBounds;
  if (
    typeof bounds?.x !== "number" ||
    typeof bounds.y !== "number" ||
    typeof bounds.width !== "number" ||
    typeof bounds.height !== "number"
  ) {
    throw new Error("diagnostics snapshot reported invalid virtual screen bounds");
  }
  if (bounds.width <= 0 || bounds.height <= 0) {
    throw new Error("diagnostics snapshot reported non-positive virtual screen bounds");
  }
}

function assertOptionalScreenTopology(snapshot) {
  if (snapshot.screenTopology) {
    assertScreenTopology(snapshot.screenTopology);
    return;
  }
  if (
    !Array.isArray(snapshot.unavailableSections) ||
    !snapshot.unavailableSections.includes("screenTopology")
  ) {
    throw new Error("diagnostics snapshot omitted screen topology without marking it unavailable");
  }
}

function assertPrivacy(privacy) {
  const expectedFalseKeys = [
    "includesActualKeyInput",
    "includesTextInput",
    "includesClipboard",
    "includesPrivateKeys",
    "includesFullPeerPublicKeys",
    "includesFullFilePaths",
  ];
  for (const key of expectedFalseKeys) {
    if (privacy?.[key] !== false) {
      throw new Error(`diagnostics snapshot privacy flag ${key} must be false`);
    }
  }
}

function assertUnavailableSections(snapshot) {
  assertStringList(
    snapshot.unavailableSections,
    expectedUnavailableSections(snapshot, false),
    "unavailable sections",
  );
}

function assertLatencyHistogram(latency) {
  if (!latency || typeof latency !== "object") {
    throw new Error("diagnostics snapshot did not include latency histogram");
  }
  if (!Number.isInteger(latency.sampleCount) || latency.sampleCount < 2) {
    throw new Error("diagnostics snapshot reported an invalid latency sample count");
  }
  for (const key of ["averageMicros", "p95Micros", "p99Micros"]) {
    if (!Number.isInteger(latency[key]) || latency[key] < 0) {
      throw new Error(`diagnostics snapshot reported invalid ${key}`);
    }
  }
  if (latency.p95Micros < latency.averageMicros || latency.p99Micros < latency.p95Micros) {
    throw new Error("diagnostics snapshot reported inconsistent latency percentiles");
  }
}

function assertRecentLogs(entries) {
  if (!Array.isArray(entries) || entries.length === 0) {
    throw new Error("diagnostics bundle did not include recent daemon logs");
  }
  const allowedLevels = new Set(["Info", "Warn", "Error"]);
  for (const entry of entries) {
    if (!Number.isInteger(entry.sequence) || entry.sequence <= 0) {
      throw new Error("diagnostics bundle reported an invalid daemon log sequence");
    }
    if (!allowedLevels.has(entry.level)) {
      throw new Error(`diagnostics bundle reported invalid daemon log level ${entry.level}`);
    }
    if (typeof entry.event !== "string" || entry.event.length === 0) {
      throw new Error("diagnostics bundle reported a daemon log without an event");
    }
    if (typeof entry.message !== "string" || entry.message.length === 0) {
      throw new Error("diagnostics bundle reported a daemon log without a message");
    }
  }
  if (!entries.some((entry) => entry.event === "daemon.logs.tail")) {
    throw new Error("diagnostics bundle did not include the daemon logs tail event");
  }
}

function assertStringList(sections, expected, label) {
  if (!Array.isArray(sections) || sections.length !== expected.length) {
    throw new Error("diagnostics snapshot reported unexpected unavailable sections");
  }
  for (const section of expected) {
    if (!sections.includes(section)) {
      throw new Error(`diagnostics snapshot did not report ${section} in ${label}`);
    }
  }
}

function expectedUnavailableSections(snapshot, hasRecentLogs) {
  const expected = [];
  if (!hasRecentLogs) {
    expected.push("recentLogs");
  }
  if (!snapshot.screenTopology) {
    expected.push("screenTopology");
  }
  if (!snapshot.latencyHistogram) {
    expected.push("latencyHistogram");
  }
  return expected;
}

function assertNoSensitiveFields(snapshot) {
  const encoded = JSON.stringify(snapshot);
  const forbiddenFragments = [
    "peerId",
    "displayName",
    endpoint,
    "privateKey",
    "secretKey",
    "identitySecretKey",
    "actualKeyInput",
    "textInput",
  ];
  for (const fragment of forbiddenFragments) {
    if (encoded.includes(fragment)) {
      throw new Error(`diagnostics snapshot included forbidden fragment: ${fragment}`);
    }
  }
}

function stopDaemon(process) {
  if (!process.killed) {
    process.kill();
  }
}

async function waitForDaemonExit(process) {
  if (process.exitCode !== null || process.signalCode !== null) {
    return;
  }

  await Promise.race([
    new Promise((resolve) => process.once("exit", resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ]);
}

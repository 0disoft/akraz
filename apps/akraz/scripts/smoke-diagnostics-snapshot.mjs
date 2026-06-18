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
  assertScreenTopology(snapshot.screenTopology);
  assertPrivacy(snapshot.privacy);
  assertUnavailableSections(snapshot.unavailableSections);
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
  if (JSON.stringify(bundle.snapshot) !== JSON.stringify(snapshot)) {
    throw new Error("diagnostics bundle snapshot does not match diagnostics snapshot output");
  }
  const expectedIncludedSections = ["daemon", "permissions", "screenTopology"];
  assertStringList(bundle.includedSections, expectedIncludedSections, "included sections");
  assertStringList(
    bundle.unavailableSections,
    snapshot.unavailableSections,
    "unavailable sections",
  );
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

function assertUnavailableSections(sections) {
  const expected = ["recentLogs", "latencyHistogram"];
  assertStringList(sections, expected, "unavailable sections");
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

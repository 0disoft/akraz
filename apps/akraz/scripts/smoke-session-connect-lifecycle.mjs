import { execFileSync, spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION } from "./windows-mvp-soak-report.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));
const extension = process.platform === "win32" ? ".exe" : "";
const daemonExecutable = join(workspaceRoot, "target", "debug", `akraz-daemon${extension}`);
const ctlExecutable = join(workspaceRoot, "target", "debug", `akrazctl${extension}`);
const tempDir = mkdtempSync(join(tmpdir(), `akraz-session-smoke-${process.pid}-`));
const sourceEndpoint = smokeEndpoint("source");
const targetEndpoint = smokeEndpoint("target");
const sourceStore = join(tempDir, "source-identity.json");
const targetStore = join(tempDir, "target-identity.json");
const sourceDocumentPath = join(tempDir, "source-peer.json");
const targetDocumentPath = join(tempDir, "target-peer.json");

execFileSync("cargo", ["build", "-p", "akraz-daemon", "-p", "akrazctl"], {
  cwd: workspaceRoot,
  stdio: "inherit",
});

const sourceIdentity = runCtlDocument([
  "identity",
  "show",
  "--identity-store",
  sourceStore,
  "--identity-display-name",
  "Local Smoke Device",
]);
const targetIdentity = runCtlDocument([
  "identity",
  "show",
  "--identity-store",
  targetStore,
  "--identity-display-name",
  "Remote Smoke Device",
]);
writeFileSync(sourceDocumentPath, `${JSON.stringify(sourceIdentity, null, 2)}\n`);
writeFileSync(targetDocumentPath, `${JSON.stringify(targetIdentity, null, 2)}\n`);
runCtlDocument([
  "identity",
  "trust",
  "--identity-store",
  sourceStore,
  "--peer-file",
  targetDocumentPath,
]);
runCtlDocument([
  "identity",
  "trust",
  "--identity-store",
  targetStore,
  "--peer-file",
  sourceDocumentPath,
]);

const targetDaemon = startDaemon("target", [
  "--serve",
  "--endpoint",
  targetEndpoint,
  "--identity-store",
  targetStore,
  "--peer-listen",
  "127.0.0.1:0",
]);
const sourceDaemon = startDaemon("source", [
  "--serve",
  "--endpoint",
  sourceEndpoint,
  "--identity-store",
  sourceStore,
]);

let activeSession = false;
let connectCount = 0;
let disconnectCount = 0;
let inputReleaseAllCount = 0;

try {
  const targetListenAddress = await waitForPeerListenerAddress(targetDaemon.output);
  const initialSource = await waitForDaemonStatus(sourceEndpoint, "source initial status");
  const initialTarget = await waitForDaemonStatus(targetEndpoint, "target initial status");
  assertStatusMode(initialSource, "Local", "source initial status");
  assertStatusMode(initialTarget, "Local", "target initial status");
  assertPeerCount(initialSource, 0, "source initial status");
  assertPeerCount(initialTarget, 0, "target initial status");

  const connected = connectSession(targetListenAddress);
  activeSession = true;
  connectCount += 1;
  assertSessionConnected(connected, sourceIdentity.deviceId, targetIdentity.deviceId);

  const afterConnect = runCtlJsonRpc(["status", "--endpoint", sourceEndpoint]);
  assertStatusMode(afterConnect, "Local", "source connected status");
  assertPeerCount(afterConnect, 1, "source connected status");
  assertPeer(
    afterConnect.result.peers[0],
    targetIdentity.deviceId,
    targetIdentity.displayName,
    true,
  );

  const disconnectedResponse = disconnectSession();
  activeSession = false;
  disconnectCount += 1;
  assertSessionDisconnected(
    disconnectedResponse,
    sourceIdentity.deviceId,
    targetIdentity.deviceId,
  );

  const afterDisconnect = runCtlJsonRpc(["status", "--endpoint", sourceEndpoint]);
  assertStatusMode(afterDisconnect, "Local", "source disconnected status");
  assertPeerCount(afterDisconnect, 0, "source disconnected status");

  const reconnected = connectSession(targetListenAddress);
  activeSession = true;
  connectCount += 1;
  assertSessionConnected(reconnected, sourceIdentity.deviceId, targetIdentity.deviceId);

  const afterReconnect = runCtlJsonRpc(["status", "--endpoint", sourceEndpoint]);
  assertStatusMode(afterReconnect, "Local", "source reconnected status");
  assertPeerCount(afterReconnect, 1, "source reconnected status");
  assertPeer(
    afterReconnect.result.peers[0],
    targetIdentity.deviceId,
    targetIdentity.displayName,
    true,
  );

  const finalDisconnect = disconnectSession();
  activeSession = false;
  disconnectCount += 1;
  assertSessionDisconnected(finalDisconnect, sourceIdentity.deviceId, targetIdentity.deviceId);

  const afterFinalDisconnect = runCtlJsonRpc(["status", "--endpoint", sourceEndpoint]);
  assertStatusMode(afterFinalDisconnect, "Local", "source final disconnected status");
  assertPeerCount(afterFinalDisconnect, 0, "source final disconnected status");

  const releaseAll = runCtlJsonRpc(["input", "release-all", "--endpoint", sourceEndpoint]);
  assertInputReleased(releaseAll);
  inputReleaseAllCount += 1;

  console.log(
    JSON.stringify({
      schemaVersion: SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION,
      daemonVersion: appPackage.version,
      connected: connected.result.connected === true,
      reconnected: reconnected.result.connected === true,
      disconnected: disconnectedResponse.result.disconnected === true,
      connectCount,
      disconnectCount,
      inputReleaseAllCount,
      finalMode: afterFinalDisconnect.result.mode,
      finalPeerCount: afterFinalDisconnect.result.peers.length,
    }),
  );
  console.log("Session connect lifecycle smoke passed.");
} finally {
  if (activeSession) {
    spawnSync(ctlExecutable, ["session", "disconnect", "--endpoint", sourceEndpoint], {
      cwd: workspaceRoot,
      encoding: "utf8",
    });
  }
  stopDaemon(sourceDaemon.process);
  stopDaemon(targetDaemon.process);
  await waitForDaemonExit(sourceDaemon.process);
  await waitForDaemonExit(targetDaemon.process);
  rmSync(tempDir, { force: true, recursive: true });
}

function smokeEndpoint(name) {
  if (process.platform === "win32") {
    return `\\\\.\\pipe\\akrazd-session-smoke-${name}-${process.pid}-${Date.now()}`;
  }

  return join(
    process.env.XDG_RUNTIME_DIR || process.env.TMPDIR || "/tmp",
    `akrazd-session-smoke-${name}-${process.pid}-${Date.now()}.sock`,
  );
}

function startDaemon(label, args) {
  const daemon = spawn(daemonExecutable, args, {
    cwd: workspaceRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const output = [];

  daemon.stdout.setEncoding("utf8");
  daemon.stderr.setEncoding("utf8");
  daemon.stdout.on("data", (chunk) => {
    output.push(chunk);
    process.stdout.write(`[${label}] ${chunk}`);
  });
  daemon.stderr.on("data", (chunk) => {
    output.push(chunk);
    process.stderr.write(`[${label}] ${chunk}`);
  });

  return { process: daemon, output };
}

async function waitForPeerListenerAddress(output, attempt = 0) {
  if (attempt >= 80) {
    throw new Error(`session connect lifecycle smoke could not find peer listener address`);
  }
  const combined = output.join("");
  const match = combined.match(/akraz-daemon peer session listener at ([^\r\n]+)/);
  if (match) {
    return match[1].trim();
  }

  await new Promise((resolve) => setTimeout(resolve, 50));
  return waitForPeerListenerAddress(output, attempt + 1);
}

async function waitForDaemonStatus(endpoint, label, attempt = 0, lastError = "") {
  if (attempt >= 80) {
    throw new Error(`session connect lifecycle smoke could not reach ${label}: ${lastError}`);
  }

  const result = spawnSync(ctlExecutable, ["status", "--endpoint", endpoint], {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (result.status === 0) {
    return parseJsonRpc(result.stdout, label);
  }

  await new Promise((resolve) => setTimeout(resolve, 50));

  return waitForDaemonStatus(
    endpoint,
    label,
    attempt + 1,
    result.stderr || result.stdout || String(result.error || "unknown error"),
  );
}

function runCtlDocument(args) {
  const result = spawnSync(ctlExecutable, args, {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (result.stderr) {
    process.stderr.write(result.stderr);
  }
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`akrazctl ${args.join(" ")} failed with exit code ${result.status}`);
  }
  if (result.stdout) {
    process.stdout.write(result.stdout);
  }

  return JSON.parse(result.stdout);
}

function runCtlJsonRpc(args) {
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

  return parseJsonRpc(result.stdout, `akrazctl ${args.join(" ")}`);
}

function connectSession(targetListenAddress) {
  return runCtlJsonRpc([
    "session",
    "connect",
    "--endpoint",
    sourceEndpoint,
    "--peer-id",
    targetIdentity.deviceId,
    "--local-device-id",
    sourceIdentity.deviceId,
    "--address",
    targetListenAddress,
  ]);
}

function disconnectSession() {
  return runCtlJsonRpc(["session", "disconnect", "--endpoint", sourceEndpoint]);
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

function assertStatusMode(response, expected, label) {
  if (response.result?.daemonVersion !== appPackage.version) {
    throw new Error(
      `${label} returned daemon version ${response.result?.daemonVersion}, expected ${appPackage.version}`,
    );
  }
  if (response.result.mode !== expected) {
    throw new Error(`${label} expected mode ${expected}, got ${response.result.mode}`);
  }
}

function assertPeerCount(response, expected, label) {
  const peers = response.result?.peers;
  if (!Array.isArray(peers) || peers.length !== expected) {
    throw new Error(`${label} expected ${expected} peers, got ${peers?.length}`);
  }
}

function assertPeer(peer, peerId, displayName, connected) {
  if (peer?.peerId !== peerId || peer.displayName !== displayName || peer.connected !== connected) {
    throw new Error("session connect lifecycle smoke reported unexpected peer status");
  }
}

function assertSessionConnected(response, localDeviceId, peerId) {
  if (response.result?.connected !== true) {
    throw new Error("session connect lifecycle smoke did not connect the session");
  }
  assertSessionStatus(response.result.session, localDeviceId, peerId, true);
}

function assertSessionDisconnected(response, localDeviceId, peerId) {
  if (response.result?.disconnected !== true || response.result.mode !== "Local") {
    throw new Error("session connect lifecycle smoke did not disconnect the session");
  }
  assertSessionStatus(response.result.session, localDeviceId, peerId, false);
}

function assertInputReleased(response) {
  if (response.result?.released !== true || response.result.mode !== "Local") {
    throw new Error("session connect lifecycle smoke did not release local input state");
  }
}

function assertSessionStatus(session, localDeviceId, peerId, connected) {
  if (
    session?.peerId !== peerId ||
    session.localDeviceId !== localDeviceId ||
    session.connected !== connected
  ) {
    throw new Error("session connect lifecycle smoke reported unexpected session status");
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

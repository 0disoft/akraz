import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));

const smoke = spawnSync(
  "cargo",
  ["run", "-p", "akraz-daemon", "--", "--akraz-smoke-peer-session-executor"],
  {
    cwd: workspaceRoot,
    encoding: "utf8",
  },
);

if (smoke.stdout) {
  process.stdout.write(smoke.stdout);
}
if (smoke.stderr) {
  process.stderr.write(smoke.stderr);
}
if (smoke.error) {
  throw smoke.error;
}
if (smoke.status !== 0) {
  throw new Error(`peer session executor smoke failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error("peer session executor smoke did not emit a JSON report");
}

const report = JSON.parse(reportLine);
if (report.daemonVersion !== appPackage.version) {
  throw new Error(
    `peer session executor smoke returned daemon version ${report.daemonVersion}, expected ${appPackage.version}`,
  );
}

if (
  report.hello?.protocolMajor !== 1 ||
  report.hello.protocolMinor !== 0 ||
  report.hello.deviceId !== "local-smoke-device" ||
  report.hello.peerId !== "loopback-peer"
) {
  throw new Error("peer session executor smoke reported unexpected hello frame");
}

const [start, forward, release, stop] = report.outcomes ?? [];
assertKind(start, "remoteSessionStarted", "first outcome");
assertKind(forward, "inputForwarded", "second outcome");
assertKind(release, "inputsReleased", "third outcome");
assertKind(stop, "remoteSessionStopped", "fourth outcome");

if (start.peerId !== "loopback-peer" || start.crossing?.localEdge !== "right") {
  throw new Error("peer session executor smoke reported unexpected remote start outcome");
}
if (
  forward.event?.kind !== "pointerMoved" ||
  forward.event.deltaX !== 8 ||
  forward.event.deltaY !== 2
) {
  throw new Error("peer session executor smoke reported unexpected forwarded input outcome");
}
if (stop.sessionId !== "loopback-session") {
  throw new Error("peer session executor smoke reported unexpected stop session outcome");
}

const [injected] = report.injectedInputs ?? [];
if (
  injected?.kind !== "pointerMoved" ||
  injected.deltaX !== 8 ||
  injected.deltaY !== 2 ||
  report.injectedInputs.length !== 1
) {
  throw new Error("peer session executor smoke did not inject the expected pointer event");
}
if (report.releaseAllCount !== 1) {
  throw new Error("peer session executor smoke did not release inputs exactly once");
}

console.log("Peer session executor smoke passed.");

function assertKind(outcome, expected, label) {
  if (outcome?.kind !== expected) {
    throw new Error(
      `peer session executor smoke expected ${label} ${expected}, got ${outcome?.kind}`,
    );
  }
}

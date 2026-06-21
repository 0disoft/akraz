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
  ["run", "-p", "akraz-daemon", "--", "--akraz-smoke-remote-control-loopback"],
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
  throw new Error(`remote control loopback smoke failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error("remote control loopback smoke did not emit a JSON report");
}

const report = JSON.parse(reportLine);
if (report.daemonVersion !== appPackage.version) {
  throw new Error(
    `remote control loopback smoke returned daemon version ${report.daemonVersion}, expected ${appPackage.version}`,
  );
}

if (
  report.hello?.protocolMajor !== 1 ||
  report.hello.protocolMinor !== 5 ||
  report.hello.deviceId !== "local-smoke-device" ||
  report.hello.peerId !== "loopback-peer"
) {
  throw new Error("remote control loopback smoke reported unexpected hello frame");
}
if (report.seededSessionId !== "remote-control-loopback-session") {
  throw new Error("remote control loopback smoke reported unexpected seeded session id");
}

const [start, pointer, button, scroll, control, alt, release, stop] = report.outcomes ?? [];
assertKind(start, "remoteSessionStarted", "first outcome");
assertKind(pointer, "inputForwarded", "second outcome");
assertKind(button, "inputForwarded", "third outcome");
assertKind(scroll, "inputForwarded", "fourth outcome");
assertKind(control, "inputForwarded", "fifth outcome");
assertKind(alt, "inputForwarded", "sixth outcome");
assertKind(release, "inputsReleased", "seventh outcome");
assertKind(stop, "remoteSessionStopped", "eighth outcome");

if (start.peerId !== "loopback-peer" || start.crossing?.localEdge !== "right") {
  throw new Error("remote control loopback smoke reported unexpected remote start outcome");
}
assertPointer(pointer.event, "forwarded pointer");
assertButton(button.event, "forwarded mouse button");
assertScroll(scroll.event, "forwarded scroll");
assertKey(control.event, "leftControl", "forwarded control key");
assertKey(alt.event, "leftAlt", "forwarded alt key");
if (stop.sessionId !== "remote-control-loopback-session") {
  throw new Error("remote control loopback smoke reported unexpected stop session outcome");
}

if (report.capturedInputs?.length !== 6) {
  throw new Error("remote control loopback smoke did not capture the expected input sequence");
}
assertPointer(report.capturedInputs[0], "captured pointer");
assertButton(report.capturedInputs[1], "captured mouse button");
assertScroll(report.capturedInputs[2], "captured scroll");
assertKey(report.capturedInputs[3], "leftControl", "captured control key");
assertKey(report.capturedInputs[4], "leftAlt", "captured alt key");
assertKey(report.capturedInputs[5], "code:14", "captured panic key");

if (report.injectedInputs?.length !== 5) {
  throw new Error("remote control loopback smoke did not inject exactly the forwarded inputs");
}
for (let index = 0; index < report.injectedInputs.length; index += 1) {
  if (
    JSON.stringify(report.injectedInputs[index]) !== JSON.stringify(report.capturedInputs[index])
  ) {
    throw new Error(`remote control loopback smoke injection ${index} drifted from capture input`);
  }
}
if (report.releaseAllCount !== 1) {
  throw new Error("remote control loopback smoke did not release remote inputs exactly once");
}
if (report.localReleaseAllCount !== 1) {
  throw new Error("remote control loopback smoke did not release local inputs exactly once");
}

console.log("Remote control loopback smoke passed.");

function assertKind(outcome, expected, label) {
  if (outcome?.kind !== expected) {
    throw new Error(
      `remote control loopback smoke expected ${label} ${expected}, got ${outcome?.kind}`,
    );
  }
}

function assertPointer(event, label) {
  if (event?.kind !== "pointerMoved" || event.deltaX !== 8 || event.deltaY !== 2) {
    throw new Error(`remote control loopback smoke reported unexpected ${label}`);
  }
}

function assertButton(event, label) {
  if (event?.kind !== "mouseButton" || event.button !== "left" || event.state !== "pressed") {
    throw new Error(`remote control loopback smoke reported unexpected ${label}`);
  }
}

function assertScroll(event, label) {
  if (event?.kind !== "scroll" || event.deltaX !== 0 || event.deltaY !== -120) {
    throw new Error(`remote control loopback smoke reported unexpected ${label}`);
  }
}

function assertKey(event, expectedKey, label) {
  if (event?.kind !== "key" || event.key !== expectedKey || event.state !== "pressed") {
    throw new Error(`remote control loopback smoke reported unexpected ${label}`);
  }
}

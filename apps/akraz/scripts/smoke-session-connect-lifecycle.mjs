import { execFileSync, spawn, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));
const extension = process.platform === "win32" ? ".exe" : "";
const daemonExecutable = join(workspaceRoot, "target", "debug", `akraz-daemon${extension}`);
const ctlExecutable = join(workspaceRoot, "target", "debug", `akrazctl${extension}`);
const endpoint =
  process.platform === "win32"
    ? `\\\\.\\pipe\\akrazd-session-smoke-${process.pid}-${Date.now()}`
    : join(
        process.env.XDG_RUNTIME_DIR || process.env.TMPDIR || "/tmp",
        `akrazd-session-smoke-${process.pid}-${Date.now()}.sock`,
      );

execFileSync("cargo", ["build", "-p", "akraz-daemon", "-p", "akrazctl"], {
  cwd: workspaceRoot,
  stdio: "inherit",
});

const peerServer = await startPeerSessionServer();
const daemon = spawn(daemonExecutable, ["--serve", "--endpoint", endpoint], {
  cwd: workspaceRoot,
  stdio: ["ignore", "pipe", "pipe"],
});
const daemonOutput = [];

daemon.stdout.setEncoding("utf8");
daemon.stderr.setEncoding("utf8");
daemon.stdout.on("data", (chunk) => {
  daemonOutput.push(chunk);
  process.stdout.write(chunk);
});
daemon.stderr.on("data", (chunk) => {
  daemonOutput.push(chunk);
  process.stderr.write(chunk);
});

let disconnected = false;

try {
  const initial = await waitForDaemonStatus();
  assertStatusMode(initial, "Local", "initial status");
  assertPeerCount(initial, 0, "initial status");

  const connected = runCtlJson([
    "session",
    "connect",
    "--endpoint",
    endpoint,
    "--peer-id",
    "loopback-peer",
    "--local-device-id",
    "local-smoke-device",
    "--address",
    peerServer.address,
  ]);
  assertSessionConnected(connected);

  const afterConnect = runCtlJson(["status", "--endpoint", endpoint]);
  assertStatusMode(afterConnect, "Local", "connected status");
  assertPeerCount(afterConnect, 1, "connected status");
  assertPeer(afterConnect.result.peers[0], true);

  const disconnectedResponse = runCtlJson(["session", "disconnect", "--endpoint", endpoint]);
  disconnected = true;
  assertSessionDisconnected(disconnectedResponse);

  const peerSession = await withTimeout(
    peerServer.sessionClosed,
    5_000,
    "peer session did not close after disconnect",
  );
  assertPeerHello(peerSession.hello);
  if (peerSession.frames.length !== 1) {
    throw new Error(
      `session connect lifecycle smoke expected only a hello frame, got ${peerSession.frames.length} frames`,
    );
  }

  const afterDisconnect = runCtlJson(["status", "--endpoint", endpoint]);
  assertStatusMode(afterDisconnect, "Local", "disconnected status");
  assertPeerCount(afterDisconnect, 0, "disconnected status");

  console.log("Session connect lifecycle smoke passed.");
} finally {
  if (!disconnected) {
    spawnSync(ctlExecutable, ["session", "disconnect", "--endpoint", endpoint], {
      cwd: workspaceRoot,
      encoding: "utf8",
    });
  }
  peerServer.close();
  if (!daemon.killed) {
    daemon.kill();
  }
  await waitForDaemonExit(daemon);
}

async function startPeerSessionServer() {
  let listenAddress;
  const server = createServer();
  const sessionClosed = new Promise((resolve, reject) => {
    server.on("connection", (socket) => {
      const frames = [];
      let buffer = "";
      let settled = false;

      socket.setEncoding("utf8");
      socket.on("data", (chunk) => {
        buffer += chunk;
        while (buffer.includes("\n")) {
          const newlineIndex = buffer.indexOf("\n");
          const line = buffer.slice(0, newlineIndex).trim();
          buffer = buffer.slice(newlineIndex + 1);
          if (line) {
            frames.push(JSON.parse(line));
          }
        }
      });
      socket.on("error", reject);
      socket.on("close", () => {
        if (settled) {
          return;
        }
        settled = true;
        resolve({
          frames,
          hello: frames[0],
        });
      });
    });

    server.on("error", reject);
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      listenAddress = `${address.address}:${address.port}`;
      resolve();
    });
  });

  return {
    address: listenAddress,
    sessionClosed,
    close() {
      server.close();
    },
  };
}

async function waitForDaemonStatus(attempt = 0, lastError = "") {
  if (attempt >= 80) {
    throw new Error(`session connect lifecycle smoke could not reach daemon: ${lastError}`);
  }

  const result = spawnSync(ctlExecutable, ["status", "--endpoint", endpoint], {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (result.status === 0) {
    return parseJsonRpc(result.stdout, "daemon status");
  }

  await new Promise((resolve) => setTimeout(resolve, 50));

  return waitForDaemonStatus(
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

  return parseJsonRpc(result.stdout, `akrazctl ${args.join(" ")}`);
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

function assertPeer(peer, connected) {
  if (
    peer?.peerId !== "loopback-peer" ||
    peer.displayName !== "loopback-peer" ||
    peer.connected !== connected
  ) {
    throw new Error("session connect lifecycle smoke reported unexpected peer status");
  }
}

function assertSessionConnected(response) {
  if (response.result?.connected !== true) {
    throw new Error("session connect lifecycle smoke did not connect the session");
  }
  assertSessionStatus(response.result.session, true);
}

function assertSessionDisconnected(response) {
  if (response.result?.disconnected !== true || response.result.mode !== "Local") {
    throw new Error("session connect lifecycle smoke did not disconnect the session");
  }
  assertSessionStatus(response.result.session, false);
}

function assertSessionStatus(session, connected) {
  if (
    session?.peerId !== "loopback-peer" ||
    session.localDeviceId !== "local-smoke-device" ||
    session.connected !== connected
  ) {
    throw new Error("session connect lifecycle smoke reported unexpected session status");
  }
}

function assertPeerHello(hello) {
  if (
    hello?.kind !== "hello" ||
    hello.protocol?.major !== 1 ||
    hello.protocol.minor !== 0 ||
    hello.deviceId !== "local-smoke-device" ||
    hello.peerId !== "loopback-peer"
  ) {
    throw new Error("session connect lifecycle smoke reported unexpected peer hello frame");
  }
}

async function withTimeout(promise, timeoutMs, message) {
  let timeout;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error(message)), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timeout);
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

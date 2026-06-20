import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, resolve } from "node:path";

export const SIGNING_PREFLIGHT_SCHEMA_VERSION = "akraz.signing.preflight/v1";

const CHECKS = [
  {
    id: "tauriUpdaterPrivateKey",
    category: "updater",
    environment: ["TAURI_SIGNING_PRIVATE_KEY"],
    description: "Tauri updater signing private key is available.",
  },
  {
    id: "tauriUpdaterPrivateKeyPassword",
    category: "updater",
    environment: ["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"],
    description: "Tauri updater signing private key password is available.",
  },
  {
    id: "windowsSigningCertificate",
    category: "windowsInstaller",
    environment: ["AKRAZ_WINDOWS_SIGNING_CERT_PATH", "AKRAZ_WINDOWS_SIGNING_CERT_BASE64"],
    description: "Windows installer signing certificate is available.",
  },
  {
    id: "windowsSigningCertificatePassword",
    category: "windowsInstaller",
    environment: ["AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD"],
    description: "Windows installer signing certificate password is available.",
  },
];

export function evaluateSigningPreflight(env = process.env) {
  const checks = [
    requireNonEmptyEnv(CHECKS[0], env),
    requireNonEmptyEnv(CHECKS[1], env),
    evaluateWindowsCertificate(CHECKS[2], env),
    requireNonEmptyEnv(CHECKS[3], env),
  ];

  return {
    schemaVersion: SIGNING_PREFLIGHT_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    checks,
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function parseSigningPreflightArgs(args) {
  const options = {
    expectMissing: false,
    outFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--expect-missing":
        options.expectMissing = true;
        break;
      case "--out-file":
        options.outFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown signing preflight argument: ${arg}`);
    }
  }

  return options;
}

export function exitCodeForSigningPreflight(report, options = {}) {
  if (report.ready) {
    return options.expectMissing ? 1 : 0;
  }

  return options.expectMissing && hasOnlyMissingChecks(report) ? 0 : 1;
}

export function writeSigningPreflightOutputFile(outFile, payload) {
  if (!outFile) {
    return undefined;
  }

  const resolvedOutFile = resolve(outFile);
  const outDirectory = dirname(resolvedOutFile);
  const tempFile = resolve(
    outDirectory,
    `.${basename(resolvedOutFile)}.${process.pid}.${Date.now()}.tmp`,
  );
  const serializedPayload = `${JSON.stringify(payload, null, 2)}\n`;

  mkdirSync(outDirectory, { recursive: true });

  let fileDescriptor;
  try {
    fileDescriptor = openSync(tempFile, "w", 0o600);
    writeFileSync(fileDescriptor, serializedPayload, "utf8");
    fsyncSync(fileDescriptor);
    closeSync(fileDescriptor);
    fileDescriptor = undefined;
    renameSync(tempFile, resolvedOutFile);
  } catch (error) {
    if (fileDescriptor !== undefined) {
      closeSync(fileDescriptor);
    }
    if (existsSync(tempFile)) {
      rmSync(tempFile, { force: true });
    }
    throw error;
  }

  return resolvedOutFile;
}

function hasOnlyMissingChecks(report) {
  return (
    report.checks.some((check) => check.status === "missing") &&
    report.checks.every((check) => check.status === "pass" || check.status === "missing")
  );
}

function requireNonEmptyEnv(definition, env) {
  return {
    ...publicCheckFields(definition),
    status: hasEnvValue(env, definition.environment[0]) ? "pass" : "missing",
  };
}

function evaluateWindowsCertificate(definition, env) {
  const hasPath = hasEnvValue(env, "AKRAZ_WINDOWS_SIGNING_CERT_PATH");
  const hasBase64 = hasEnvValue(env, "AKRAZ_WINDOWS_SIGNING_CERT_BASE64");

  if (!hasPath && !hasBase64) {
    return {
      ...publicCheckFields(definition),
      status: "missing",
      detail: "certificateSourceMissing",
    };
  }

  if (hasPath && !existsSync(env.AKRAZ_WINDOWS_SIGNING_CERT_PATH)) {
    return {
      ...publicCheckFields(definition),
      status: "invalid",
      detail: "certificatePathMissing",
      source: "path",
    };
  }

  return {
    ...publicCheckFields(definition),
    status: "pass",
    source: hasPath ? "path" : "base64",
  };
}

function publicCheckFields(definition) {
  return {
    id: definition.id,
    category: definition.category,
    environment: definition.environment,
    description: definition.description,
  };
}

function hasEnvValue(env, name) {
  return typeof env[name] === "string" && env[name].trim().length > 0;
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseSigningPreflightArgs(process.argv.slice(2));
  const report = evaluateSigningPreflight();
  writeSigningPreflightOutputFile(options.outFile, report);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForSigningPreflight(report, options));
}

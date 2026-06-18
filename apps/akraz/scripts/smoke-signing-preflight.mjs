import { existsSync } from "node:fs";

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
  return {
    expectMissing: args.includes("--expect-missing"),
  };
}

export function exitCodeForSigningPreflight(report, options = {}) {
  if (report.ready) {
    return options.expectMissing ? 1 : 0;
  }

  return options.expectMissing && hasOnlyMissingChecks(report) ? 0 : 1;
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

if (import.meta.main) {
  const options = parseSigningPreflightArgs(process.argv.slice(2));
  const report = evaluateSigningPreflight();
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForSigningPreflight(report, options));
}

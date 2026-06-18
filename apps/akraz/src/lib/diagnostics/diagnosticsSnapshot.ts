import type { DiagnosticsSnapshot, DiagnosticsSupportBundle } from "../api/types";

export function formatDiagnosticsSnapshot(snapshot: DiagnosticsSnapshot): string {
  return JSON.stringify(snapshot, null, 2);
}

export function formatDiagnosticsSupportBundle(bundle: DiagnosticsSupportBundle): string {
  return JSON.stringify(bundle, null, 2);
}

export function screenTopologySummary(snapshot: DiagnosticsSnapshot): string {
  const topology = snapshot.screenTopology;
  if (!topology) {
    return "확인 안 됨";
  }

  const bounds = topology.virtualScreenBounds;
  return `${bounds.width}x${bounds.height} @ ${bounds.x},${bounds.y}`;
}

export function latencySummary(snapshot: DiagnosticsSnapshot): string {
  const latency = snapshot.latencyHistogram;
  if (!latency) {
    return "확인 안 됨";
  }

  return `평균 ${formatMicrosAsMillis(latency.averageMicros)} · p95 ${formatMicrosAsMillis(latency.p95Micros)} · p99 ${formatMicrosAsMillis(latency.p99Micros)}`;
}

export function includedSectionsSummary(bundle: DiagnosticsSupportBundle): string {
  return bundle.includedSections.join(", ");
}

export function unavailableSectionsSummary(snapshot: DiagnosticsSnapshot): string {
  return snapshot.unavailableSections.length === 0
    ? "없음"
    : snapshot.unavailableSections.join(", ");
}

function formatMicrosAsMillis(micros: number): string {
  return `${(micros / 1000).toFixed(2)}ms`;
}

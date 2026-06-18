import type {
  DiagnosticsScreenTopology,
  IdentityTrustedPeer,
  LayoutSettings,
  ScreenEdge,
} from "../api/types";

export type LayoutMismatchStatus = "ready" | "needs-action" | "limited";

export type LayoutMismatchIssueCode =
  | "missing-binding"
  | "empty-peer-id"
  | "unknown-peer"
  | "duplicate-local-edge"
  | "missing-topology"
  | "invalid-topology";

export interface LayoutMismatchIssue {
  code: LayoutMismatchIssueCode;
  status: Exclude<LayoutMismatchStatus, "ready">;
  message: string;
}

export interface LayoutMismatchReport {
  status: LayoutMismatchStatus;
  issues: LayoutMismatchIssue[];
  validBindingCount: number;
  bindingCount: number;
  trustedPeerCount: number;
  hasUsableTopology: boolean;
}

export interface AnalyzeLayoutMismatchInput {
  layout: LayoutSettings;
  trustedPeers: IdentityTrustedPeer[];
  topology: DiagnosticsScreenTopology | null;
}

const daemonStartBlockingIssueCodes = new Set<LayoutMismatchIssueCode>([
  "empty-peer-id",
  "unknown-peer",
  "duplicate-local-edge",
]);

const issueMessages: Record<LayoutMismatchIssueCode, string> = {
  "missing-binding": "화면 배치에서 넘어갈 경계를 추가해.",
  "empty-peer-id": "기기가 비어 있는 경계가 있어.",
  "unknown-peer": "신뢰 목록에 없는 기기가 배치에 남아 있어.",
  "duplicate-local-edge": "같은 화면 끝에 여러 기기가 붙어 있어.",
  "missing-topology": "화면 확인을 하면 현재 해상도와 배치를 함께 검사할 수 있어.",
  "invalid-topology": "현재 화면 범위를 읽지 못했어.",
};

export function isUsableScreenTopology(topology: DiagnosticsScreenTopology | null): boolean {
  if (!topology) {
    return false;
  }

  const bounds = topology.virtualScreenBounds;
  return (
    Number.isFinite(bounds.x) &&
    Number.isFinite(bounds.y) &&
    Number.isFinite(bounds.width) &&
    Number.isFinite(bounds.height) &&
    bounds.width > 0 &&
    bounds.height > 0
  );
}

export function analyzeLayoutMismatch(input: AnalyzeLayoutMismatchInput): LayoutMismatchReport {
  const trustedPeerIds = new Set(input.trustedPeers.map((peer) => peer.peerId));
  const seenLocalEdges = new Set<ScreenEdge>();
  const issues = new Map<LayoutMismatchIssueCode, LayoutMismatchIssue>();
  let validBindingCount = 0;

  if (input.layout.edgeBindings.length === 0) {
    addIssue(issues, "missing-binding", "needs-action");
  }

  for (const binding of input.layout.edgeBindings) {
    const peerId = binding.peerId.trim();
    const isDuplicateLocalEdge = seenLocalEdges.has(binding.localEdge);
    seenLocalEdges.add(binding.localEdge);

    if (peerId.length === 0) {
      addIssue(issues, "empty-peer-id", "needs-action");
      continue;
    }

    const hasTrustedPeer = trustedPeerIds.has(peerId);
    if (!hasTrustedPeer) {
      addIssue(issues, "unknown-peer", "needs-action");
    }

    if (isDuplicateLocalEdge) {
      addIssue(issues, "duplicate-local-edge", "needs-action");
    }

    if (hasTrustedPeer && !isDuplicateLocalEdge) {
      validBindingCount += 1;
    }
  }

  const hasUsableTopology = isUsableScreenTopology(input.topology);
  if (!input.topology) {
    addIssue(issues, "missing-topology", "limited");
  } else if (!hasUsableTopology) {
    addIssue(issues, "invalid-topology", "needs-action");
  }

  const issueList = Array.from(issues.values());
  return {
    status: layoutMismatchStatus(issueList),
    issues: issueList,
    validBindingCount,
    bindingCount: input.layout.edgeBindings.length,
    trustedPeerCount: input.trustedPeers.length,
    hasUsableTopology,
  };
}

export function firstLayoutDaemonStartBlockingIssue(
  report: LayoutMismatchReport,
): LayoutMismatchIssue | null {
  return report.issues.find((issue) => daemonStartBlockingIssueCodes.has(issue.code)) ?? null;
}

function addIssue(
  issues: Map<LayoutMismatchIssueCode, LayoutMismatchIssue>,
  code: LayoutMismatchIssueCode,
  status: LayoutMismatchIssue["status"],
) {
  if (issues.has(code)) {
    return;
  }

  issues.set(code, {
    code,
    status,
    message: issueMessages[code],
  });
}

function layoutMismatchStatus(issues: LayoutMismatchIssue[]): LayoutMismatchStatus {
  if (issues.some((issue) => issue.status === "needs-action")) {
    return "needs-action";
  }

  if (issues.length > 0) {
    return "limited";
  }

  return "ready";
}

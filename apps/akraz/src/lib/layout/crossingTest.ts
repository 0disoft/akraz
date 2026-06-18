import type {
  DiagnosticsScreenTopology,
  LogicalPointSnapshot,
  ScreenEdge,
  ScreenEdgeBinding,
} from "../api/types";
import { isUsableScreenTopology } from "./layoutMismatch";

export interface LayoutCrossingPreview {
  peerId: string;
  localEdge: ScreenEdge;
  remoteEdge: ScreenEdge;
  exitPosition: LogicalPointSnapshot;
  edgeOffset: number;
}

function midpoint(start: number, size: number): number {
  return Math.round(start + size / 2);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

export function previewEdgeCrossing(
  binding: ScreenEdgeBinding,
  topology: DiagnosticsScreenTopology,
): LayoutCrossingPreview | null {
  const peerId = binding.peerId.trim();
  if (peerId.length === 0 || !isUsableScreenTopology(topology)) {
    return null;
  }

  const bounds = topology.virtualScreenBounds;
  const maxX = bounds.x + bounds.width - 1;
  const maxY = bounds.y + bounds.height - 1;
  const midX = clamp(midpoint(bounds.x, bounds.width), bounds.x, maxX);
  const midY = clamp(midpoint(bounds.y, bounds.height), bounds.y, maxY);

  if (binding.localEdge === "left") {
    return {
      peerId,
      localEdge: binding.localEdge,
      remoteEdge: binding.remoteEdge,
      exitPosition: { x: bounds.x - 1, y: midY },
      edgeOffset: midY,
    };
  }

  if (binding.localEdge === "right") {
    return {
      peerId,
      localEdge: binding.localEdge,
      remoteEdge: binding.remoteEdge,
      exitPosition: { x: bounds.x + bounds.width, y: midY },
      edgeOffset: midY,
    };
  }

  if (binding.localEdge === "top") {
    return {
      peerId,
      localEdge: binding.localEdge,
      remoteEdge: binding.remoteEdge,
      exitPosition: { x: midX, y: bounds.y - 1 },
      edgeOffset: midX,
    };
  }

  return {
    peerId,
    localEdge: binding.localEdge,
    remoteEdge: binding.remoteEdge,
    exitPosition: { x: midX, y: bounds.y + bounds.height },
    edgeOffset: midX,
  };
}

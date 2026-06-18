import { describe, expect, test } from "bun:test";

import { previewEdgeCrossing } from "../src/lib/layout/crossingTest";
import type { DiagnosticsScreenTopology, ScreenEdgeBinding } from "../src/lib/api/types";

const topology: DiagnosticsScreenTopology = {
  pointerPosition: { x: 960, y: 540 },
  virtualScreenBounds: { x: 0, y: 0, width: 1920, height: 1080 },
  monitors: [
    {
      id: "primary",
      bounds: { x: 0, y: 0, width: 1920, height: 1080 },
      scaleFactorPercent: 100,
      isPrimary: true,
    },
  ],
};

function binding(localEdge: ScreenEdgeBinding["localEdge"]): ScreenEdgeBinding {
  return {
    localEdge,
    peerId: " linux-laptop ",
    remoteEdge: localEdge === "left" ? "right" : "left",
  };
}

describe("layout crossing preview", () => {
  test("previews a right edge crossing with trimmed peer id", () => {
    expect(previewEdgeCrossing(binding("right"), topology)).toEqual({
      peerId: "linux-laptop",
      localEdge: "right",
      remoteEdge: "left",
      exitPosition: { x: 1920, y: 540 },
      edgeOffset: 540,
    });
  });

  test("previews a top edge crossing with horizontal edge offset", () => {
    expect(previewEdgeCrossing(binding("top"), topology)).toEqual({
      peerId: "linux-laptop",
      localEdge: "top",
      remoteEdge: "left",
      exitPosition: { x: 960, y: -1 },
      edgeOffset: 960,
    });
  });

  test("rejects a binding without a peer id", () => {
    expect(
      previewEdgeCrossing(
        {
          localEdge: "bottom",
          peerId: " ",
          remoteEdge: "top",
        },
        topology,
      ),
    ).toBeNull();
  });

  test("rejects unusable screen topology", () => {
    expect(
      previewEdgeCrossing(binding("right"), {
        pointerPosition: { x: 0, y: 0 },
        virtualScreenBounds: { x: 0, y: 0, width: -1, height: 1080 },
        monitors: [],
      }),
    ).toBeNull();
  });
});

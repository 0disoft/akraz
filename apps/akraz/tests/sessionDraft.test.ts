import { describe, expect, test } from "bun:test";

import { selectTrustedPeerSessionDraft } from "../src/lib/session/sessionDraft";

describe("session draft trusted peer selection", () => {
  test("fills peer id, saved address, and local device id", () => {
    expect(
      selectTrustedPeerSessionDraft(
        { peerId: "", localDeviceId: "", address: "" },
        "linux-laptop",
        "127.0.0.1:4455",
        "windows-desktop",
      ),
    ).toEqual({
      peerId: "linux-laptop",
      localDeviceId: "windows-desktop",
      address: "127.0.0.1:4455",
    });
  });

  test("keeps the current local device id when local identity is unavailable", () => {
    expect(
      selectTrustedPeerSessionDraft(
        {
          peerId: "old-peer",
          localDeviceId: "manual-local",
          address: "127.0.0.1:4400",
        },
        "linux-laptop",
        "127.0.0.1:4455",
        null,
      ),
    ).toEqual({
      peerId: "linux-laptop",
      localDeviceId: "manual-local",
      address: "127.0.0.1:4455",
    });
  });

  test("ignores the direct-input option", () => {
    const draft = {
      peerId: "manual-peer",
      localDeviceId: "manual-local",
      address: "127.0.0.1:4455",
    };

    expect(selectTrustedPeerSessionDraft(draft, "", "", "windows-desktop")).toBe(draft);
  });
});

import { invoke } from "@tauri-apps/api/core";

import type { LayoutSettings } from "./types";

export const layoutClient = {
  get(): Promise<LayoutSettings> {
    return invoke<LayoutSettings>("layout_get");
  },

  set(layout: LayoutSettings): Promise<LayoutSettings> {
    return invoke<LayoutSettings>("layout_set", { layout });
  },
};

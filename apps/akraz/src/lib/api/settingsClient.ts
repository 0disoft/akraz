import { invoke } from "@tauri-apps/api/core";

import type { AppSettings } from "./types";

export const settingsClient = {
  load(): Promise<AppSettings> {
    return invoke<AppSettings>("settings_load");
  },

  save(settings: AppSettings): Promise<AppSettings> {
    return invoke<AppSettings>("settings_save", { settings });
  },
};

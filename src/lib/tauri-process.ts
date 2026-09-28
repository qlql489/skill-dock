// Minimal binding for tauri-plugin-process — the relaunch used to finish an
// app update install.

import { invoke } from "@tauri-apps/api/core";

export async function relaunch(): Promise<void> {
  await invoke("plugin:process|restart");
}

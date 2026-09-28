// Minimal hand-rolled bindings for tauri-plugin-updater (JS guest bindings).
// Mirrors the official @tauri-apps/plugin-updater API surface but depends only
// on @tauri-apps/api/core, so we don't need the npm plugin package.

import { Channel, invoke, Resource } from "@tauri-apps/api/core";

export interface CheckOptions {
  headers?: HeadersInit;
  timeout?: number;
  proxy?: string;
  target?: string;
  allowDowngrades?: boolean;
}

export interface DownloadOptions {
  headers?: HeadersInit;
  timeout?: number;
}

type UpdateMetadata = {
  rid: number;
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  rawJson: Record<string, unknown>;
};

export type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished"; data: Record<string, never> };

function convertToRustHeaders(options?: { headers?: HeadersInit }): void {
  if (options?.headers) {
    options.headers = Array.from(new Headers(options.headers).entries());
  }
}

export class Update extends Resource {
  available = true;
  currentVersion: string;
  version: string;
  date?: string;
  body?: string;
  rawJson: Record<string, unknown>;
  private downloadedBytes?: Resource;

  constructor(metadata: UpdateMetadata) {
    super(metadata.rid);
    this.currentVersion = metadata.currentVersion;
    this.version = metadata.version;
    this.date = metadata.date;
    this.body = metadata.body;
    this.rawJson = metadata.rawJson;
  }

  async download(
    onEvent?: (progress: DownloadEvent) => void,
    options?: DownloadOptions,
  ): Promise<void> {
    convertToRustHeaders(options);
    const channel = new Channel<DownloadEvent>();
    if (onEvent) {
      channel.onmessage = onEvent;
    }
    const downloadedBytesRid = await invoke<number>("plugin:updater|download", {
      onEvent: channel,
      rid: this.rid,
      ...options,
    });
    this.downloadedBytes = new Resource(downloadedBytesRid);
  }

  async install(): Promise<void> {
    if (!this.downloadedBytes) {
      throw new Error("Update.install called before Update.download");
    }
    await invoke("plugin:updater|install", {
      updateRid: this.rid,
      bytesRid: this.downloadedBytes.rid,
    });
    this.downloadedBytes = undefined;
  }

  async downloadAndInstall(
    onEvent?: (progress: DownloadEvent) => void,
    options?: DownloadOptions,
  ): Promise<void> {
    convertToRustHeaders(options);
    const channel = new Channel<DownloadEvent>();
    if (onEvent) {
      channel.onmessage = onEvent;
    }
    await invoke("plugin:updater|download_and_install", {
      onEvent: channel,
      rid: this.rid,
      ...options,
    });
  }

  async close(): Promise<void> {
    await this.downloadedBytes?.close();
    await super.close();
  }
}

export async function check(options?: CheckOptions): Promise<Update | null> {
  convertToRustHeaders(options);
  const metadata = await invoke<UpdateMetadata | null>("plugin:updater|check", {
    ...options,
  });
  return metadata ? new Update(metadata) : null;
}

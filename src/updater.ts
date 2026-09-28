// App self-update over tauri-plugin-updater. One module owns the whole state
// machine (idle → checking → available → downloading → ready) plus the silent
// background check loop; the settings page just renders `getUpdateState()`.
//
// The updater is config-gated: until the pubkey/endpoints in tauri.conf.json
// carry real values (no REPLACE_WITH_* placeholders), every entry point
// degrades to a friendly "未配置" state instead of network errors.

import { getVersion } from "@tauri-apps/api/app";
import tauriConfig from "../src-tauri/tauri.conf.json";
import { check, type Update } from "./lib/tauri-updater";
import { relaunch } from "./lib/tauri-process";
import { toast } from "./toast";

export type UpdateStatus =
  | "idle" // 尚未检查过
  | "checking" // 检查中
  | "available" // 发现新版本,等待下载
  | "downloading" // 下载安装中(带进度)
  | "ready" // 安装完成,等待重启
  | "latest" // 已是最新
  | "error"; // 检查/下载失败,原因见 message

export interface UpdateState {
  status: UpdateStatus;
  /** 新版本号;available / downloading / ready 时有值。 */
  version: string | null;
  /** 0-100,仅 downloading 阶段有效。 */
  progress: number;
  /** error 时的可读原因,或未配置时的说明。 */
  message: string | null;
}

type UpdaterPluginConfig = { pubkey?: string; endpoints?: string[] };

const updaterConfig =
  (tauriConfig as { plugins?: { updater?: UpdaterPluginConfig } }).plugins?.updater ?? {};

const PLACEHOLDER_MARKERS = ["REPLACE_WITH", "your-name", "your-repo"];

function hasPlaceholder(value: string): boolean {
  return PLACEHOLDER_MARKERS.some((marker) => value.includes(marker));
}

const configuredPubkey = updaterConfig.pubkey?.trim() ?? "";
const configuredEndpoints = (updaterConfig.endpoints ?? []).map((e) => e.trim()).filter(Boolean);

/** 更新源是否已真正配置(公钥 + endpoint 都填了真实值)。 */
export const UPDATER_ENABLED: boolean =
  configuredPubkey.length > 0 &&
  !hasPlaceholder(configuredPubkey) &&
  configuredEndpoints.length > 0 &&
  configuredEndpoints.every((endpoint) => !hasPlaceholder(endpoint));

export const UPDATER_DISABLED_REASON = !configuredPubkey
  ? "自动更新未启用:缺少 updater 公钥(见 README 的发布流程)。"
  : configuredEndpoints.length === 0
    ? "自动更新未启用:缺少更新源地址。"
    : "自动更新未启用:请先把 tauri.conf.json 里的 pubkey 与 endpoint 换成真实值。";

let state: UpdateState = { status: "idle", version: null, progress: 0, message: null };
let handle: Update | null = null;
const listeners = new Set<() => void>();
let appVersionPromise: Promise<string> | null = null;

export function getUpdateState(): UpdateState {
  return state;
}

export function onUpdateState(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function setState(patch: Partial<UpdateState>): void {
  state = { ...state, ...patch };
  for (const listener of listeners) listener();
}

/** 真实应用版本(tauri.conf.json 的 version 引用 package.json)。结果缓存。 */
export function getAppVersion(): Promise<string> {
  appVersionPromise ??= getVersion();
  return appVersionPromise;
}

/** 把插件抛出的底层错误翻译成用户能看懂的提示。 */
function mapUpdateErrorMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  const normalized = raw.toLowerCase();
  if (normalized.includes("404")) {
    return "没有找到更新清单(latest.json),请确认 GitHub Release 已发布并上传了 latest.json。";
  }
  if (normalized.includes("signature") || normalized.includes("keyid")) {
    return "更新包签名校验失败:Release 产物必须用与客户端公钥配对的私钥签名。";
  }
  if (normalized.includes("tls") || normalized.includes("certificate")) {
    return "更新地址证书校验失败,请确认下载地址可通过 HTTPS 访问。";
  }
  return raw;
}

/**
 * 检查一次更新。silent=true 用于后台轮询:发现新版本只弹一次 toast,
 * 失败不打扰;手动检查(silent=false)则成功失败都给出反馈。
 */
export async function checkForAppUpdate(options: { silent?: boolean } = {}): Promise<void> {
  const { silent = false } = options;
  if (!UPDATER_ENABLED) {
    if (!silent) setState({ status: "error", message: UPDATER_DISABLED_REASON });
    return;
  }
  if (state.status === "checking" || state.status === "downloading") return;
  setState({ status: "checking", message: null });
  try {
    const update = await check();
    if (update) {
      const firstDiscovery = handle === null;
      handle = update;
      setState({ status: "available", version: update.version, message: null });
      if (silent && firstDiscovery) {
        toast(`发现新版本 v${update.version},可在 设置 → 应用更新 安装`, "info");
      }
    } else {
      await handle?.close().catch(() => {});
      handle = null;
      setState({ status: "latest", version: null, message: null });
      if (!silent) toast("已是最新版本", "success");
    }
  } catch (error) {
    await handle?.close().catch(() => {});
    handle = null;
    setState({ status: "error", message: mapUpdateErrorMessage(error) });
  }
}

/** 下载并安装新版本(不重启;重启交给 relaunchToUpdate)。 */
export async function downloadAndInstallUpdate(): Promise<void> {
  if (!handle) return;
  setState({ status: "downloading", progress: 0, message: null });
  let total = 0;
  let done = 0;
  try {
    await handle.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? 0;
      } else if (event.event === "Progress") {
        done += event.data.chunkLength;
        const percent = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
        setState({ progress: percent });
      }
    });
    setState({ status: "ready", progress: 100 });
  } catch (error) {
    setState({ status: "error", message: mapUpdateErrorMessage(error) });
  }
}

export async function relaunchToUpdate(): Promise<void> {
  try {
    await relaunch();
  } catch (error) {
    setState({ status: "error", message: `重启失败:${error}` });
  }
}

const AUTO_CHECK_DELAY_MS = 8_000; // 启动后等首屏稳定
const AUTO_CHECK_INTERVAL_MS = 10 * 60_000; // 之后每 10 分钟静默复查

/** 启动后台自动检查;更新源未配置时是 no-op。在 boot() 里调用一次。 */
export function initAutoUpdateCheck(): void {
  if (!UPDATER_ENABLED) {
    console.info("[updater] disabled: updater config still has placeholders");
    return;
  }
  window.setTimeout(() => void checkForAppUpdate({ silent: true }), AUTO_CHECK_DELAY_MS);
  window.setInterval(() => void checkForAppUpdate({ silent: true }), AUTO_CHECK_INTERVAL_MS);
}

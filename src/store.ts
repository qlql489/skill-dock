// Tiny store — single in-memory snapshot of backend state, plus
// a tiny pub/sub for view re-renders.

import { api } from "./api";
import type { StateSnapshot, UpdateReport } from "./types";

type Listener = () => void;

class Store {
  private snapshot: StateSnapshot | null = null;
  private listeners = new Set<Listener>();
  private loading = false;

  async load(): Promise<void> {
    this.loading = true;
    try {
      this.snapshot = await api.getState();
      this.notify();
    } finally {
      this.loading = false;
    }
  }

  async refresh(): Promise<void> {
    await this.load();
  }

  get(): StateSnapshot {
    if (!this.snapshot) throw new Error("store not loaded");
    return this.snapshot;
  }

  hasUpdateReport(): UpdateReport | null {
    return this.snapshot?.update_report ?? null;
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private notify() {
    for (const fn of this.listeners) fn();
  }
}

export const store = new Store();

// ----- helpers -----

export function skillById(id: string) {
  return store.get().skills.find((s) => s.id === id);
}

export function targetById(id: string) {
  return store.get().targets.find((t) => t.id === id);
}

export function sourceById(id: string) {
  return store.get().sources.find((s) => s.id === id);
}

export function installedTargets(skillId: string) {
  return store.get().installations.filter((i) => i.skill_id === skillId && isHealthy(i));
}

export function isInstalled(skillId: string, targetId: string) {
  return store
    .get()
    .installations.some((i) => i.skill_id === skillId && i.target_id === targetId && isHealthy(i));
}

/** 健康记录才计入"已安装"。missing/conflict 记录保留在账本里供概览页
 *  展示，但对 UI 而言不等于"装着"。 */
export function isHealthy(i: { status?: string }): boolean {
  const s = i.status ?? "";
  return s === "" || s === "ok";
}

/** 所有健康安装记录（需要遍历时用，避免每处重复过滤逻辑）。 */
export function healthyInstallations() {
  return store.get().installations.filter(isHealthy);
}

/** 显示用：把 /Users/xxx 开头的绝对路径折叠成 ~/，任何要渲染给用户看的
 *  路径都应先过这个函数（title 提示里保留原始绝对路径）。 */
export function tildePath(p: string): string {
  return (p ?? "").replace(/^\/Users\/[^/]+/, "~");
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function formatRelative(iso: string | null): string {
  if (!iso) return "—";
  const t = new Date(iso).getTime();
  const now = Date.now();
  const diff = now - t;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  if (diff < 30 * 86_400_000) return `${Math.floor(diff / 86_400_000)}d ago`;
  return new Date(iso).toLocaleDateString();
}

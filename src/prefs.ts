// 应用级偏好（存后端 state.json 的 ui 段）。主题色走 appearance.ts；
// 这里是「添加来源默认勾选 Agents」开关与「中央仓库路径」：启动时预载
// 一份内存缓存，设置页（同步渲染）、发现页收编弹窗直接读缓存，写操作
// 先更缓存再落盘。

import { api } from "./api";

let addDefaultInstallTargets = true;

export function getAddDefaultInstallTargets(): boolean {
  return addDefaultInstallTargets;
}

/** 启动时调用；旧后端没有该命令时保持默认 true（= 原行为）。 */
export async function loadAddDefaultInstallTargets(): Promise<void> {
  try {
    addDefaultInstallTargets = await api.getAddDefaultInstallTargets();
  } catch {
    // 静默走默认
  }
}

/** 保存失败时回滚缓存并抛错（调用方 toast）。 */
export async function setAddDefaultInstallTargets(enabled: boolean): Promise<void> {
  const prev = addDefaultInstallTargets;
  addDefaultInstallTargets = enabled;
  try {
    await api.setAddDefaultInstallTargets(enabled);
  } catch (e) {
    addDefaultInstallTargets = prev;
    throw e;
  }
}

// ----- 中央仓库路径（收编默认目的地）-----

/** 生效路径（绝对路径）。预载完成前先给内置默认，仅影响瞬时显示。 */
let vaultPath = "~/.skill-dock/skills";

export function getVaultPath(): string {
  return vaultPath;
}

/** 启动时调用；旧后端没有该命令时保持内置默认。 */
export async function loadVaultPath(): Promise<void> {
  try {
    vaultPath = await api.getVaultPath();
  } catch {
    // 静默走默认
  }
}

/** 保存失败时回滚缓存并抛错（调用方 toast）。 */
export async function setVaultPath(path: string): Promise<void> {
  const prev = vaultPath;
  vaultPath = path;
  try {
    await api.setVaultPath(path);
  } catch (e) {
    vaultPath = prev;
    throw e;
  }
}

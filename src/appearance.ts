// 外观（主题色）—— 机制对齐 aite 项目：
// 预设色板 + 自定义取色 → 后端持久化 → 运行时把 RGB 三元组写入
// :root CSS 变量，所有 accent 系变量（accent/hover/soft/line/阴影）
// 由三元组派生，一处切换全局生效。

import { api } from "./api";

export interface AccentPreset {
  name: string;
  value: string;
}

export const ACCENT_PRESETS: AccentPreset[] = [
  { name: "默认蓝", value: "#3b82f6" },
  { name: "翠绿", value: "#10b981" },
  { name: "紫色", value: "#8b5cf6" },
  { name: "粉色", value: "#ec4899" },
  { name: "橙色", value: "#f97316" },
  { name: "青色", value: "#06b6d4" },
  { name: "红色", value: "#ef4444" },
  { name: "琥珀", value: "#f59e0b" },
  { name: "墨黑", value: "#1a1a1a" },
];

export const DEFAULT_ACCENT = "#3b82f6";

let currentAccent = DEFAULT_ACCENT;

export function getAccent(): string {
  return currentAccent;
}

function hexToRgb(hex: string): { r: number; g: number; b: number } | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 };
}

/** 应用主题色：写入三元组 + 派生 hover 深一档。 */
export function applyAccentColor(hex: string): boolean {
  const rgb = hexToRgb(hex);
  if (!rgb) return false;
  currentAccent = hex.startsWith("#") ? hex : `#${hex}`;
  const root = document.documentElement;
  root.style.setProperty("--accent-rgb", `${rgb.r}, ${rgb.g}, ${rgb.b}`);
  const f = 0.86; // hover/active 统一压暗 14%
  root.style.setProperty(
    "--accent-hi-rgb",
    `${Math.round(rgb.r * f)}, ${Math.round(rgb.g * f)}, ${Math.round(rgb.b * f)}`,
  );
  return true;
}

/** 启动时恢复持久化的主题色（在任何页面渲染前调用）。 */
export async function initAppearance(): Promise<void> {
  try {
    const saved = await api.getAccentColor();
    if (saved) applyAccentColor(saved);
  } catch {
    // 旧后端没有该命令时静默走默认蓝
  }
}

/** 设置并持久化（设置页调用）。返回是否合法。 */
export async function changeAccent(hex: string): Promise<boolean> {
  if (!applyAccentColor(hex)) return false;
  try {
    await api.setAccentColor(currentAccent);
  } catch (e) {
    // 持久化失败不阻塞预览，下次启动会回到默认
    console.error("保存主题色失败:", e);
  }
  return true;
}

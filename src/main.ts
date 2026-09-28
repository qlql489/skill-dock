// Main entry — wire up navigation, store load, and update banner.

import { store, tildePath } from "./store";
import { renderLibrary } from "./views/library";
import { renderMarket } from "./views/market";
import { renderDiscover } from "./views/discover";
import { renderTargets } from "./views/targets";
import { renderProjects, renderProjectDetail } from "./views/projects";
import { renderAgentSkills } from "./views/agent-skills";
import { renderSourceSkills } from "./views/source-skills";
import { renderPending } from "./views/pending";
import { renderSettings, type SettingsSectionId } from "./views/settings";
import { renderSkillPage, renderLocalSkillPage } from "./views/skill";
import type { SkillLocationKind } from "./types";
import { initAppearance } from "./appearance";
import { loadAddDefaultInstallTargets, loadVaultPath } from "./prefs";
import { getAppVersion, initAutoUpdateCheck } from "./updater";
import { paintIcons } from "./icon";
import { toast } from "./toast";
import { listen } from "@tauri-apps/api/event";
import { maybePromptPostAdd, cancelPendingPostAdd } from "./post-add";
import { openAddPicker } from "./add-picker";

/** A parsed route. Flat routes are simple strings; sub-routes carry params. */
type Route =
  | "library"
  | "market"
  | "discover"
  | "targets"
  | "pending"
  | "projects"
  | { name: "project"; projectId: string }
  | { name: "settings"; section: SettingsSectionId }
  | { name: "agent-skills"; targetId: string; kind?: SkillLocationKind }
  | { name: "source-skills"; sourceId: string }
  | { name: "skill"; skillId: string }
  | { name: "local-skill"; path: string };

const mount = () => document.getElementById("view-mount")!;
const railButtons = () => Array.from(document.querySelectorAll<HTMLButtonElement>(".rail__btn[data-route]"));

function setRoute(r: Route) {
  const isSettings = typeof r !== "string" && r.name === "settings";
  document.querySelector<HTMLElement>(".layout")?.classList.toggle("layout--settings", isSettings);

  // Only highlight rail buttons for top-level routes.
  const topName = typeof r === "string" ? r : isSettings ? "settings" : "";
  railButtons().forEach((b) => {
    b.setAttribute("data-active", String(b.dataset.route === topName));
  });
  const m = mount();
  m.dispatchEvent(new Event("cleanup"));
  m.innerHTML = "";
  // agent-skills / source-skills / settings are full-bleed, self-contained panels (own
  // header + 3-col grid) — they must not inherit the standard `.view` padding,
  // or the content column collapses to near-zero width and text wraps to one
  // char per line.
  m.classList.toggle(
    "view--bleed",
    typeof r !== "string" && (r.name === "agent-skills" || r.name === "source-skills" || r.name === "settings"),
  );
  if (typeof r === "string") {
    switch (r) {
      case "library": renderLibrary(m); break;
      case "market": renderMarket(m); break;
      case "discover": renderDiscover(m); break;
      case "targets": renderTargets(m); break;
      case "pending": renderPending(m); break;
      case "projects": renderProjects(m); break;
    }
  } else if (r.name === "settings") {
    renderSettings(m, r.section);
  } else if (r.name === "agent-skills") {
    renderAgentSkills(m, r.targetId, r.kind ?? null);
  } else if (r.name === "source-skills") {
    renderSourceSkills(m, r.sourceId);
  } else if (r.name === "skill") {
    renderSkillPage(m, r.skillId);
  } else if (r.name === "local-skill") {
    renderLocalSkillPage(m, r.path);
  } else if (r.name === "project") {
    renderProjectDetail(m, r.projectId);
  }
  // Update hash without triggering a re-route.
  // skillId 必须与 openSkillDetail 一致地 encodeURIComponent：skill id 形如
  // "source_id:相对路径"（含冒号/中文/斜杠），若此处写原始形式，浏览器会把
  // 编码版与原始版当作两个不同历史条目，返回键就会在等价条目间打转（表现为
  // 点返回没反应）。
  location.hash =
    typeof r === "string"
      ? `#/${r}`
      : r.name === "agent-skills"
        ? `#/agent-skills/${encodeURIComponent(r.targetId)}${r.kind ? `?kind=${r.kind}` : ""}`
        : r.name === "source-skills"
          ? `#/source-skills/${encodeURIComponent(r.sourceId)}`
          : r.name === "skill"
            ? `#/skill/${encodeURIComponent(r.skillId)}`
            : r.name === "local-skill"
              ? `#/local-skill/${encodeURIComponent(r.path)}`
              : r.name === "project"
                ? `#/projects/${encodeURIComponent(r.projectId)}`
                : `#/settings/${r.section}`;
  paintIcons(m);
}

function readRoute(): Route {
  const h = (location.hash || "").replace(/^#\/?/, "");
  const segments = h.split("/");
  const name = segments[0];
  if (name === "agent-skills" && segments[1]) {
    // 可带 ?kind= 筛选（从 Agents 页的统计 chip 点进来）。target id 是
    // slugify 产物，不会含 "?"，直接在段内切分即可。
    const [rawId, query = ""] = decodeURIComponent(segments[1]).split("?");
    const kind = new URLSearchParams(query).get("kind");
    return {
      name: "agent-skills",
      targetId: rawId,
      kind: kind === "local" || kind === "managed" || kind === "external_symlink" ? kind : undefined,
    };
  }
  if (name === "source-skills" && segments[1]) {
    return { name: "source-skills", sourceId: decodeURIComponent(segments[1]) };
  }
  if (name === "skill" && segments[1]) {
    return { name: "skill", skillId: decodeURIComponent(segments.slice(1).join("/")) };
  }
  if (name === "local-skill" && segments[1]) {
    // 磁盘路径整体 encodeURIComponent 后放进单个段。
    return { name: "local-skill", path: decodeURIComponent(segments.slice(1).join("/")) };
  }
  if (name === "projects") {
    // #/projects = 列表；#/projects/<id> = 项目详情。
    if (segments[1]) return { name: "project", projectId: decodeURIComponent(segments[1]) };
    return "projects";
  }
  if (name === "settings") {
    // 旧哈希 #/settings/agents 也落到通用（Agent 管理已合并到 Agents 页）。
    return { name: "settings", section: "general" };
  }
  // 「来源」页已合并进技能库的「按来源」tab；旧 #/sources 哈希落回技能库。
  if (["library", "discover", "market", "targets", "pending"].includes(name)) return name as Route;
  if (name === "sources") return "library";
  return "library";
}

function refreshNavCounts() {
  const s = store.get();
  setText("nav-count-library", s.skills.length);
  setText("nav-count-targets", s.targets.filter((t) => t.enabled).length);
  // 待办数只统计"问题"类事项（失效/冲突链接 + 有更新来源 + 未检测 agent），
  // 未安装的 skill 太常见，放进去数字会永远很大。
  const broken = s.installations.filter((i) => i.status === "missing" || i.status === "conflict").length;
  const outdated = s.update_report?.updates?.length ?? 0;
  const undetected = s.targets.filter((t) => t.enabled && !t.detected).length;
  setText("nav-count-pending", broken + outdated + undetected);
}

function setText(id: string, n: number) {
  const el = document.getElementById(id);
  if (el) el.textContent = String(n);
}

async function boot() {
  try {
    await store.load();
  } catch (e) {
    console.error(e);
    toast(`加载状态失败：${e}`, "error");
    return;
  }
  // 恢复持久化的主题色 —— 必须在首次页面渲染之前。
  await initAppearance();
  // 预载「添加来源默认勾选 Agents」开关（异步即可，设置页/添加弹窗读缓存）。
  void loadAddDefaultInstallTargets();
  // 预载「中央仓库路径」（发现页收编弹窗/设置页读缓存）。
  void loadVaultPath();

  // 侧栏底部版本号用真实应用版本(单一来源 package.json),避免两处硬编码。
  void getAppVersion().then((v) => {
    const el = document.querySelector<HTMLElement>(".rail__btn-version");
    if (el) el.textContent = `v${v}`;
  });
  // 应用自更新:启动延迟首查 + 每 10 分钟静默复查;更新源未配置时自动跳过。
  initAutoUpdateCheck();

  for (const btn of railButtons()) {
    btn.addEventListener("click", () => {
      const route = btn.dataset.route;
      if (route === "settings") setRoute({ name: "settings", section: "general" });
      else setRoute(route as Route);
    });
  }
  // Global "+" button — opens the add-source picker from any page.
  document.getElementById("rail-add-btn")?.addEventListener("click", () => openAddPicker());
  window.addEventListener("hashchange", () => setRoute(readRoute()));

  await listen<unknown>("skill-dock://updates", async () => {
    await store.refresh();
    toast("有可用更新", "info");
  });

  // Background scan completed for a single source. If this was a source the
  // user just added (tracked via markPendingPostAdd), pop the post-add config
  // modal (agents + flat/grouped) now that we know the skill count.
  await listen<string>("skill-dock://source-scanned", async (e) => {
    const sourceId = e.payload;
    const prompted = await maybePromptPostAdd(sourceId);
    if (!prompted) {
      await store.refresh();
    }
  });

  // Background GitHub clone failed — refresh so the row flips to the
  // failed state with the error message + retry button. Also drop any
  // pending post-add marker so it doesn't leak.
  await listen<{ source_id: string; error: string }>(
    "skill-dock://source-clone-failed",
    async (e) => {
      cancelPendingPostAdd(e.payload.source_id);
      await store.refresh();
    },
  );

  // Auto-sync completed (either from the initial add-local scan, or from the
  // filesystem watcher detecting a change). Refresh the store and toast the
  // result so the user knows skills were linked without a manual click.
  await listen<{ source_id: string; installed: number; failed: string[] }>(
    "skill-dock://auto-synced",
    async (e) => {
      await store.refresh();
      const { installed, failed } = e.payload;
      if (installed > 0 || failed.length > 0) {
        toast(
          failed.length
            ? `自动同步：${installed} 成功，${failed.length} 失败`
            : `已自动同步 ${installed} 个 skill`,
          failed.length ? "error" : "success",
        );
      }
    },
  );

  // Background rescan-all completed
  await listen<unknown>("skill-dock://rescan-all-done", async () => {
    await store.refresh();
    toast("扫描完成", "info");
  });

  // Background scan-all completed
  await listen<unknown>("skill-dock://scan-all-done", async () => {
    await store.refresh();
  });

  const statePath = document.getElementById("state-path")!;
  const snap = store.get();
  statePath.textContent = tildePath(snap.state_path);
  statePath.title = snap.state_path;

  store.subscribe(refreshNavCounts);
  store.subscribe(() => {
    const sp = document.getElementById("state-path")!;
    sp.textContent = tildePath(store.get().state_path);
  });
  refreshNavCounts();

  paintIcons(); // icons in rail + banner
  setRoute(readRoute());
}

// 未捕获的前端异常直接以 toast 浮出，避免"页面空白但毫无线索"。
window.addEventListener("error", (e) => {
  console.error(e.error ?? e.message);
  toast(`前端错误：${e.message}`, "error");
});
window.addEventListener("unhandledrejection", (e) => {
  console.error(e.reason);
  toast(`前端错误：${e.reason}`, "error");
});

boot();

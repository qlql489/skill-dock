// Library view — main browse/search/filter surface for all skills.
// 「按来源」tab 同时是来源管理入口（原「来源」页已合并进来）：来源行 =
// 原来源页管理行（重扫/克隆重试/访达/删除/更新预览），点击展开组内技能。

import { api } from "../api";
import { openAddPicker } from "../add-picker";
import {
  store,
  sourceById,
  isInstalled,
  isHealthy,
  tildePath,
  formatBytes,
  formatRelative,
} from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { openSkillDetail } from "./skill";
import { agentIconMarkup } from "../agent-icons";
import { renderComboStrip } from "./combos";
import { openModal } from "../modal";
import { ask } from "@tauri-apps/plugin-dialog";
import { marked } from "marked";
import type { SourceFileChange, SourceUpdatePreview } from "../types";

type SortKey = "custom" | "installed" | "name" | "modified" | "source" | "count" | "installtime";

/** 按来源 tab 的来源类型筛选（全部/GitHub/本地）。localStorage 持久化。 */
type SourceKindFilter = "all" | "github" | "local";
const KIND_KEY = "skill-dock:lib-source-kind";
function loadKindFilter(): SourceKindFilter {
  try {
    const v = localStorage.getItem(KIND_KEY);
    return v === "github" || v === "local" ? v : "all";
  } catch {
    return "all";
  }
}

/** 当前过滤的组合 id（localStorage 持久化）。null = 不过滤。 */
const COMBO_KEY = "skill-dock:combo-filter";
function loadComboFilter(): string | null {
  try {
    return localStorage.getItem(COMBO_KEY) || null;
  } catch {
    return null;
  }
}
function saveComboFilter(id: string | null) {
  try {
    if (id) localStorage.setItem(COMBO_KEY, id);
    else localStorage.removeItem(COMBO_KEY);
  } catch { /* ignore */ }
}

/** Display preference: flat (independent cards) or by-source (grouped under
 *  each source, collapsible). This is a global UI toggle (localStorage), fully
 *  decoupled from how the source's skills are installed. */
type DisplayMode = "flat" | "by-source";
const DISPLAY_KEY = "skill-dock:display-mode";

function loadDisplayMode(): DisplayMode {
  try {
    const v = localStorage.getItem(DISPLAY_KEY);
    return v === "by-source" ? "by-source" : "flat";
  } catch {
    return "flat";
  }
}

/** 每个 tab 独立的搜索词 + 排序状态（仅内存）。dir: 1=正序、-1=逆序。
 *  平铺默认「已安装在前」——最常关心的（装了的）排最前。 */
const tabState: Record<DisplayMode, { query: string; sort: string; dir: 1 | -1 }> = {
  flat: { query: "", sort: "installed", dir: 1 },
  "by-source": { query: "", sort: "source", dir: 1 },
};

/** 各 tab 的排序项（电商式排序条）。dir 是该排序项首次选中时的默认方向。
 *  tip 给语义不直观的项加悬浮说明。 */
const SORT_DEFS: Record<DisplayMode, { key: string; label: string; dir: 1 | -1; tip?: string }[]> = {
  flat: [
    { key: "installed", label: "已安装", dir: 1 },
    { key: "custom", label: "自定义", dir: 1 },
    { key: "name", label: "名称", dir: 1 },
    { key: "modified", label: "修改", dir: -1 },
  ],
  "by-source": [
    {
      key: "installtime",
      label: "安装时间",
      dir: -1,
      tip: "按来源里最近一次把技能装到 Agent 的时间排；还没装过的来源排在最后",
    },
    { key: "source", label: "名称", dir: 1 },
    { key: "count", label: "数量", dir: -1 },
  ],
};

/** 自定义排序（平铺 tab）：skill id 顺序表，localStorage 持久化。
 *  表里没有的 skill（新扫进来的）排在表尾、按名称稳定排序。 */
const CUSTOM_ORDER_KEY = "skill-dock:custom-skill-order";
function loadCustomOrder(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(CUSTOM_ORDER_KEY) || "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}
function saveCustomOrder(ids: string[]) {
  try { localStorage.setItem(CUSTOM_ORDER_KEY, JSON.stringify(ids)); } catch { /* ignore */ }
}

/** 列表形态：单行列表 / 多列卡片（localStorage 持久化，两个 tab 共用）。 */
type ViewMode = "rows" | "grid";
const VIEW_KEY = "skill-dock:lib-view";
function loadViewMode(): ViewMode {
  try {
    return localStorage.getItem(VIEW_KEY) === "grid" ? "grid" : "rows";
  } catch {
    return "rows";
  }
}

/** 启停开关的目标记忆：停用前 skill 装在哪些目标上（skillId → targetId[]），
 *  重新启用时只装回这些而不是全部。localStorage 持久化。 */
const TOGGLE_MEM_KEY = "skill-dock:toggle-memory";
function loadToggleMemory(): Record<string, string[]> {
  try {
    return JSON.parse(localStorage.getItem(TOGGLE_MEM_KEY) || "{}");
  } catch {
    return {};
  }
}
function saveToggleMemory(mem: Record<string, string[]>) {
  try { localStorage.setItem(TOGGLE_MEM_KEY, JSON.stringify(mem)); } catch { /* ignore */ }
}
function forgetToggleMemory(skillId: string) {
  const mem = loadToggleMemory();
  if (mem[skillId]) {
    delete mem[skillId];
    saveToggleMemory(mem);
  }
}

export function renderLibrary(mount: HTMLElement): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  mount.append(view);

  // --- Persistent shell (built once) ---
  // The tab row, toolbar (search box, filters), facet strips and the sort bar
  // are built once and never touched by re-renders. Only the list area
  // (`listMount`) is repainted on filter/sort/display changes. This keeps the
  // search input focused while typing — previously paint() did
  // view.innerHTML = "" which destroyed the input mid-keystroke.

  // 分类区：组合胶囊条。
  let comboFilter = loadComboFilter();

  // 显示模式 Tab（平铺/按来源）——整个库页最顶部的行。每个 tab 有独立的
  // 搜索词与排序状态（tabState），切换时搜索框和排序条同步到对应状态。
  let currentMode: DisplayMode = loadDisplayMode();
  let viewMode: ViewMode = loadViewMode();
  let kindFilter: SourceKindFilter = loadKindFilter();
  let customOrder = loadCustomOrder();
  // 按来源区块的展开状态（页面实例内保留，store 重绘不收起）。
  const expandedSources = new Set<string>();

  const tabBar = document.createElement("div");
  tabBar.className = "lib-display-tabs";
  tabBar.innerHTML = `
    <div class="tabs" data-display-group>
      <button class="tab" data-display="flat">平铺</button>
      <button class="tab" data-display="by-source">按来源</button>
    </div>
    <div class="lib-display-tabs__actions" data-src-actions>
      <button class="btn btn--ghost btn--sm" data-action="check-updates"><span data-icon="download" data-size="14"></span>检查更新</button>
      <button class="btn btn--ghost btn--sm" data-action="rescan-all"><span data-icon="refresh" data-size="14"></span>重新扫描</button>
    </div>
  `;
  view.append(tabBar);

  const facetZone = document.createElement("div");
  facetZone.className = "lib-facets";
  facetZone.innerHTML = `
    <div class="lib-facets__group">
      <div class="lib-facets__label">组合</div>
      <div class="lib-facets__host" data-combos></div>
    </div>
    <div class="lib-facets__total">共 <b data-stat="total">0</b> 个</div>
  `;

  const comboHost = facetZone.querySelector<HTMLElement>("[data-combos]")!;

  function drawFacets() {
    const snap = store.get();

    // 组合可能已删除：失效的过滤自动复位。
    if (comboFilter && !(snap.groups ?? []).some((g) => g.id === comboFilter)) {
      comboFilter = null;
      saveComboFilter(null);
    }

    renderComboStrip(comboHost, {
      activeId: comboFilter,
      onSelect: (id) => {
        comboFilter = id;
        saveComboFilter(id);
        drawFacets();
        renderList();
      },
    });
  }

  const toolbar = document.createElement("div");
  toolbar.className = "lib-toolbar";
  toolbar.innerHTML = `
      <div class="lib-filters">
        <div class="search lib-search">
          <span class="search__icon" data-icon="search" data-size="14"></span>
          <input class="input" data-search placeholder="搜索名称、描述、路径…" />
        </div>
        <select class="select lib-filter-select" data-source-filter></select>
        <select class="select lib-filter-select" data-installed-filter>
          <option value="">全部</option>
          <option value="yes">已安装</option>
          <option value="no">未安装</option>
        </select>
        <div class="seg-pills" data-kind-pills role="group" aria-label="来源类型">
          <button class="seg-pill" data-kind="all">全部</button>
          <button class="seg-pill" data-kind="github">git</button>
          <button class="seg-pill" data-kind="local">本地</button>
        </div>
    </div>
  `;
  view.append(toolbar);

  // 组合行：搜索框下方一行，组合胶囊在左、技能总数在右。
  view.append(facetZone);

  // 排序条：电商式排序按钮（左）+ 单行/多列视图切换（右），紧贴列表上方。
  // 点击已选中排序项切换正/逆序，当前项带 ↑/↓ 箭头；两个 tab 的排序项
  // 集合不同（见 SORT_DEFS）。
  const sortBar = document.createElement("div");
  sortBar.className = "lib-sortbar";
  sortBar.innerHTML = `
    <div class="lib-sortbar__sorts" data-sort-host></div>
    <div class="lib-viewswitch" role="group" aria-label="视图切换">
      <button class="view-btn" data-view="rows" title="单行列表"><span data-icon="list" data-size="14"></span></button>
      <button class="view-btn" data-view="grid" title="多列卡片"><span data-icon="grid" data-size="14"></span></button>
    </div>
  `;
  view.append(sortBar);

  // The list mount — everything below the toolbar gets repainted here.
  const listMount = document.createElement("div");
  view.append(listMount);

  // Cache the source-filter <select> options so we can detect when the source
  // set changes (after add/remove/rescan) and rebuild them — without wiping the
  // select's current value (which would reset the filter).
  let cachedSourceIds = "";

  /** Rebuild the source-filter <option>s if the source set changed. Preserves
   *  the current selection when possible. */
  function syncSourceOptions(sources: any[]) {
    const sf = toolbar.querySelector<HTMLSelectElement>("[data-source-filter]")!;
    const sig = sources.map((s) => s.id).join(",");
    if (sig === cachedSourceIds) return;
    cachedSourceIds = sig;
    const prev = sf.value;
    sf.innerHTML = `<option value="">所有来源</option>` +
      sources.map((s) => `<option value="${s.id}">${escapeHtml(s.name)} (${s.skill_count})</option>`).join("");
    // Restore selection if still valid, else reset.
    sf.value = sources.some((s) => s.id === prev) ? prev : "";
    if (sf.value) sf.removeAttribute("data-empty"); else sf.setAttribute("data-empty", "");
  }

  /** Sync the active tab styling to the current displayMode. */
  function syncDisplayTabs() {
    for (const btn of view.querySelectorAll<HTMLButtonElement>("[data-display]")) {
      const active = btn.dataset.display === currentMode;
      if (active) btn.setAttribute("data-active", "true");
      else btn.removeAttribute("data-active");
    }
  }

  /** 按 tab 切换可见性：组合行（胶囊+总数）整行只在平铺显示；类型 pills 和
   *  来源管理操作只在按来源。 */
  function syncFacetRow() {
    facetZone.style.display = currentMode === "flat" ? "" : "none";
    const pills = toolbar.querySelector<HTMLElement>("[data-kind-pills]");
    if (pills) pills.style.display = currentMode === "by-source" ? "" : "none";
    const actions = tabBar.querySelector<HTMLElement>("[data-src-actions]");
    if (actions) actions.style.display = currentMode === "by-source" ? "" : "none";
  }

  /** 同步类型 pills 的高亮与计数（在 renderList 里随 store 数据刷新）。 */
  function syncKindPills() {
    const snap = store.get();
    const gh = snap.sources.filter((s) => s.kind === "github").length;
    const counts: Record<SourceKindFilter, number> = {
      all: snap.sources.length,
      github: gh,
      local: snap.sources.length - gh,
    };
    const labels = { all: "全部", github: "git", local: "本地" };
    for (const btn of toolbar.querySelectorAll<HTMLButtonElement>("[data-kind]")) {
      const k = btn.dataset.kind as SourceKindFilter;
      btn.textContent = `${labels[k]} ${counts[k]}`;
      if (k === kindFilter) btn.setAttribute("data-active", "true");
      else btn.removeAttribute("data-active");
    }
  }

  /** 按当前 tab 重画排序条：选中项高亮并带 ↑/↓ 箭头；点击已选中项翻转方向。
   *  「自定义」没有方向概念：不显示箭头、点击不翻转，选中时给出拖拽提示。 */
  function drawSortBar() {
    const st = tabState[currentMode];
    const host = sortBar.querySelector<HTMLElement>("[data-sort-host]")!;
    host.innerHTML = "";
    for (const def of SORT_DEFS[currentMode]) {
      const btn = document.createElement("button");
      btn.className = "sort-btn";
      const active = def.key === st.sort;
      if (active) btn.setAttribute("data-active", "true");
      if (def.tip) btn.title = def.tip;
      const arrow = active && def.key !== "custom" ? (st.dir === 1 ? "↑" : "↓") : "";
      btn.innerHTML =
        `${escapeHtml(def.label)}<span class="sort-btn__arrow">${arrow}</span>`;
      btn.addEventListener("click", () => {
        const cur = tabState[currentMode];
        if (cur.sort === def.key) {
          if (def.key !== "custom") cur.dir = cur.dir === 1 ? -1 : 1;
        } else {
          cur.sort = def.key;
          cur.dir = def.dir;
        }
        drawSortBar();
        renderList();
      });
      host.append(btn);
    }
    if (currentMode === "flat" && st.sort === "custom") {
      const hint = document.createElement("span");
      hint.className = "lib-sortbar__hint";
      hint.textContent = "拖拽卡片调整顺序";
      host.append(hint);
    }
  }

  /** 同步单行/多列视图切换按钮的高亮。 */
  function syncViewSwitch() {
    for (const btn of sortBar.querySelectorAll<HTMLButtonElement>("[data-view]")) {
      if (btn.dataset.view === viewMode) btn.setAttribute("data-active", "true");
      else btn.removeAttribute("data-active");
    }
  }

  // ------------------------------------------------------------------
  // 自定义排序拖拽（平铺 tab + 选中「自定义」时）：单行/多列都可拖。
  // ------------------------------------------------------------------
  function wireCardDrag(grid: HTMLElement): void {
    let dragId: string | null = null;
    grid.querySelectorAll<HTMLElement>(".skill-card[data-skid]").forEach((card) => {
      card.addEventListener("dragstart", (e) => {
        dragId = card.dataset.skid!;
        // WebKit 对空 dataTransfer 会取消拖拽，必须带数据。
        e.dataTransfer?.setData("text/plain", dragId);
        if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
        card.classList.add("is-dragging");
      });
      card.addEventListener("dragend", () => {
        dragId = null;
        card.classList.remove("is-dragging");
        grid.querySelectorAll(".drag-before, .drag-after").forEach((n) => n.classList.remove("drag-before", "drag-after"));
      });
      card.addEventListener("dragover", (e) => {
        if (!dragId || dragId === card.dataset.skid) return;
        e.preventDefault();
        const rect = card.getBoundingClientRect();
        // 单行视图上下分；多列卡片左右分（卡片横向流动）。
        const before = viewMode === "grid"
          ? e.clientX < rect.left + rect.width / 2
          : e.clientY < rect.top + rect.height / 2;
        card.classList.toggle("drag-before", before);
        card.classList.toggle("drag-after", !before);
      });
      card.addEventListener("dragleave", () => card.classList.remove("drag-before", "drag-after"));
      card.addEventListener("drop", (e) => {
        e.preventDefault();
        const before = card.classList.contains("drag-before");
        card.classList.remove("drag-before", "drag-after");
        if (!dragId || dragId === card.dataset.skid) return;
        const dragged = dragId;
        dragId = null;
        const visibleIds = Array.from(
          grid.querySelectorAll<HTMLElement>(".skill-card[data-skid]"),
        ).map((c) => c.dataset.skid!);
        applyCustomDrop(dragged, card.dataset.skid!, before, visibleIds);
        renderList();
      });
    });
  }

  /** 把拖拽结果写回自定义顺序表。只在当前可见的技能里重排，被筛选
   *  隐藏的技能保持原有相对位置；顺序持久化到 localStorage。 */
  function applyCustomDrop(draggedId: string, targetId: string, before: boolean, visibleIds: string[]): void {
    const snap = store.get();
    const liveIds = new Set(snap.skills.map((s) => s.id));
    const arranged = customOrder.filter((id) => liveIds.has(id));
    const tail = snap.skills
      .filter((s) => !arranged.includes(s.id))
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((s) => s.id);
    const full = [...arranged, ...tail]; // 全量有效顺序（含未排过的）

    const visSet = new Set(visibleIds);
    const newSeq = visibleIds.filter((id) => id !== draggedId);
    const at = newSeq.indexOf(targetId);
    if (at === -1) return;
    newSeq.splice(before ? at : at + 1, 0, draggedId);

    // 回写：全量顺序里“可见位”按新序列代入，隐藏位原样保留。
    const out: string[] = [];
    let i = 0;
    for (const id of full) out.push(visSet.has(id) ? newSeq[i++] : id);
    while (i < newSeq.length) out.push(newSeq[i++]);
    customOrder = out;
    saveCustomOrder(out);
  }

  /** Read current filter state from the persistent toolbar controls + tab state. */
  function readFilters() {
    return {
      query: (toolbar.querySelector<HTMLInputElement>("[data-search]")?.value || "").trim().toLowerCase(),
      sourceFilter: toolbar.querySelector<HTMLSelectElement>("[data-source-filter]")?.value || "",
      installedFilter: toolbar.querySelector<HTMLSelectElement>("[data-installed-filter]")?.value || "",
      sort: tabState[currentMode].sort as SortKey,
      dir: tabState[currentMode].dir,
      displayMode: currentMode,
    };
  }

  /** Repaint only the list area. Safe to call while the search box is focused —
   *  it never touches the toolbar. */
  function renderList() {
    const snap = store.get();
    let pool = snap.skills.slice();
    const sources = snap.sources;
    const targets = snap.targets;
    const installs = snap.installations.filter(isHealthy);
    const { query, sourceFilter, installedFilter, sort, dir, displayMode } = readFilters();

    // 来源名小写映射，供“按来源”tab 的搜索与分组排序使用。
    const sourceNameById = new Map(sources.map((s) => [s.id, s.name.toLowerCase()]));

    syncSourceOptions(sources);

    // 组合过滤先收窄候选池（组合逻辑只在平铺 tab 生效）。
    if (comboFilter && displayMode === "flat") {
      const g = (snap.groups ?? []).find((x) => x.id === comboFilter);
      const ids = new Set(g?.skill_ids ?? []);
      pool = pool.filter((s) => ids.has(s.id));
    }

    const filtered = pool.filter((s) => {
      if (query) {
        // 搜索逻辑按 tab 区分：平铺只搜技能字段；按来源还匹配来源名，
        // 命中来源名时整组保留，否则只留组内命中的技能。
        if (!(displayMode === "by-source" &&
              (sourceNameById.get(s.source_id) ?? "").includes(query))) {
          const hay = `${s.name} ${s.description} ${s.relative_path}`.toLowerCase();
          if (!hay.includes(query)) return false;
        }
      }
      if (sourceFilter && s.source_id !== sourceFilter) return false;
      if (installedFilter === "yes" && !installs.some((i) => i.skill_id === s.id)) return false;
      if (installedFilter === "no" && installs.some((i) => i.skill_id === s.id)) return false;
      return true;
    });

    if (displayMode === "flat") {
      if (sort === "custom") {
        // 自定义：按拖拽顺序表排；没排过的按名称接尾。方向键对它无意义。
        const pos = new Map(customOrder.map((id, i) => [id, i] as const));
        filtered.sort((a, b) =>
          (pos.get(a.id) ?? Number.MAX_SAFE_INTEGER) - (pos.get(b.id) ?? Number.MAX_SAFE_INTEGER)
          || a.name.localeCompare(b.name));
      } else {
        // 平铺：对技能列表排序；dir 控制正/逆序，名称做稳定副键。
        const installedSet = new Set(installs.map((i) => i.skill_id));
        filtered.sort((a, b) => {
          switch (sort) {
            case "installed": return dir * (Number(installedSet.has(b.id)) - Number(installedSet.has(a.id))) || a.name.localeCompare(b.name);
            case "name":     return dir * a.name.localeCompare(b.name);
            case "modified": return dir * (+new Date(a.modified_at) - +new Date(b.modified_at)) || a.name.localeCompare(b.name);
            default:         return 0;
          }
        });
      }
    }

    listMount.innerHTML = "";

    // Empty states：没有任何来源 → 引导添加（两个 tab 一样）；按来源 tab
    // 即使 0 skill 也要渲染来源区块（管理入口，含克隆失败/空来源）。
    if (snap.sources.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.innerHTML = `
        <div class="empty__icon" data-icon="folder" data-size="22"></div>
        <h2 class="empty__title">还没有来源</h2>
        <p class="empty__sub">点左侧「+ 添加skills」，从本地文件夹、Git 仓库或压缩包导入，我们会递归扫描里面的 <code>SKILL.md</code>。</p>
        <div class="empty__actions">
          <button class="btn btn--primary" data-action="goto-sources"><span data-icon="add" data-size="14"></span>添加来源</button>
        </div>
      `;
      listMount.append(empty);
    } else if (displayMode === "by-source") {
      // ---- 按来源：来源管理行（原「来源」页）+ 可展开的组内技能 ----
      const queryRaw = query;
      const bySource = new Map<string, typeof filtered>();
      for (const s of filtered) {
        const arr = bySource.get(s.source_id) ?? [];
        arr.push(s);
        bySource.set(s.source_id, arr);
      }
      const shown = sources
        .filter((src) => kindFilter === "all" || (kindFilter === "github" ? src.kind === "github" : src.kind !== "github"))
        .filter((src) => {
          // 搜索命中来源名时整组保留（含 0 skill）；否则要有命中的技能。
          if (!queryRaw) return true;
          if (src.name.toLowerCase().includes(queryRaw)) return true;
          return (bySource.get(src.id) ?? []).length > 0;
        });
      // 排序条作用于“来源”：安装时间 / 名称 / 组内技能数；组内技能固定按名称正序。
      // 「安装时间」= 该来源下技能最近一次装到某个 Agent 的时刻（取所有目标里
      // 最新的那条 installed_at）；从没装过的来源在正逆序下都垫底，只按名称排。
      if (sort === "installtime") {
        const skillSource = new Map(snap.skills.map((s) => [s.id, s.source_id] as const));
        const lastInstall = new Map<string, number>();
        for (const i of installs) {
          const sid = skillSource.get(i.skill_id);
          if (!sid) continue;
          const t = +new Date(i.installed_at);
          const prev = lastInstall.get(sid);
          if (prev === undefined || t > prev) lastInstall.set(sid, t);
        }
        shown.sort((a, b) => {
          const tA = lastInstall.get(a.id);
          const tB = lastInstall.get(b.id);
          if (tA === undefined || tB === undefined) {
            if (tA !== undefined || tB !== undefined) return tA === undefined ? 1 : -1;
            return a.name.localeCompare(b.name);
          }
          return dir * (tA - tB) || a.name.localeCompare(b.name);
        });
      } else {
        shown.sort((a, b) => {
          const nA = bySource.get(a.id) ?? [];
          const nB = bySource.get(b.id) ?? [];
          const base = sort === "count" ? nA.length - nB.length : a.name.localeCompare(b.name);
          return dir * base || a.name.localeCompare(b.name);
        });
      }

      if (shown.length === 0) {
        const empty = document.createElement("div");
        empty.className = "empty";
        empty.innerHTML = `<div class="empty__icon" data-icon="search" data-size="22"></div>
          <h2 class="empty__title">没有匹配的来源</h2>
          <p class="empty__sub">换个搜索词，或切换上方的类型筛选。</p>`;
        listMount.append(empty);
      } else {
        const updateMap = new Map(
          (snap.update_report?.updates ?? []).map((u) => [u.source_id, u]),
        );
        for (const src of shown) {
          const groupSkills = (bySource.get(src.id) ?? []).slice()
            .sort((a, b) => a.name.localeCompare(b.name));
          listMount.append(
            renderSourceSection(src, groupSkills, targets, snap.installations, renderList, updateMap.get(src.id)),
          );
        }
      }
    } else if (filtered.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.innerHTML = `
        <div class="empty__icon" data-icon="search" data-size="22"></div>
        <h2 class="empty__title">没有匹配</h2>
        <p class="empty__sub">换个搜索词、清除筛选，或到「按来源」里重新扫描来源。</p>
      `;
      listMount.append(empty);
    } else {
      const grid = document.createElement("div");
      grid.className = viewMode === "grid" ? "lib-grid lib-grid--cols" : "lib-grid";
      const customDrag = sort === "custom";
      grid.classList.toggle("lib-grid--drag", customDrag);
      for (const skill of filtered) {
        const card = renderSkillCard(skill, targets, snap.installations);
        if (customDrag) {
          card.draggable = true;
          card.dataset.skid = skill.id;
          // 卡片左缘的竖向抓手，标识可拖拽排序。
          const grip = document.createElement("span");
          grip.className = "skill-card__grip";
          grip.title = "拖拽排序";
          grip.innerHTML = `<span class="icon" data-icon="gripVertical" data-size="14"></span>`;
          card.prepend(grip);
        }
        grid.append(card);
      }
      if (customDrag) wireCardDrag(grid);
      listMount.append(grid);
    }

    // 组合行右侧的总数
    const totalEl = view.querySelector("[data-stat='total']");
    if (totalEl) totalEl.textContent = String(snap.skills.length);

    syncKindPills();
    paintIcons(listMount);
  }

  // --- Wire toolbar interactions (once) ---
  // Delegated click for the empty-state button (lives inside listMount).
  view.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    if (t.closest("[data-action='goto-sources']")) {
      openAddPicker();
    }
  });

  const debouncedRender = debounce(renderList, 120);
  const searchInput = toolbar.querySelector<HTMLInputElement>("[data-search]")!;
  // 搜索词即时写入当前 tab 的状态（去抖只延迟重绘），切 tab 不会串词。
  searchInput.addEventListener("input", () => {
    tabState[currentMode].query = searchInput.value;
    debouncedRender();
  });
  toolbar.querySelector<HTMLSelectElement>("[data-source-filter]")!
    .addEventListener("change", renderList);
  toolbar.querySelector<HTMLSelectElement>("[data-installed-filter]")!
    .addEventListener("change", renderList);
  // 来源类型 pills（按来源 tab）：全部 / GitHub / 本地。
  for (const btn of toolbar.querySelectorAll<HTMLButtonElement>("[data-kind]")) {
    btn.addEventListener("click", () => {
      const k = btn.dataset.kind as SourceKindFilter;
      if (k === kindFilter) return;
      kindFilter = k;
      try { localStorage.setItem(KIND_KEY, k); } catch { /* ignore */ }
      renderList();
    });
  }
  // 按来源 tab 头部的来源级操作（原「来源」页头部功能）。
  tabBar.querySelector<HTMLButtonElement>("[data-action='rescan-all']")!
    .addEventListener("click", async () => {
      try {
        await api.rescanAll();
        await store.refresh();
        toast("正在重新扫描…", "info");
      } catch (e) { toast(String(e), "error"); }
    });
  tabBar.querySelector<HTMLButtonElement>("[data-action='check-updates']")!
    .addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      btn.disabled = true;
      try {
        await api.checkUpdates();
        await store.refresh();
        const count = store.get().update_report?.updates.length ?? 0;
        toast(count > 0 ? `发现 ${count} 个来源有更新` : "全部最新", count > 0 ? "info" : "success");
      } catch (err) { toast(String(err), "error"); }
      finally { btn.disabled = false; }
    });
  // 显示模式 Tab：切换后把搜索框与排序条同步到该 tab 自己的状态。
  for (const btn of view.querySelectorAll<HTMLButtonElement>("[data-display]")) {
    btn.addEventListener("click", () => {
      const mode = btn.dataset.display as DisplayMode;
      if (mode === currentMode) return;
      currentMode = mode;
      try { localStorage.setItem(DISPLAY_KEY, mode); } catch { /* ignore */ }
      syncDisplayTabs();
      syncSearchInput();
      syncFacetRow();
      drawSortBar();
      debouncedRender();
    });
  }

  // 单行/多列视图切换：只影响卡片排布形态，过滤排序不变。
  for (const btn of sortBar.querySelectorAll<HTMLButtonElement>("[data-view]")) {
    btn.addEventListener("click", () => {
      const v = btn.dataset.view as ViewMode;
      if (v === viewMode) return;
      viewMode = v;
      try { localStorage.setItem(VIEW_KEY, v); } catch { /* ignore */ }
      syncViewSwitch();
      renderList();
    });
  }

  /** 把搜索框内容与占位文案同步为当前 tab 的状态。 */
  function syncSearchInput() {
    searchInput.value = tabState[currentMode].query;
    searchInput.placeholder =
      currentMode === "by-source" ? "搜索来源或技能…" : "搜索名称、描述、路径…";
  }

  // --- Initial render ---
  syncDisplayTabs();
  syncSearchInput();
  syncFacetRow();
  drawSortBar();
  syncViewSwitch();
  // 分类条挂了也不能拖累列表绘制（曾经它一抛错整个库页就空白）。
  try { drawFacets(); } catch (e) { console.error("facet strip failed:", e); }
  renderList();
  paintIcons(view);

  /** 目标名 → 方形徽章里的字母缩写（最多两个字符）。 */
  function monogram(name: string): string {
    const words = name.trim().split(/[\s-_]+/).filter(Boolean);
    if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
    return name.slice(0, 2);
  }

  function renderSkillCard(skill: any, targets: any[], installs: any[]): HTMLElement {
    const card = document.createElement("div");
    const src = sourceById(skill.source_id);

    // 每个启用的目标一枚可点击徽章：实心=已链接（点击卸载），描边=未装
    // （点击安装）。设置里停用的 agent 不在这里出现；替换冲突/真实目录
    // 占位会在快速点击失败时以后端错误 toast 提示，完整分流走"+"选择器。
    const chipHtml = targets.filter((t) => t.enabled).map((t) => {
      const on = installs.some((i) => i.skill_id === skill.id && i.target_id === t.id && isHealthy(i));
      const cls = ["a-chip", on ? "is-on" : "is-off"].join(" ").trim();
      const tip = on ? `${t.name} · 已链接，点击卸载` : `${t.name} · 未安装，点击安装`;
      return `<button class="${cls}" data-chip="${escapeHtml(t.id)}" title="${escapeHtml(tip)}">${agentIconMarkup(t.id, t.name, "agent-icon--chip")}</button>`;
    }).join("");

    const nameHtml = `
      <h3 class="skill-card__name" title="${escapeHtml(skill.name)}">
        ${escapeHtml(skill.name)}
        ${skill.is_shared ? `<span class="tag tag--mono" style="font-size:10px;padding:1px 6px;margin-left:6px;vertical-align:middle" title="位于中央仓库，多个 agent 通过软链共用">共用</span>` : ""}
      </h3>
    `;
    const delHtml = `
      <button class="btn btn--sm btn--icon" data-action="delete" title="彻底删除（移除软链并删除目录）">
        <span data-icon="delete" data-size="14"></span>
      </button>
    `;
    // 来源：本地来源画文件夹，Git 克隆画分支；名称超长省略，悬停看全名。
    const srcIco = src?.kind === "github" ? "git" : "folder";
    const metaHtml = `
      <div class="skill-card__meta" title="${escapeHtml(`来源：${src?.name ?? "—"}`)}">
        <span data-icon="${srcIco}" data-size="12"></span>
        <span>${escapeHtml(src?.name ?? "—")}</span>
      </div>
    `;
    const desc = (skill.description ?? "").trim();
    const descHtml = desc
      ? `<div class="skill-card__desc${viewMode === "grid" ? " skill-card__desc--clamp" : ""}" title="${escapeHtml(desc)}">${escapeHtml(desc)}</div>`
      : `<div class="skill-card__desc skill-card__desc--empty">暂无描述</div>`;
    const toggleHtml = skillToggleHtml(skill, targets, installs);

    if (viewMode === "grid") {
      // 多列卡片：开关、名称一行，描述两行截断，底部来源在左、徽章在右。
      card.className = "skill-card skill-card--grid lift";
      card.innerHTML = `
        <div class="skill-card__head">
          ${toggleHtml}
          <div class="skill-card__title">${nameHtml}</div>
          <div class="skill-card__actions">${delHtml}</div>
        </div>
        ${descHtml}
        <div class="skill-card__foot">
          ${metaHtml}
          <div class="skill-card__agents" data-chips>${chipHtml}</div>
        </div>
      `;
    } else {
      // 单行列表：启停开关 | 名称 | 描述 | agent 徽章 | 来源 | 删除。
      card.className = "skill-card skill-card--row lift";
      card.innerHTML = `
        ${toggleHtml}
        <div class="skill-card__title">${nameHtml}</div>
        ${descHtml}
        <div class="skill-card__agents" data-chips>${chipHtml}</div>
        ${metaHtml}
        <div class="skill-card__actions">${delHtml}</div>
      `;
    }

    card.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const chip = t.closest<HTMLButtonElement>("[data-chip]");
      if (chip && !chip.disabled) {
        e.stopPropagation();
        await toggleChip(chip, skill, card);
        return;
      }
      const toggleBtn = t.closest<HTMLButtonElement>("[data-toggle]");
      if (toggleBtn && !toggleBtn.disabled) {
        e.stopPropagation();
        await toggleSkillAll(toggleBtn, skill, card);
        return;
      }
      if (t.closest("[data-action='delete']")) {
        e.stopPropagation();
        const snap = store.get();
        const linkCount = snap.installations.filter((i) => i.skill_id === skill.id && isHealthy(i)).length;
        const src = sourceById(skill.source_id);
        const gitNote = src?.kind === "github"
          ? "\n⚠ 该 skill 位于 Git 克隆内，之后「更新来源」可能把它重新拉回来。"
          : "";
        if (!(await ask(
          `彻底删除「${skill.name}」？\n` +
          `· 移除 ${linkCount} 个 agent 上的软链\n` +
          `· 删除目录：${tildePath(skill.absolute_path)}（移入废纸篓）` +
          gitNote +
          `\n应用内不保留副本，确定继续？`,
          { title: "彻底删除", kind: "warning", okLabel: "删除", cancelLabel: "取消" },
        ))) return;
        try {
          const removed = await api.deleteSkillForever(skill.id);
          forgetToggleMemory(skill.id);
          await store.refresh();
          toast(`已删除 ${skill.name}（移除 ${removed} 个软链）`, "success");
        } catch (err) {
          toast(`删除失败：${err}`, "error");
        }
        return;
      }
      openSkillDetail(skill.id);
    });
    return card;
  }

  /** 行尾启停开关：启用=装回到停用前记住的目标（没有记忆则装全部）；
   *  停用=从已装目标全部卸载并记住它们。开状态取"至少装在一个启用的
   *  目标上"；细粒度控制走各 agent 徽章。 */
  function skillToggleHtml(skill: any, targets: any[], installs: any[]): string {
    const enabled = targets.filter((t) => t.enabled);
    const onCount = enabled.filter((t) =>
      installs.some((i) => i.skill_id === skill.id && i.target_id === t.id && isHealthy(i)),
    ).length;
    const on = onCount > 0;
    let tip: string;
    if (on) {
      tip = `已启用（${onCount}/${enabled.length} 个目标），点击停用：从全部目标卸载（会记住这些目标）`;
    } else {
      const mem = loadToggleMemory();
      const remembered = (mem[skill.id] ?? []).filter((id) => enabled.some((t) => t.id === id));
      tip = remembered.length > 0
        ? `未启用，点击启用：装回之前的 ${remembered.length} 个目标`
        : `未启用，点击启用：安装到全部 ${enabled.length} 个目标`;
    }
    return `<button class="skill-toggle" data-toggle data-on="${on}" title="${escapeHtml(tip)}"><span class="skill-toggle__knob"></span></button>`;
  }

  /** 总开关的执行体：逐目标并发度为 1 地安装/卸载，失败收集后汇总 toast。 */
  async function toggleSkillAll(btn: HTMLButtonElement, skill: any, card: HTMLElement) {
    const snap = store.get();
    const enabled = snap.targets.filter((t: any) => t.enabled);
    if (enabled.length === 0) {
      toast("没有启用的目标：先到「设置」里启用至少一个 agent", "error");
      return;
    }
    const installed = enabled.filter((t: any) => isInstalled(skill.id, t.id));
    const turningOff = installed.length > 0;

    btn.classList.add("is-busy");
    card.querySelectorAll<HTMLButtonElement>("button").forEach((b) => (b.disabled = true));
    try {
      const fails: string[] = [];
      let ok = 0;
      if (turningOff) {
        // 停用前记住这次装在哪些目标上，重新启用时只装回这些。
        const mem = loadToggleMemory();
        mem[skill.id] = installed.map((t: any) => t.id);
        saveToggleMemory(mem);
        for (const t of installed) {
          try { await api.uninstallSkill(skill.id, t.id); ok++; }
          catch (e: any) { fails.push(`${t.name}：${e?.message ?? e}`); }
        }
        await store.refresh();
        if (fails.length === 0) toast(`已停用 ${skill.name}（从 ${ok} 个目标卸载，已记住）`, "success");
        else toast(`停用 ${skill.name}：${ok} 个成功，${fails.length} 个失败 —— ${fails[0]}`, "error");
      } else {
        // 有记忆就只装回记住的目标（仍启用的），没有则装全部。
        const mem = loadToggleMemory();
        const rememberedIds = mem[skill.id] ?? [];
        const remembered = enabled.filter((t: any) => rememberedIds.includes(t.id));
        const toInstall = remembered.length > 0 ? remembered : enabled;
        for (const t of toInstall) {
          try { await api.installSkill(skill.id, t.id, "fail"); ok++; }
          catch (e: any) { fails.push(`${t.name}：${e?.message ?? e}`); }
        }
        await store.refresh();
        forgetToggleMemory(skill.id);
        const scope = remembered.length > 0 ? `装回之前的 ${ok} 个目标` : `安装到 ${ok} 个目标`;
        if (fails.length === 0) toast(`已启用 ${skill.name}（${scope}）`, "success");
        else if (ok === 0) toast(`启用失败：${fails[0]}`, "error");
        else toast(`启用 ${skill.name}：${ok} 个成功，${fails.length} 个失败 —— ${fails[0]}`, "error");
      }
    } finally {
      card.querySelectorAll<HTMLButtonElement>("button").forEach((b) => (b.disabled = false));
      btn.classList.remove("is-busy");
    }
  }

  /** 单枚徽章的快速安装/卸载。执行中给该卡片的其余徽章加锁，避免并发。 */
  async function toggleChip(chip: HTMLButtonElement, skill: any, card: HTMLElement) {
    const tid = chip.dataset.chip!;
    const target = store.get().targets.find((x) => x.id === tid);
    if (!target || !target.enabled) return;
    const wasOn = isInstalled(skill.id, tid);

    card.querySelectorAll<HTMLButtonElement>(".a-chip").forEach((b) => (b.disabled = true));
    chip.classList.add("is-busy");
    try {
      if (wasOn) {
        await api.uninstallSkill(skill.id, tid);
      } else {
        // Fail 策略快速路径；被占用/受阻的错误原样弹给用户看。
        await api.installSkill(skill.id, tid, "fail");
      }
      await store.refresh();
    } catch (e: any) {
      const msg = String(e?.message ?? e);
      // 被其他 skill 的软链占用：确认后显式替换（真实目录占位无法覆盖）。
      if (!wasOn && msg.includes("已被占用")) {
        if (await ask(`${skill.name} → ${target.name}：${msg}\n\n用「${skill.name}」替换掉现有的软链？`, {
          title: "替换软链",
          kind: "warning",
          okLabel: "替换",
          cancelLabel: "取消",
        })) {
          try {
            await api.installSkill(skill.id, tid, "replace");
            await store.refresh();
            toast(`已替换并安装到 ${target.name}`, "success");
          } catch (e2: any) {
            toast(`替换失败：${e2?.message ?? e2}`, "error");
          }
        }
      } else {
        toast(`${wasOn ? "卸载" : "安装"}失败：${msg}`, "error");
      }
    } finally {
      card.querySelectorAll<HTMLButtonElement>(".a-chip").forEach((b) => (b.disabled = false));
      chip.classList.remove("is-busy");
    }
  }

  // 「按来源」区块 = 原来源页管理行 + 组内技能。收起时是来源行（重扫/
  // 克隆重试/访达/删除/更新预览），点击展开后顶部是整组操作，下面是
  // 组内技能卡片。展开状态记在 expandedSources，store 重绘不收起。
  function renderSourceSection(
    src: any,
    groupSkills: any[],
    targets: any[],
    installs: any[],
    repaint: () => void,
    update?: { commit_changed: boolean },
  ): HTMLElement {
    const section = document.createElement("div");
    section.className = "group-section";
    section.style.cssText = "border:1px solid var(--border);border-radius:12px;margin-bottom:var(--s-4);overflow:hidden;background:var(--bg)";

    const isGithub = src.kind === "github";
    const cloning = isGithub && (src.clone_status ?? "ready") === "cloning";
    const failed = isGithub && (src.clone_status ?? "ready") === "failed";
    const hasUpdate = !!update && !cloning && !failed;

    // 仅「全部」tab 显示类型胶囊：选了 git/本地 tab 时类型不言自明。
    const kindPill = kindFilter === "all"
      ? (isGithub
        ? `<span class="src-kind-pill src-kind-pill--git" title="Git 仓库来源">git<span class="icon" data-icon="git" data-size="10"></span></span>`
        : `<span class="src-kind-pill src-kind-pill--local" title="本地目录来源">本地</span>`)
      : "";

    const kindChips = cloning
      ? `<span class="src2-chip is-dim">克隆中…</span>`
      : failed
        ? `<span class="src2-chip is-bad" title="${escapeHtml(src.clone_error ?? "")}">克隆失败</span>`
        : isGithub
          ? `<span class="src2-chip">${escapeHtml(src.branch || "main")}</span>` +
            (src.last_commit_sha ? `<span class="src2-chip tag--mono">${src.last_commit_sha.slice(0, 7)}</span>` : "")
          : `<span class="src2-chip">本地目录</span>`;

    const scanTip = `上次扫描于 ${formatRelative(src.last_scanned_at)}${src.last_scanned_at ? ` · ${new Date(src.last_scanned_at).toLocaleString()}` : ""}`;
    const open = expandedSources.has(src.id);

    const header = document.createElement("div");
    header.className = "src2-row group-section__head";
    header.style.cursor = "pointer";
    header.innerHTML = `
      <div class="src2-idxcell"><span class="group-section__arrow" style="transition:transform .15s;display:inline-block;color:var(--ink-dim);font-size:10px;transform:rotate(${open ? 90 : 0}deg)">▶</span></div>
      <div class="src-row__main">
        <h3 class="src-row__name">${escapeHtml(src.name)}${kindPill}</h3>
        <div class="src-row__path" title="${escapeHtml(src.location)}">${escapeHtml(tildePath(src.location))}</div>
        ${failed && src.clone_error ? `<div class="src2-err">${escapeHtml(src.clone_error)}</div>` : ""}
      </div>
      <div class="src2-meta" title="${escapeHtml(scanTip)}">
        <div class="src2-meta__count"><b>${groupSkills.length}</b> skill</div>
        <div class="src2-meta__chips">${kindChips}</div>
      </div>
      <div class="src2-state">
        ${hasUpdate ? `<button class="src2-updpill" data-saction="preview-update" title="查看远端变更">有更新</button>` : ""}
      </div>
      <div class="src2-actions">
        ${hasUpdate ? `<button class="btn btn--primary btn--sm" data-saction="preview-update">查看更新</button>` : ""}
        ${failed
          ? `<button class="btn btn--ghost btn--icon" data-saction="retry" title="重试克隆"><span data-icon="refresh" data-size="14"></span></button>`
          : `<button class="btn btn--ghost btn--icon" data-saction="rescan" title="重新扫描"${cloning ? " disabled" : ""}><span data-icon="refresh" data-size="14"></span></button>`}
        <button class="btn btn--ghost btn--icon" data-saction="reveal" title="在访达中显示"${cloning ? " disabled" : ""}><span data-icon="external" data-size="14"></span></button>
        <button class="btn btn--danger btn--icon" data-saction="remove" title="删除来源"><span data-icon="delete" data-size="14"></span></button>
      </div>
    `;

    const body = document.createElement("div");
    body.className = "group-section__body";
    body.style.cssText = `display:${open ? "block" : "none"};padding:12px 16px;border-top:1px solid var(--border)`;

    // 展开区顶部：整组操作行（原按来源 tab 的组管理功能）。
    const instCount = groupSkills.filter((s) => installs.some((i) => i.skill_id === s.id)).length;
    const actionsRow = document.createElement("div");
    actionsRow.className = "group-section__tools";
    actionsRow.innerHTML = `
      <span class="tag" style="font-size:10px">整组管理</span>
      <span style="font-size:12px;color:var(--ink-mute)">已装 ${instCount} / ${targets.length} 目标</span>
      <span style="flex:1"></span>
      <button class="btn btn--sm btn--ghost" data-saction="install-all" title="安装整组到目标"><span data-icon="plus" data-size="14"></span>安装整组</button>
      <button class="btn btn--sm btn--ghost" data-saction="update-group" title="拉取并刷新整组"><span data-icon="refresh" data-size="14"></span>更新整组</button>
      <button class="btn btn--sm btn--danger" data-saction="delete-all" title="卸载并隐藏整组"><span data-icon="delete" data-size="14"></span>删除整组</button>
    `;
    body.append(actionsRow);

    if (groupSkills.length === 0) {
      const hint = document.createElement("div");
      hint.style.cssText = "font-size:12.5px;color:var(--ink-mute);padding:10px 0 4px";
      hint.textContent = cloning
        ? "正在克隆，扫描完成后这里会出现技能。"
        : failed
          ? "克隆失败 —— 点行上的 ↻ 重试。"
          : "这个来源目前没有技能（可能已被全部删除，或目录里没有 SKILL.md）。";
      body.append(hint);
    } else {
      const innerGrid = document.createElement("div");
      innerGrid.className = viewMode === "grid" ? "lib-grid lib-grid--cols" : "lib-grid";
      for (const skill of groupSkills) {
        innerGrid.append(renderSkillCard(skill, targets, installs));
      }
      body.append(innerGrid);
    }

    section.append(header, body);

    // 收起/展开（点行头，操作按钮除外）。
    header.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      if (t.closest("[data-saction]")) return;
      const nowOpen = !expandedSources.has(src.id);
      if (nowOpen) expandedSources.add(src.id);
      else expandedSources.delete(src.id);
      body.style.display = nowOpen ? "block" : "none";
      (header.querySelector(".group-section__arrow") as HTMLElement).style.transform = nowOpen ? "rotate(90deg)" : "none";
    });

    // ---- 来源管理操作（原「来源」页行内功能）----
    header.querySelectorAll<HTMLElement>("[data-saction]").forEach((el) => {
      el.addEventListener("click", async (e) => {
        e.stopPropagation();
        const action = el.dataset.saction;
        try {
          if (action === "preview-update") {
            openUpdateModal(src.id);
          } else if (action === "rescan") {
            await api.rescanSource(src.id);
            await store.refresh();
            toast(`已重新扫描 ${src.name}`, "success");
          } else if (action === "retry") {
            await api.retryCloneSource(src.id);
            await store.refresh();
            toast(`正在重新克隆 ${src.name}`, "info");
          } else if (action === "reveal") {
            api.revealInFinder(src.clone_path ?? src.location);
          } else if (action === "remove") {
            if (!(await ask(`删除来源「${src.name}」？已安装的软链会断开，但源文件夹本身保持不变。`, {
              title: "删除来源",
              kind: "warning",
              okLabel: "删除",
              cancelLabel: "取消",
            }))) return;
            await api.removeSource(src.id);
            await store.refresh();
            toast("来源已删除", "success");
            repaint();
          }
        } catch (err) {
          toast(`操作失败：${err}`, "error");
        }
      });
    });

    // ---- 整组操作（展开区）----
    actionsRow.querySelector<HTMLButtonElement>("[data-saction='install-all']")!
      .addEventListener("click", (e) => {
        e.stopPropagation();
        openGroupInstallPicker(src, groupSkills, targets);
      });
    actionsRow.querySelector<HTMLButtonElement>("[data-saction='update-group']")!
      .addEventListener("click", async (e) => {
        e.stopPropagation();
        try {
          await api.updateSourceGroup(src.id);
          toast(`正在更新整组「${src.name}」…`, "info");
        } catch (err) {
          toast(`更新失败：${err}`, "error");
        }
      });
    actionsRow.querySelector<HTMLButtonElement>("[data-saction='delete-all']")!
      .addEventListener("click", async (e) => {
        e.stopPropagation();
        if (!(await ask(`删除整组「${src.name}」？\n会卸载组内全部 ${groupSkills.length} 个 skill 并从库里隐藏（源文件保留）。`, {
          title: "删除整组",
          kind: "warning",
          okLabel: "删除",
          cancelLabel: "取消",
        }))) return;
        try {
          const n = await api.deleteSourceGroup(src.id);
          await store.refresh();
          toast(`已删除整组，移除 ${n} 个安装`, "success");
          repaint();
        } catch (err) {
          toast(`删除失败：${err}`, "error");
        }
      });

    return section;
  }

  // Multi-target picker for "install the whole group": reuse the modal
  // pattern from install-picker, but operate on a source id (not a skill).
  function openGroupInstallPicker(src: any, groupSkills: any[], targets: any[]) {
    const body = document.createElement("div");
    body.innerHTML = `
      <p class="field__hint" style="margin-bottom:12px">
        将 <span style="font-family:var(--font-display);color:var(--ink);font-size:18px">${escapeHtml(src.name)}</span>
        的全部 ${groupSkills.length} 个 skill 软链到勾选的目标。
      </p>
      <div class="col" data-list></div>
    `;
    const list = body.querySelector("[data-list]")!;
    for (const t of targets) {
      const item = document.createElement("label");
      item.style.cssText = `
        display:flex;align-items:center;gap:12px;
        padding:10px 12px;border:1px solid var(--border);
        background:var(--bg);cursor:${t.enabled ? "pointer" : "not-allowed"};
        opacity:${t.enabled ? 1 : 0.4};
      `;
      item.innerHTML = `
        <input type="checkbox" data-tid="${t.id}" ${t.enabled ? "" : "disabled"} style="accent-color:var(--accent)" />
        <div style="flex:1;min-width:0">
          <div style="font-family:var(--font-display);color:var(--ink);font-size:15px">${escapeHtml(t.name)}</div>
          <div style="font-family:var(--font-mono);font-size:12px;color:var(--ink-mute);letter-spacing:0.04em">${escapeHtml(tildePath(t.skills_dir))}</div>
        </div>
      `;
      list.append(item);
    }

    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;align-items:center;flex:1";
    footer.innerHTML = `
      <button class="btn btn--ghost" data-act="cancel">取消</button>
      <button class="btn btn--primary" data-act="apply">安装</button>
    `;

    const modal = openModal({ title: "安装整组到目标", body, footer });
    footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());
    footer.querySelector("[data-act='apply']")!.addEventListener("click", async () => {
      const checks = body.querySelectorAll<HTMLInputElement>("input[type='checkbox'][data-tid]:checked");
      const btn = footer.querySelector("[data-act='apply']") as HTMLButtonElement;
      btn.disabled = true;
      let totalInstalled = 0;
      const failures: string[] = [];
      for (const c of checks) {
        const tid = c.dataset.tid!;
        try {
          const r = await api.installSourceGroup(src.id, tid);
          totalInstalled += r.installed;
          failures.push(...r.failed);
        } catch (e) {
          failures.push(String(e));
        }
      }
      await store.refresh();
      modal.close();
      if (failures.length === 0) {
        toast(`已安装 ${totalInstalled} 个 skill`, "success");
      } else {
        toast(`安装完成：${totalInstalled} 个成功，${failures.length} 失败`, "error");
      }
    });
  }

  // Re-render when the store updates (e.g. after install/uninstall elsewhere).
  const unsub = store.subscribe(() => {
    try { drawFacets(); } catch (e) { console.error("facet strip failed:", e); }
    renderList();
  });
  mount.addEventListener("cleanup", () => unsub(), { once: true });
}

function debounce<T extends (...a: any[]) => void>(fn: T, ms: number): T {
  let t: any;
  return ((...args: any[]) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  }) as T;
}

// ---------------------------------------------------------------------------
// 更新预览弹窗（原「来源」页迁移）：远端变更文件清单 + 单文件 unified diff，
// 确认后才真正拉取——直连软链架构下拉取即刻生效，绝不静默 pull。
// ---------------------------------------------------------------------------

async function openUpdateModal(srcId: string): Promise<void> {
  const src = sourceById(srcId);
  if (!src) return;

  const body = document.createElement("div");
  body.innerHTML = `
    <div class="upd-head">
      <b>${escapeHtml(src.name)}</b>
      <span class="src2-chip" data-branch></span>
      <span class="upd-shas" data-shas></span>
    </div>
    <div data-content style="margin-top:12px">
      <div class="empty" style="padding:24px"><p class="empty__sub">正在获取远端变更…</p></div>
    </div>
  `;
  const content = body.querySelector("[data-content]")! as HTMLElement;

  const footer = document.createElement("div");
  footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
  footer.innerHTML = `
    <button class="btn btn--ghost" data-act="cancel">取消</button>
    <button class="btn btn--primary" data-act="apply" disabled>拉取并更新</button>
  `;
  const modal = openModal({ title: "更新预览", body, footer, width: 640 });
  const applyBtn = footer.querySelector<HTMLButtonElement>("[data-act='apply']")!;
  footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());

  let preview: SourceUpdatePreview;
  try {
    preview = await api.sourceUpdatePreview(srcId);
  } catch (e) {
    content.innerHTML = `<div class="empty" style="padding:20px"><p class="empty__sub">获取失败：${escapeHtml(String(e))}</p></div>`;
    return;
  }

  body.querySelector("[data-branch]")!.textContent = preview.branch;
  body.querySelector("[data-shas]")!.innerHTML =
    `${shortSha(preview.current_sha)} <span data-icon="arrowRight" data-size="11" style="vertical-align:-1px;color:var(--ink-mute)"></span> <b>${shortSha(preview.remote_sha)}</b>`;

  if (preview.note) {
    content.innerHTML = `<div class="upd-note is-warn">${escapeHtml(preview.note)}</div>`;
    return;
  }
  if (!preview.has_update) {
    content.innerHTML = `
      <div class="empty" style="padding:28px">
        <div class="empty__icon" data-icon="check" data-size="20"></div>
        <h2 class="empty__title" style="font-size:15px">已是最新</h2>
        <p class="empty__sub">远端没有新的提交。</p>
      </div>`;
    paintIcons(content);
    return;
  }

  applyBtn.disabled = false;
  const affected = preview.affected_skills;
  const behind = preview.behind_count;
  const behindText = behind === null
    ? ""
    : ` · 落后 <b>${behind}${preview.behind_exact ? "" : "+"}</b> 个提交`;
  const commitsBlock = preview.recent_commits.length
    ? `<div class="upd-commits"><div class="upd-commits__title">远端新提交</div>${
        preview.recent_commits.map((c) => `<div class="upd-commits__row">${escapeHtml(c)}</div>`).join("")
      }</div>`
    : "";
  content.innerHTML = `
    <div class="upd-summary">
      ${preview.files.length} 个文件变更${behindText} · 影响 ${affected.length} 个已入库 skill
      ${preview.truncated ? ` · <span style="color:#946200">清单超 ${500} 项已截断</span>` : ""}
    </div>
    ${commitsBlock}
    ${affected.length ? `<div class="upd-affected" title="${escapeHtml(affected.join("、"))}">${affected.slice(0, 8).map((n) => `<span class="src2-chip">${escapeHtml(n)}</span>`).join("")}${affected.length > 8 ? `<span class="src2-chip is-dim">+${affected.length - 8}</span>` : ""}</div>` : ""}
    <div class="upd-files" data-files></div>
  `;
  const filesHost = content.querySelector("[data-files]")! as HTMLElement;
  for (const f of preview.files) filesHost.append(updateFileRow(srcId, f));

  applyBtn.addEventListener("click", async () => {
    applyBtn.disabled = true;
    applyBtn.textContent = "拉取中…";
    try {
      await api.applyUpdates(srcId);
      await api.checkUpdates();   // 刷新缓存报告，清掉"有更新"角标
      await store.refresh();
      toast(`「${src.name}」已更新到 ${shortSha(preview.remote_sha)}`, "success");
      modal.close();
    } catch (e) {
      toast(`更新失败：${e}`, "error");
      applyBtn.disabled = false;
      applyBtn.textContent = "拉取并更新";
    }
  });
}

function updateFileRow(sourceId: string, f: SourceFileChange): HTMLElement {
  const box = document.createElement("div");
  box.className = "upd-file";
  const displayPath = f.new_path ? `${f.path} → ${f.new_path}` : f.path;
  box.innerHTML = `
    <div class="upd-file__row">
      <span class="upd-badge upd-badge--${f.status}">${fileChangeLabel(f.status)}</span>
      <span class="upd-file__path" title="${escapeHtml(displayPath)}">${escapeHtml(displayPath)}</span>
      <span class="upd-file__toggle" data-icon="arrowRight" data-size="12"></span>
    </div>
    <div class="upd-file__diff" hidden></div>
  `;
  const toggleEl = box.querySelector(".upd-file__toggle")! as HTMLElement;
  box.querySelector(".upd-file__row")!.addEventListener("click", async () => {
    const diffZone = box.querySelector(".upd-file__diff")! as HTMLElement;
    const opening = diffZone.hidden;
    diffZone.hidden = !opening;
    toggleEl.style.transform = opening ? "rotate(90deg)" : "";
    if (!opening || diffZone.dataset.loaded === "1") return;
    diffZone.innerHTML = `<div class="upd-diff-loading">载入差异…</div>`;
    try {
      const patch = await api.sourceFilePatch(sourceId, f.path);
      diffZone.innerHTML = colorizePatch(patch);
      diffZone.dataset.loaded = "1";
    } catch (e) {
      diffZone.innerHTML = `<div class="upd-diff-loading">载入失败：${escapeHtml(String(e))}</div>`;
    }
  });
  return box;
}

function fileChangeLabel(s: string): string {
  switch (s) {
    case "added": return "新增";
    case "deleted": return "删除";
    case "renamed": return "改名";
    default: return "修改";
  }
}

/** 给 unified patch 上色：+/−/@@/文件头 各自配色。 */
function colorizePatch(patch: string): string {
  const lines = patch.split("\n");
  const html = lines.map((ln) => {
    const esc = escapeHtml(ln);
    if (ln.startsWith("@@")) return `<div class="dl-hunk">${esc}</div>`;
    if (ln.startsWith("+")) return `<div class="dl-add">${esc}</div>`;
    if (ln.startsWith("-")) return `<div class="dl-del">${esc}</div>`;
    if (ln.startsWith("diff ") || ln.startsWith("index ")) return `<div class="dl-head">${esc}</div>`;
    return `<div class="dl-ctx">${esc || "&nbsp;"}</div>`;
  }).join("");
  return `<pre class="upd-diff">${html}</pre>`;
}

function shortSha(sha: string | null): string {
  return sha ? sha.slice(0, 7) : "—";
}

export function escapeHtml(s: string | undefined | null): string {
  if (s == null) return "";
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// marked import is shared with skill view
export { marked };

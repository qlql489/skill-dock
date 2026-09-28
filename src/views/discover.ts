// 本机skill管理 —— 以 Skill 为中心的全局扫描页。
//
// 双 tab：
// - 全局视角：扫描所有启用 Agent 的 skills 目录，按"真实文件"聚合；
// - agents 视角：可多选任意 Agent（含未启用的），看所选范围内聚合出的
//   skill 并同样可纳入管理。
//
// 两个 tab 各有独立搜索框。纳入管理：选择复制目的地（默认中央仓库 / 任一
// 本地来源），复制后把现有软链原地改指向新副本，完成迁移。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { agentIconMarkup, paintAgentIcons } from "../agent-icons";
import { toast } from "../toast";
import { escapeHtml } from "./library";
import { openSkillDetail, openLocalSkillDetail } from "./skill";
import { openModal } from "../modal";
import { getVaultPath } from "../prefs";
import type { DiscoveredSkillGroup, DiscoveredLink } from "../types";

type DiscoverTab = "global" | "agents";
const TAB_KEY = "skill-dock:discover-tab";

function loadTab(): DiscoverTab {
  try {
    return localStorage.getItem(TAB_KEY) === "agents" ? "agents" : "global";
  } catch {
    return "global";
  }
}

export function renderDiscover(mount: HTMLElement): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  mount.append(view);

  // --- State ---
  let tab: DiscoverTab = loadTab();
  let allGroups: DiscoveredSkillGroup[] = []; // 全量扫描（含未启用 Agent）
  let qGlobal = "";
  let qAgents = "";
  let selectedAgent: string | null = null; // agents 视角当前选中的 agent

  // --- 顶部：tab 切换 + 重新扫描 ---
  const topbar = document.createElement("div");
  topbar.className = "dtop-bar";
  topbar.innerHTML = `
    <div class="tabs" role="group" aria-label="视角切换">
      <button class="tab" data-tab="global">全局视角</button>
      <button class="tab" data-tab="agents">agents视角</button>
    </div>
    <button class="btn btn--ghost btn--sm" data-act="refresh"><span data-icon="refresh" data-size="13"></span>重新扫描</button>
  `;
  for (const b of topbar.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
    b.addEventListener("click", () => {
      tab = b.dataset.tab as DiscoverTab;
      try { localStorage.setItem(TAB_KEY, tab); } catch { /* ignore */ }
      paintTabs();
      paintActive();
    });
  }
  topbar.querySelector("[data-act='refresh']")!.addEventListener("click", load);
  view.append(topbar);

  function paintTabs() {
    for (const b of topbar.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
      b.setAttribute("data-active", String(b.dataset.tab === tab));
    }
  }

  // --- 全局视角 pane ---
  const globalPane = document.createElement("div");
  const globalList = document.createElement("div");
  globalList.className = "discover-list";
  globalPane.innerHTML = `
    <p class="discover-hint">扫描所有启用 Agent 的 skills 目录，按真实文件聚合。软链指向同一个目录的会归为一组；不在中央技能库里的可以一键纳入管理：复制到中央仓库或本地来源，并自动把软链改指向新副本。</p>
  `;
  const globalSearch = buildSearchBox(qGlobal, (v) => { qGlobal = v; paintGlobal(); });
  globalPane.append(globalSearch, globalList);
  view.append(globalPane);

  // --- agents 视角 pane：左 agent 列表（单选，顺序同 agents 页），
  //     右侧该 agent skills 目录下的聚合组卡 ---
  const agentsPane = document.createElement("div");
  agentsPane.className = "dpick-layout";
  const agentSide = document.createElement("div");
  agentSide.className = "dpick-side";
  const agentsMain = document.createElement("div");
  agentsMain.className = "dpick-main";
  const agentsList = document.createElement("div");
  agentsList.className = "discover-list";
  const agentsSearch = buildSearchBox(qAgents, (v) => { qAgents = v; paintAgents(); });
  agentsMain.append(agentsSearch, agentsList);
  agentsPane.append(agentSide, agentsMain);
  view.append(agentsPane);

  function buildSearchBox(value: string, onInput: (v: string) => void): HTMLElement {
    const box = document.createElement("div");
    box.className = "search discover-search";
    box.innerHTML = `
      <span class="search__icon" data-icon="search" data-size="14"></span>
      <input class="input" placeholder="搜索名称、路径…" />
    `;
    const input = box.querySelector<HTMLInputElement>("input")!;
    input.value = value;
    input.addEventListener("input", () => onInput(input.value));
    return box;
  }

  // --- 数据加载 ---
  async function load() {
    globalList.innerHTML = agentsList.innerHTML = `<div class="empty"><p class="empty__sub">扫描中…</p></div>`;
    try {
      // 一次拉全量（含未启用 Agent），两个 tab 各自前端过滤。
      allGroups = await api.discoverLocalSkills(true);
    } catch (e) {
      globalList.innerHTML = agentsList.innerHTML = `<div class="empty"><p class="empty__sub">扫描失败：${escapeHtml(String(e))}</p></div>`;
      return;
    }
    paintActive();
  }

  function paintActive() {
    paintTabs();
    // 两个 pane 都常驻 DOM（保留各自搜索词/滚动位置），按 tab 切换可见性。
    globalPane.style.display = tab === "global" ? "" : "none";
    agentsPane.style.display = tab === "agents" ? "" : "none";
    if (tab === "global") paintGlobal();
    else paintAgents();
  }

  /** 当前 tab 下的 (组, 可见落点) 列表。agents 视角 = 只看选中的那个 agent。 */
  function visibleGroups(): { g: DiscoveredSkillGroup; links: DiscoveredLink[] }[] {
    const enabledIds = new Set(store.get().targets.filter((t) => t.enabled).map((t) => t.id));
    const scope = tab === "global"
      ? enabledIds
      : (selectedAgent ? new Set([selectedAgent]) : new Set<string>());
    const q = (tab === "global" ? qGlobal : qAgents).trim().toLowerCase();

    const out: { g: DiscoveredSkillGroup; links: DiscoveredLink[] }[] = [];
    for (const g of allGroups) {
      const links = g.links.filter((l) => scope.has(l.target_id));
      if (links.length === 0) continue;
      if (q && !g.name.toLowerCase().includes(q) && !g.real_path.toLowerCase().includes(q)) continue;
      out.push({ g, links });
    }
    // 已纳管在前，未纳管紧随（需要处理的更醒目）。
    return out.sort((a, b) => Number(!!a.g.source_id) - Number(!!b.g.source_id));
  }

  function paintList(
    host: HTMLElement,
    items: { g: DiscoveredSkillGroup; links: DiscoveredLink[] }[],
    emptyText: string,
  ) {
    host.innerHTML = "";
    if (items.length === 0) {
      host.innerHTML = `
        <div class="empty">
          <div class="empty__icon" data-icon="search" data-size="22"></div>
          <h2 class="empty__title">${escapeHtml(emptyText)}</h2>
        </div>`;
      paintIcons(host);
      return;
    }
    for (const it of items) host.append(renderGroup(it.g, it.links));
    paintIcons(host);
  }

  function paintGlobal() {
    const items = visibleGroups();
    paintList(globalList, items, qGlobal ? "没有匹配的 Skill" : "没有扫到 Skill");
  }

  function paintAgents() {
    renderAgentSide();
    const targets = store.get().targets;
    if (!selectedAgent) {
      agentsList.innerHTML = `<div class="empty"><p class="empty__sub">在左侧选择一个 Agent。</p></div>`;
      return;
    }
    const items = visibleGroups();
    paintList(agentsList, items, targets.length === 0
      ? "还没有 Agent"
      : (qAgents ? "没有匹配的 Skill" : "这个 Agent 目录下没有扫到 Skill"));
  }

  /** 每个 agent 目录下扫到的 skill 数（用于左侧列表计数）。 */
  function countFor(targetId: string): number {
    return allGroups.filter((g) => g.links.some((l) => l.target_id === targetId)).length;
  }

  // ------------------------------------------------------------------
  // agents 视角：左侧 agent 列表（单选；顺序同 agents 页，含未启用的，
  // 灰色虚线标记；每行显示该目录下扫到的 skill 数）
  // ------------------------------------------------------------------
  function renderAgentSide() {
    const targets = store.get().targets;
    if (selectedAgent === null || !targets.some((t) => t.id === selectedAgent)) {
      selectedAgent = targets[0]?.id ?? null;
    }
    agentSide.innerHTML = targets.map((t) => `
      <div class="dpick-row" data-id="${escapeHtml(t.id)}"
           data-active="${t.id === selectedAgent}" data-enabled="${t.enabled}"
           title="${escapeHtml(tildePath(t.skills_dir))}">
        ${agentIconMarkup(t.id, t.name, "agent-icon--row")}
        <div class="dpick-row__main">
          <div class="dpick-row__name"><span>${escapeHtml(t.name)}</span>${t.enabled ? "" : `<i class="dpick-row__off">未启用</i>`}</div>
          <div class="dpick-row__path">${escapeHtml(tildePath(t.skills_dir))}</div>
        </div>
        <span class="dpick-row__count">${countFor(t.id)}</span>
      </div>`).join("");
    paintAgentIcons(agentSide);
    for (const row of agentSide.querySelectorAll<HTMLElement>(".dpick-row")) {
      row.addEventListener("click", () => {
        selectedAgent = row.dataset.id!;
        paintAgents();
      });
    }
  }

  // ------------------------------------------------------------------
  // 组卡片：名称 + 真实路径 + 已纳管/未纳管 + 纳入管理 + 落点 chips
  // ------------------------------------------------------------------
  function renderGroup(g: DiscoveredSkillGroup, links: DiscoveredLink[]): HTMLElement {
    const card = document.createElement("div");
    card.className = "dgroup";
    const managed = !!g.source_id;

    // 真实目录落点 = 源文件就躺在这个 Agent 目录里，不属于"链接落点"，
    // 单独以主题色标注在名称右边；软链落点才出现在下方 chips 行。
    const sourceLinks = links.filter((l) => !l.is_symlink);
    const symlinkLinks = links.filter((l) => l.is_symlink);

    const linkChips = symlinkLinks
      .map(
        (l) => `
        <span class="dgroup__link" title="${escapeHtml(l.link_path)}">
          <span data-icon="link" data-size="11"></span>
          ${escapeHtml(l.target_name)}
        </span>`,
      )
      .join("");
    const srcTag = sourceLinks
      .map((l) => `<span class="dgroup__src" title="${escapeHtml(l.link_path)}">源目录：${escapeHtml(l.target_name)}</span>`)
      .join("");

    card.innerHTML = `
      <div class="dgroup__head">
        <div class="dgroup__title">
          <div class="dgroup__nameline">
            <h3>${escapeHtml(g.name)}</h3>
          </div>
          <div class="dgroup__pathline">${srcTag}<code class="dgroup__path" title="${escapeHtml(g.real_path)}">${escapeHtml(tildePath(g.real_path))}</code></div>
        </div>
        <div class="dgroup__side">
          ${managed
            ? `<span class="src2-chip is-managed">已纳管 · ${escapeHtml(g.source_name ?? "")}</span>`
            : `<span class="src2-chip is-external">未纳管</span>
               <button class="btn btn--primary btn--sm" data-adopt title="复制到中央技能库，并接管后续安装、更新与同步管理"><span data-icon="upload" data-size="13"></span>纳入管理</button>`}
        </div>
      </div>
      ${symlinkLinks.length ? `<div class="dgroup__links">${linkChips}</div>` : ""}
    `;

    // 整卡可点进详情：已纳管走技能库详情（含 Agents 安装区），未纳管走
    // 路径详情（没有 Agents 区）。纳入管理按钮单独拦截，不触发整卡跳转。
    card.classList.add("is-clickable");
    card.addEventListener("click", () => {
      if (g.skill_id) openSkillDetail(g.skill_id, "#/discover");
      else openLocalSkillDetail(g.real_path, "#/discover");
    });
    card.querySelector("[data-adopt]")?.addEventListener("click", (e) => {
      e.stopPropagation();
      openAdoptModal(g, links);
    });
    return card;
  }

  // ------------------------------------------------------------------
  // 纳入管理弹窗：选择复制目的地
  // ------------------------------------------------------------------
  function openAdoptModal(g: DiscoveredSkillGroup, links: DiscoveredLink[]): void {
    const snap = store.get();
    const localSources = snap.sources.filter((s) => s.kind === "local");

    const body = document.createElement("div");
    body.innerHTML = `
      <p class="field__hint" style="margin-bottom:12px">
        将 <b style="color:var(--ink)">${escapeHtml(g.name)}</b> 复制到目的地，
        并把它在所有落点上的软链改指向新副本（迁移过程不断链）。
      </p>
      <div class="col" style="gap:8px" data-dest-list></div>
    `;
    const destList = body.querySelector("[data-dest-list]")!;

    type Dest = { id: string | null; title: string; desc: string };
    const dests: Dest[] = [
      { id: null, title: "中央仓库（默认）", desc: `${tildePath(getVaultPath())} —— 未注册来源时会自动注册` },
      ...localSources.map((s) => ({
        id: s.id,
        title: s.name,
        desc: tildePath(s.location),
      })),
    ];
    let chosen: string | null = null; // null = 中央仓库
    const rows: HTMLElement[] = [];
    for (const d of dests) {
      const row = document.createElement("div");
      row.className = "ip-row dest-row";
      row.innerHTML = `
        <span class="dest-radio" data-checked="false"></span>
        <div class="ip-row__main">
          <div class="ip-row__name">${escapeHtml(d.title)}</div>
          <div class="ip-row__path">${escapeHtml(d.desc)}</div>
        </div>
      `;
      row.addEventListener("click", () => {
        chosen = d.id;
        rows.forEach((r) => r.setAttribute("data-checked", r === row ? "true" : "false"));
      });
      rows.push(row);
      destList.append(row);
    }
    // 默认选中中央仓库
    rows[0]?.click();

    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
    footer.innerHTML = `
      <button class="btn btn--ghost" data-act="cancel">取消</button>
      <button class="btn btn--primary" data-act="go" title="复制到所选技能库，并接管后续安装、更新与同步管理"><span data-icon="upload" data-size="13"></span>纳入管理</button>
    `;
    const modal = openModal({ title: `纳入管理「${g.name}」`, body, footer, width: 560 });
    footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());

    footer.querySelector("[data-act='go']")!.addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      btn.disabled = true;
      btn.textContent = "迁移中…";
      try {
        const r = await api.adoptLocalSkill(g.real_path, chosen);
        await store.refresh();
        modal.close();
        toast(
          `已纳入管理至 ${tildePath(r.new_path)}${r.repointed ? `，改指 ${r.repointed} 个软链` : ""}${r.moved_original ? `，迁移 ${r.moved_original} 个原目录` : ""}`,
          "success",
        );
        load();
      } catch (err) {
        toast(`纳入管理失败：${err}`, "error");
        btn.disabled = false;
        btn.innerHTML = `<span data-icon="upload" data-size="13"></span>纳入管理`;
        paintIcons(btn);
      }
    });
  }

  paintActive();
  load();
  const unsub = store.subscribe(() => { /* 数据经 load() 自刷新；订阅仅为保持与其它页一致的生存期 */ });
  mount.addEventListener("cleanup", () => unsub(), { once: true });
}

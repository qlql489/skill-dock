// Agent skills page — read-only inventory of one target's skills directory.
//
// 只列不读：本页不展示 skill 内容。顶部统计条（全部 / 本地 / 已安装 /
// 符号链接）既是汇总也是筛选器，点击即按类过滤；搜索框按名称过滤。
// 点击任意条目统一跳转到全库的 Skill 详情页（#/skill/:id）；只有不在
// 技能库中的 skill 无法跳转，此时提示去「发现」页收编。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { escapeHtml } from "./library";
import { openSkillDetail } from "./skill";
import type { TargetSkillEntry, SkillLocationKind } from "../types";
import { agentIconMarkup } from "../agent-icons";

const LOCATION_BADGES: Record<SkillLocationKind, { label: string; className: string; hint: string }> = {
  local: {
    label: "本地",
    className: "skill-item__badge--local",
    hint: "真实目录，未由 SkillDock 安装。",
  },
  managed: {
    label: "已安装",
    className: "skill-item__badge--managed",
    hint: "由 SkillDock 创建并记录的软链接。",
  },
  external_symlink: {
    label: "符号链接",
    className: "skill-item__badge--external-link",
    hint: "外部创建的软链接，未添加到本应用的安装来源。",
  },
};

/**
 * The frontend can hot-reload before Tauri's Rust side restarts. During that
 * short window the old `list_target_skills` DTO has no `kind` field; never
 * let that compatibility gap blank the whole Agent list.
 */
function locationKind(entry: TargetSkillEntry): SkillLocationKind {
  if (entry.kind && Object.hasOwn(LOCATION_BADGES, entry.kind)) {
    return entry.kind;
  }
  return entry.is_symlink ? "external_symlink" : "local";
}

export function renderAgentSkills(
  mount: HTMLElement,
  targetId: string,
  initialKind: SkillLocationKind | null = null,
): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  view.className = "agent-detail";
  mount.append(view);

  const snap = store.get();
  const target = snap.targets.find((t) => t.id === targetId);
  if (!target) {
    view.innerHTML = `<div class="empty"><h2>未找到该 agent</h2></div>`;
    return;
  }

  // --- State ---
  let allSkills: TargetSkillEntry[] = [];
  let filterKind: SkillLocationKind | "" = initialKind ?? "";
  let searchQuery = "";

  // 从详情页返回时还原进入时的筛选状态。
  const backHash = `#/agent-skills/${encodeURIComponent(targetId)}${initialKind ? `?kind=${initialKind}` : ""}`;

  // --- Header with back button ---
  const head = document.createElement("div");
  head.className = "agent-detail__head";
  head.innerHTML = `
    <button class="btn btn--ghost btn--sm" id="agent-back"><span data-icon="arrowLeft" data-size="14"></span>返回</button>
    <div class="agent-detail__title">${agentIconMarkup(target.id, target.name, "agent-icon--detail")}<span>${escapeHtml(target.name)} 的 skills</span></div>
    <div class="agent-detail__path" title="${escapeHtml(target.skills_dir)}">${escapeHtml(tildePath(target.skills_dir))}</div>
  `;
  view.append(head);
  head.querySelector("#agent-back")!.addEventListener("click", () => {
    location.hash = "#/targets";
  });

  // --- Body: stats bar + skill list ---
  const body = document.createElement("div");
  body.className = "agent-detail__panel";
  view.append(body);
  body.innerHTML = `<div class="agent-detail__loading">加载中…</div>`;

  api.listTargetSkills(targetId).then((entries) => {
    allSkills = entries.filter((s) => s.has_skill_md);
    renderPanel();
  }).catch((e) => {
    body.innerHTML = `<div class="agent-detail__empty">加载失败：${escapeHtml(String(e))}</div>`;
  });

  function kindCounts(): Record<SkillLocationKind, number> {
    const c: Record<SkillLocationKind, number> = { local: 0, managed: 0, external_symlink: 0 };
    for (const s of allSkills) c[locationKind(s)]++;
    return c;
  }

  function getFilteredSkills(): TargetSkillEntry[] {
    let list = allSkills;
    if (filterKind) list = list.filter((s) => locationKind(s) === filterKind);
    if (searchQuery) {
      const q = searchQuery.toLowerCase();
      list = list.filter((s) => s.name.toLowerCase().includes(q));
    }
    return list;
  }

  function renderPanel() {
    body.innerHTML = "";

    const c = kindCounts();
    const bar = document.createElement("div");
    bar.className = "agent-detail__toolbar";
    const pills: { kind: SkillLocationKind | ""; label: string; n: number }[] = [
      { kind: "", label: "全部", n: allSkills.length },
      { kind: "local", label: "本地", n: c.local },
      { kind: "managed", label: "已安装", n: c.managed },
      { kind: "external_symlink", label: "符号链接", n: c.external_symlink },
    ];
    bar.innerHTML = `
      <div class="kind-filter" role="group" aria-label="按类型筛选">
        ${pills.map((p) => `
          <button class="kind-filter__pill${p.kind ? ` kind-filter__pill--${p.kind}` : ""}"
                  data-kind="${p.kind}" data-active="${filterKind === p.kind}">
            ${p.kind ? `<span class="kind-filter__dot"></span>` : ""}${p.label}
            <span class="kind-filter__num">${p.n}</span>
          </button>`).join("")}
      </div>
      <div class="skill-search">
        <span class="skill-search__icon" data-icon="search" data-size="13"></span>
        <input class="skill-search__input" data-input="search" type="text" placeholder="搜索 skill…" value="${escapeHtml(searchQuery)}" />
      </div>
    `;
    for (const b of bar.querySelectorAll<HTMLButtonElement>(".kind-filter__pill")) {
      b.addEventListener("click", () => {
        filterKind = (b.dataset.kind || "") as SkillLocationKind | "";
        for (const other of bar.querySelectorAll<HTMLButtonElement>(".kind-filter__pill")) {
          if (other === b) other.setAttribute("data-active", "true");
          else other.removeAttribute("data-active");
        }
        renderRows();
      });
    }
    const input = bar.querySelector<HTMLInputElement>("[data-input='search']")!;
    input.addEventListener("input", () => {
      searchQuery = input.value;
      renderRows();
    });
    body.append(bar);

    const listHost = document.createElement("div");
    listHost.className = "agent-detail__list";
    body.append(listHost);
    renderRows();
    paintIcons(bar);
  }

  function renderRows() {
    const listHost = body.querySelector(".agent-detail__list");
    if (!listHost) return;
    listHost.innerHTML = "";

    const filtered = getFilteredSkills();
    if (filtered.length === 0) {
      const empty = document.createElement("div");
      empty.className = "agent-detail__empty";
      empty.textContent = searchQuery
        ? "没有匹配"
        : filterKind
          ? `没有${LOCATION_BADGES[filterKind].label}类型的 skill`
          : "没有 skill";
      listHost.append(empty);
      return;
    }

    for (const s of filtered) {
      const badge = LOCATION_BADGES[locationKind(s)];
      const realPath = s.symlink_target ?? s.path;
      const targetHint = s.symlink_target ? `${badge.hint}\n实际目标：${s.symlink_target}` : badge.hint;

      const item = document.createElement("div");
      item.className = "skill-item skill-item--page";
      item.innerHTML = `
        <span class="skill-item__icon" data-icon="file" data-size="14"></span>
        <span class="skill-item__name">${escapeHtml(s.name)}</span>
        <span class="skill-item__path" title="${escapeHtml(realPath)}">${escapeHtml(tildePath(realPath))}</span>
        <span class="skill-item__badge ${badge.className}" title="${escapeHtml(targetHint)}">${badge.label}</span>
      `;
      item.addEventListener("click", () => {
        // 统一跳转到 Skill 详情页；不在库中的 skill（本地散装或指向外部
        // 的软链）没有详情页，提示先纳入管理。
        api.resolveSkillByPath(realPath).then((m) => {
          if (m) openSkillDetail(m.id, backHash);
          else toast("该 skill 不在中央技能库中，可到「本机skill管理」页纳入管理后再查看", "info");
        }).catch(() => {
          toast("该 skill 不在中央技能库中，可到「本机skill管理」页纳入管理后再查看", "info");
        });
      });
      listHost.append(item);
    }
    paintIcons(listHost as HTMLElement);
  }

  paintIcons(view);
}

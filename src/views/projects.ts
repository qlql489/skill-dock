// 项目级 skill 管理 —— 以项目为中心的工作区页。
//
// 两级视图：
// - 项目列表：已注册项目的卡片（技能数 + 五态健康度 + 打开/定位/删除）；
// - 项目详情：该项目内各 agent 项目级 skills 目录的技能清单，每条带
//   五态同步徽章（一致/项目较新/中央较新/分叉/仅项目）、启停开关
//   （rename 到 <skills>-disabled）、纳入管理/从中央更新/删除操作。
//
// 「从中央库安装」把库内技能导出为项目内副本（默认，随 git 走）或软链
// （单一来源，不随 git）；「纳入管理」把项目技能复制进中央仓库，项目文件不动。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { escapeHtml } from "./library";
import { openModal } from "../modal";
import { ask } from "@tauri-apps/plugin-dialog";
import type { ProjectOverview, ProjectSkillInfo, ProjectSlotInfo } from "../types";

export type SyncStatus = "in_sync" | "project_newer" | "center_newer" | "diverged" | "project_only";

/** 五态的展示文案与配色（CSS 类在 styles.css 的 .sync-badge）。 */
export const SYNC_STATUS_META: Record<SyncStatus, { label: string; cls: string; desc: string }> = {
  in_sync: { label: "一致", cls: "is-insync", desc: "与中央库内容一致" },
  project_newer: { label: "项目较新", cls: "is-pnewer", desc: "项目里的版本比中央库新，可纳入中央库管理" },
  center_newer: { label: "中央较新", cls: "is-cnewer", desc: "中央库的版本比项目新，可拉取更新" },
  diverged: { label: "内容分叉", cls: "is-diverged", desc: "两侧都有改动且时间接近，请手动比对" },
  project_only: { label: "仅项目", cls: "is-ponly", desc: "中央库里没有这个技能" },
};

function statusMeta(s: string) {
  return SYNC_STATUS_META[s as SyncStatus] ?? SYNC_STATUS_META.project_only;
}

// ---------------------------------------------------------------------------
// 项目列表
// ---------------------------------------------------------------------------

export function renderProjects(mount: HTMLElement): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  view.innerHTML = `
    <div class="proj-topbar">
      <p class="proj-hint">把代码仓库注册为项目，管理项目里各 Agent 的项目级 skills 目录（如 <code>.claude/skills</code>）：与中央技能库比对同步状态、按项目启停、纳入管理、从库安装。</p>
      <div class="proj-topbar__acts">
        <button class="btn btn--ghost btn--sm" data-act="scan"><span data-icon="search" data-size="13"></span>扫描文件夹</button>
        <button class="btn btn--primary btn--sm" data-act="add"><span data-icon="add" data-size="13"></span>添加项目</button>
      </div>
    </div>
    <div class="proj-list" data-list></div>
  `;
  mount.append(view);
  const list = view.querySelector<HTMLElement>("[data-list]")!;

  view.querySelector("[data-act='add']")!.addEventListener("click", () => openAddModal());
  view.querySelector("[data-act='scan']")!.addEventListener("click", () => openScanModal());

  async function load() {
    list.innerHTML = `<div class="empty"><p class="empty__sub">扫描中…</p></div>`;
    let overviews: ProjectOverview[];
    try {
      overviews = await api.getProjects();
    } catch (e) {
      list.innerHTML = `<div class="empty"><p class="empty__sub">加载失败：${escapeHtml(String(e))}</p></div>`;
      return;
    }
    if (overviews.length === 0) {
      list.innerHTML = `
        <div class="empty">
          <div class="empty__icon" data-icon="folder" data-size="22"></div>
          <h2 class="empty__title">还没有注册项目</h2>
          <p class="empty__sub">添加一个代码仓库，管理它的项目级 skills；也可以先「扫描文件夹」批量发现。</p>
        </div>`;
      paintIcons(list);
      return;
    }
    list.innerHTML = "";
    for (const p of overviews) list.append(renderCard(p));
    paintIcons(list);
  }

  function renderCard(p: ProjectOverview): HTMLElement {
    const card = document.createElement("div");
    card.className = "proj-card";
    const chips: string[] = [`<span class="proj-card__count">${p.skill_count} 个技能</span>`];
    const healthOrder: [keyof typeof p.health, SyncStatus][] = [
      ["in_sync", "in_sync"],
      ["project_newer", "project_newer"],
      ["center_newer", "center_newer"],
      ["diverged", "diverged"],
      ["project_only", "project_only"],
    ];
    for (const [key, status] of healthOrder) {
      const n = p.health[key];
      if (n > 0) chips.push(`<span class="sync-dot ${statusMeta(status).cls}" title="${statusMeta(status).desc}">${statusMeta(status).label} ${n}</span>`);
    }
    card.innerHTML = `
      <div class="proj-card__main">
        <div class="proj-card__name">${escapeHtml(p.name)}</div>
        <code class="proj-card__path" title="${escapeHtml(p.path)}">${escapeHtml(tildePath(p.path))}</code>
        <div class="proj-card__chips">${chips.join("")}</div>
      </div>
      <div class="proj-card__acts">
        <button class="btn btn--ghost btn--sm" data-open><span data-icon="chevronRight" data-size="13"></span>打开</button>
        <button class="btn btn--ghost btn--icon" data-finder title="在 Finder 中显示"><span data-icon="external" data-size="14"></span></button>
        <button class="btn btn--ghost btn--icon" data-remove title="移除项目（只取消注册，不动文件）"><span data-icon="delete" data-size="14"></span></button>
      </div>
    `;
    card.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest("button")) return;
      location.hash = `#/projects/${encodeURIComponent(p.id)}`;
    });
    card.querySelector("[data-open]")!.addEventListener("click", () => {
      location.hash = `#/projects/${encodeURIComponent(p.id)}`;
    });
    card.querySelector("[data-finder]")!.addEventListener("click", () => {
      api.revealInFinder(p.path).catch((e) => toast(String(e), "error"));
    });
    card.querySelector("[data-remove]")!.addEventListener("click", async () => {
      const ok = await ask(`移除项目「${p.name}」？\n\n只取消注册，项目文件不会被改动。`, {
        title: "移除项目",
        kind: "warning",
        okLabel: "移除",
        cancelLabel: "取消",
      });
      if (!ok) return;
      try {
        await api.removeProject(p.id);
        await store.refresh();
        toast("已移除项目", "success");
        load();
      } catch (e) {
        toast(`移除失败：${e}`, "error");
      }
    });
    return card;
  }

  // ------------------------------------------------------------------
  // 添加项目弹窗：手输路径 / 文件夹选择
  // ------------------------------------------------------------------
  function openAddModal(): void {
    const body = document.createElement("div");
    body.innerHTML = `
      <div class="col" style="gap:8px">
        <label class="field__label">项目根目录</label>
        <div class="proj-add-row">
          <input class="input" data-input="path" placeholder="~/work/my-repo" />
          <button class="btn btn--ghost" data-act="pick">选择…</button>
        </div>
        <p class="field__hint">项目里各 Agent 的项目级 skills 目录（如 <code>.claude/skills</code>）会自动被发现。</p>
      </div>
    `;
    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
    footer.innerHTML = `
      <button class="btn btn--ghost" data-act="cancel">取消</button>
      <button class="btn btn--primary" data-act="go">添加</button>
    `;
    const modal = openModal({ title: "添加项目", body, footer, width: 560 });
    const input = body.querySelector<HTMLInputElement>("[data-input='path']")!;
    input.focus();
    body.querySelector("[data-act='pick']")!.addEventListener("click", async () => {
      const dir = await api.pickFolder();
      if (dir) input.value = dir;
    });
    footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());
    footer.querySelector("[data-act='go']")!.addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      if (!input.value.trim()) return;
      btn.disabled = true;
      btn.textContent = "添加中…";
      try {
        await api.addProject(input.value);
        await store.refresh();
        modal.close();
        toast("已添加项目", "success");
        load();
      } catch (err) {
        toast(`添加失败：${err}`, "error");
        btn.disabled = false;
        btn.textContent = "添加";
      }
    });
  }

  // ------------------------------------------------------------------
  // 扫描弹窗：输入根目录 → 深度 ≤4 找候选项目 → 勾选批量添加
  // ------------------------------------------------------------------
  function openScanModal(): void {
    const body = document.createElement("div");
    body.innerHTML = `
      <div class="col" style="gap:10px">
        <div class="proj-add-row">
          <input class="input" data-input="root" placeholder="要扫描的父目录，如 ~/work" />
          <button class="btn btn--ghost" data-act="pick">选择…</button>
          <button class="btn btn--primary" data-act="scan">扫描</button>
        </div>
        <div class="col" style="gap:6px" data-results>
          <p class="field__hint">扫描深度最多 4 层；命中任一项目级 skills 目录（含 <code>-disabled</code>）的目录会列为候选。</p>
        </div>
      </div>
    `;
    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
    footer.innerHTML = `
      <button class="btn btn--ghost" data-act="cancel">取消</button>
      <button class="btn btn--primary" data-act="add" disabled>添加所选</button>
    `;
    const modal = openModal({ title: "扫描文件夹找项目", body, footer, width: 620 });
    const rootInput = body.querySelector<HTMLInputElement>("[data-input='root']")!;
    const results = body.querySelector<HTMLElement>("[data-results]!")!;
    const addBtn = footer.querySelector<HTMLButtonElement>("[data-act='add']")!;
    const found = new Map<string, boolean>(); // path -> checked

    body.querySelector("[data-act='pick']")!.addEventListener("click", async () => {
      const dir = await api.pickFolder();
      if (dir) rootInput.value = dir;
    });
    footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());

    function paintResults() {
      if (found.size === 0) {
        results.innerHTML = `<p class="field__hint">没有找到候选项目。</p>`;
        addBtn.disabled = true;
        return;
      }
      results.innerHTML = [...found.keys()]
        .map(
          (p) => `
          <div class="ip-row proj-scan-row" data-path="${escapeHtml(p)}" data-checked="${found.get(p)}">
            <span class="proj-check" data-icon="check" data-size="13"></span>
            <div class="ip-row__main">
              <div class="ip-row__name">${escapeHtml(p.split("/").filter(Boolean).pop() ?? p)}</div>
              <div class="ip-row__path">${escapeHtml(tildePath(p))}</div>
            </div>
          </div>`,
        )
        .join("");
      for (const row of results.querySelectorAll<HTMLElement>(".proj-scan-row")) {
        row.addEventListener("click", () => {
          const p = row.dataset.path!;
          found.set(p, !found.get(p));
          row.setAttribute("data-checked", String(found.get(p)));
          addBtn.disabled = ![...found.values()].some(Boolean);
        });
      }
      addBtn.disabled = ![...found.values()].some(Boolean);
      paintIcons(results);
    }

    body.querySelector("[data-act='scan']")!.addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      if (!rootInput.value.trim()) return;
      btn.disabled = true;
      btn.textContent = "扫描中…";
      results.innerHTML = `<p class="field__hint">扫描中…</p>`;
      try {
        const paths = await api.scanProjectRoots(rootInput.value);
        const existing = new Set(store.get().projects.map((p) => p.path.replace(/\/+$/, "")));
        found.clear();
        for (const p of paths) found.set(p, !existing.has(p.replace(/\/+$/, "")));
        paintResults();
        // 已注册的候选标灰
        for (const row of results.querySelectorAll<HTMLElement>(".proj-scan-row")) {
          if (existing.has(row.dataset.path!.replace(/\/+$/, ""))) {
            row.setAttribute("data-existing", "true");
            row.setAttribute("data-checked", "false");
            found.set(row.dataset.path!, false);
          }
        }
        addBtn.disabled = ![...found.values()].some(Boolean);
      } catch (err) {
        results.innerHTML = `<p class="field__hint">扫描失败：${escapeHtml(String(err))}</p>`;
      } finally {
        btn.disabled = false;
        btn.textContent = "扫描";
      }
    });

    addBtn.addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      const targets = [...found.entries()].filter(([, checked]) => checked).map(([p]) => p);
      if (targets.length === 0) return;
      btn.disabled = true;
      btn.textContent = "添加中…";
      let ok = 0;
      const fails: string[] = [];
      for (const p of targets) {
        try {
          await api.addProject(p);
          ok++;
        } catch (err) {
          fails.push(String(err));
        }
      }
      await store.refresh();
      modal.close();
      toast(fails.length ? `添加了 ${ok} 个项目，${fails.length} 个失败：${fails[0]}` : `已添加 ${ok} 个项目`, fails.length ? "error" : "success");
      load();
    });
  }

  load();
}

// ---------------------------------------------------------------------------
// 项目详情
// ---------------------------------------------------------------------------

export function renderProjectDetail(mount: HTMLElement, projectId: string): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  view.innerHTML = `
    <div class="proj-topbar">
      <button class="btn btn--ghost btn--sm" data-act="back"><span data-icon="arrowLeft" data-size="13"></span>返回</button>
      <div class="proj-head">
        <div class="proj-head__name" data-el="name"></div>
        <code class="proj-card__path" data-el="path"></code>
      </div>
      <div class="proj-topbar__acts">
        <button class="btn btn--ghost btn--sm" data-act="refresh"><span data-icon="refresh" data-size="13"></span>重新扫描</button>
        <button class="btn btn--primary btn--sm" data-act="install"><span data-icon="download" data-size="13"></span>从中央库安装</button>
      </div>
    </div>
    <div class="proj-filterbar">
      <div class="search proj-search">
        <span class="search__icon" data-icon="search" data-size="14"></span>
        <input class="input" placeholder="搜索名称、路径…" />
      </div>
      <div class="proj-statuschips" role="group" aria-label="状态筛选"></div>
    </div>
    <div class="proj-skilllist" data-list></div>
  `;
  mount.append(view);
  const list = view.querySelector<HTMLElement>("[data-list]")!;
  const searchInput = view.querySelector<HTMLInputElement>(".proj-search input")!;
  const chipsHost = view.querySelector<HTMLElement>(".proj-statuschips")!;

  view.querySelector("[data-act='back']")!.addEventListener("click", () => {
    location.hash = "#/projects";
  });
  view.querySelector("[data-act='refresh']")!.addEventListener("click", load);
  view.querySelector("[data-act='install']")!.addEventListener("click", () => openInstallModal());
  searchInput.addEventListener("input", paint);

  // --- 状态 ---
  let allSkills: ProjectSkillInfo[] = [];
  let slots: ProjectSlotInfo[] = [];
  let q = "";
  let statusFilter: SyncStatus | "all" = "all";
  const chipDefs: { key: SyncStatus | "all"; label: string }[] = [
    { key: "all", label: "全部" },
    { key: "project_newer", label: "项目较新" },
    { key: "center_newer", label: "中央较新" },
    { key: "diverged", label: "分叉" },
    { key: "project_only", label: "仅项目" },
    { key: "in_sync", label: "一致" },
  ];

  function paintChips() {
    const counts = new Map<string, number>([["all", allSkills.length]]);
    for (const s of allSkills) counts.set(s.sync_status, (counts.get(s.sync_status) ?? 0) + 1);
    chipsHost.innerHTML = chipDefs
      .map(({ key, label }) => {
        const n = counts.get(key) ?? 0;
        const meta = key === "all" ? null : statusMeta(key);
        return `<button class="proj-chip ${meta ? statusMeta(key).cls : ""}" data-status="${key}" data-active="${statusFilter === key}" ${n === 0 && key !== "all" ? "data-empty" : ""}>${label} ${n}</button>`;
      })
      .join("");
    for (const btn of chipsHost.querySelectorAll<HTMLButtonElement>("[data-status]")) {
      btn.addEventListener("click", () => {
        statusFilter = btn.dataset.status as SyncStatus | "all";
        paintChips();
        paint();
      });
    }
  }

  async function load() {
    list.innerHTML = `<div class="empty"><p class="empty__sub">扫描中…</p></div>`;
    try {
      const project = store.get().projects.find((p) => p.id === projectId);
      if (project) {
        view.querySelector("[data-el='name']")!.textContent = project.name;
        const pathEl = view.querySelector<HTMLElement>("[data-el='path']")!;
        pathEl.textContent = tildePath(project.path);
        pathEl.title = project.path;
      }
      [allSkills, slots] = await Promise.all([api.getProjectSkills(projectId), api.getProjectSlots(projectId)]);
    } catch (e) {
      list.innerHTML = `<div class="empty"><p class="empty__sub">加载失败：${escapeHtml(String(e))}</p></div>`;
      return;
    }
    paintChips();
    paint();
  }

  function paint() {
    const kw = q.trim().toLowerCase();
    const items = allSkills.filter((s) => {
      if (statusFilter !== "all" && s.sync_status !== statusFilter) return false;
      if (kw && !s.name.toLowerCase().includes(kw) && !s.relative_path.toLowerCase().includes(kw) && !s.description.toLowerCase().includes(kw)) return false;
      return true;
    });
    list.innerHTML = "";
    if (items.length === 0) {
      list.innerHTML = `<div class="empty"><div class="empty__icon" data-icon="search" data-size="22"></div><h2 class="empty__title">${kw || statusFilter !== "all" ? "没有匹配的技能" : "这个项目里还没有扫到技能"}</h2><p class="empty__sub">${kw || statusFilter !== "all" ? "" : "可以从中央库安装，或把已有技能放进项目的 skills 目录后重新扫描。"}</p></div>`;
      paintIcons(list);
      return;
    }
    for (const s of items) list.append(renderRow(s));
    paintIcons(list);
  }

  function renderRow(s: ProjectSkillInfo): HTMLElement {
    const row = document.createElement("div");
    row.className = "proj-row";
    const meta = statusMeta(s.sync_status);
    row.innerHTML = `
      <button class="skill-toggle" data-toggle data-on="${s.enabled}" title="${s.enabled ? "已启用，点击禁用（移动到 -disabled 目录）" : "未启用，点击启用"}"><span class="skill-toggle__knob"></span></button>
      <div class="proj-row__main">
        <div class="proj-row__nameline">
          <span class="proj-row__name">${escapeHtml(s.name)}</span>
          <span class="proj-row__agent" title="${escapeHtml(s.agent_names)}">${escapeHtml(s.agent_dir)}</span>
          <span class="sync-dot ${meta.cls}" title="${meta.desc}">${meta.label}</span>
          ${s.is_symlink ? `<span class="proj-row__link" title="${escapeHtml(s.path)}"><span data-icon="link" data-size="11"></span>软链</span>` : ""}
        </div>
        <div class="proj-row__sub">${escapeHtml(s.description || s.relative_path)}</div>
      </div>
      <div class="proj-row__acts">
        ${canAdopt(s) ? `<button class="btn btn--ghost btn--sm" data-adopt title="复制到中央技能库，接管后续安装、更新与同步管理；项目文件不动"><span data-icon="upload" data-size="13"></span>纳入管理</button>` : ""}
        ${canPull(s) ? `<button class="btn btn--ghost btn--sm" data-pull title="${s.sync_status === "center_newer" ? "用中央库版本覆盖项目副本" : "两侧都有改动：以中央库版本覆盖（项目旧副本进废纸篓）"}"><span data-icon="refresh" data-size="13"></span>从中央更新</button>` : ""}
        <button class="btn btn--ghost btn--icon" data-view title="查看 SKILL.md"><span data-icon="view" data-size="14"></span></button>
        <button class="btn btn--ghost btn--icon" data-remove title="删除（进废纸篓，可恢复）"><span data-icon="delete" data-size="14"></span></button>
      </div>
    `;
    row.querySelector("[data-toggle]")!.addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      btn.disabled = true;
      try {
        await api.toggleProjectSkill(projectId, s.agent_dir, s.relative_path, !s.enabled);
        toast(s.enabled ? `已禁用 ${s.name}` : `已启用 ${s.name}`, "success");
        load();
      } catch (err) {
        toast(`操作失败：${err}`, "error");
        btn.disabled = false;
      }
    });
    row.querySelector("[data-adopt]")?.addEventListener("click", async (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      btn.disabled = true;
      btn.textContent = "纳入管理中…";
      try {
        const r = await api.adoptProjectSkill(projectId, s.agent_dir, s.relative_path);
        await store.refresh();
        toast(`已纳入管理至 ${tildePath(r.new_path)}，稍后出现在技能库`, "success");
        load();
      } catch (err) {
        toast(`纳入管理失败：${err}`, "error");
        btn.disabled = false;
        btn.innerHTML = `<span data-icon="upload" data-size="13"></span>纳入管理`;
        paintIcons(btn);
      }
    });
    row.querySelector("[data-pull]")?.addEventListener("click", async (e) => {
      const ok = await ask(
        s.sync_status === "diverged"
          ? `「${s.name}」两侧都有改动。从中央更新会把项目副本移入废纸篓并写入中央库版本，确定继续？`
          : `用中央库版本更新「${s.name}」？项目里的当前副本会移入废纸篓。`,
        { title: "从中央更新", kind: "warning", okLabel: "更新", cancelLabel: "取消" },
      );
      if (!ok) return;
      const btn = e.currentTarget as HTMLButtonElement;
      btn.disabled = true;
      try {
        await api.updateProjectSkillFromCenter(projectId, s.agent_dir, s.relative_path);
        toast(`已从中央更新 ${s.name}`, "success");
        load();
      } catch (err) {
        toast(`更新失败：${err}`, "error");
        btn.disabled = false;
      }
    });
    row.querySelector("[data-view]")!.addEventListener("click", () => openDocModal(s));
    row.querySelector("[data-remove]")!.addEventListener("click", async () => {
      const ok = await ask(`删除项目里的「${s.name}」？\n\n会移入废纸篓（可恢复），${s.center_skill_id ? "中央库副本不受影响。" : "中央库里没有它的副本。"}`, {
        title: "删除技能",
        kind: "warning",
        okLabel: "删除",
        cancelLabel: "取消",
      });
      if (!ok) return;
      try {
        await api.deleteProjectSkill(projectId, s.agent_dir, s.relative_path);
        toast(`已删除 ${s.name}`, "success");
        load();
      } catch (err) {
        toast(`删除失败：${err}`, "error");
      }
    });
    return row;
  }

  /** 收编有意义的状态：中央没有 / 项目有独有改动。 */
  function canAdopt(s: ProjectSkillInfo): boolean {
    return ["project_only", "project_newer", "diverged"].includes(s.sync_status);
  }
  /** 从中央更新有意义的状态：中央有且项目不比中央新。 */
  function canPull(s: ProjectSkillInfo): boolean {
    return ["center_newer", "diverged", "project_newer"].includes(s.sync_status);
  }

  // ------------------------------------------------------------------
  // SKILL.md 预览弹窗
  // ------------------------------------------------------------------
  function openDocModal(s: ProjectSkillInfo): void {
    const body = document.createElement("div");
    body.innerHTML = `<p class="field__hint">读取中…</p>`;
    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:space-between;width:100%";
    footer.innerHTML = `
      <span class="proj-docmeta">${escapeHtml(s.agent_dir)}/${escapeHtml(s.relative_path)}</span>
      <button class="btn btn--primary btn--sm" data-act="close">关闭</button>
    `;
    const modal = openModal({ title: s.name, body, footer, width: 720 });
    footer.querySelector("[data-act='close']")!.addEventListener("click", () => modal.close());
    api
      .getProjectSkillDoc(projectId, s.agent_dir, s.relative_path)
      .then((doc) => {
        body.innerHTML = `
          ${doc.description ? `<p class="proj-docdesc">${escapeHtml(doc.description)}</p>` : ""}
          <pre class="proj-docpre">${escapeHtml(doc.content)}</pre>
        `;
      })
      .catch((e) => {
        body.innerHTML = `<p class="field__hint">读取失败：${escapeHtml(String(e))}</p>`;
      });
  }

  // ------------------------------------------------------------------
  // 从中央库安装弹窗：选技能 → 选 Agent 槽位 → 选模式
  // ------------------------------------------------------------------
  function openInstallModal(): void {
    if (slots.length === 0) {
      toast("没有可用的 Agent 槽位（需要至少一个路径以 /skills 结尾的 Agent）", "error");
      return;
    }
    const snap = store.get();
    const body = document.createElement("div");
    body.innerHTML = `
      <div class="col" style="gap:10px">
        <div>
          <div class="field__label" style="margin-bottom:6px">1 · 选择技能</div>
          <div class="search proj-search">
            <span class="search__icon" data-icon="search" data-size="14"></span>
            <input class="input" placeholder="搜索技能库…" />
          </div>
          <div class="proj-picklist" data-skilllist></div>
        </div>
        <div>
          <div class="field__label" style="margin-bottom:6px">2 · 安装到 Agent</div>
          <div class="proj-slotrow" data-slots></div>
        </div>
        <div>
          <div class="field__label" style="margin-bottom:6px">3 · 安装方式</div>
          <div class="proj-modes" data-modes></div>
          <p class="field__hint" data-modehint></p>
        </div>
      </div>
    `;
    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
    footer.innerHTML = `
      <button class="btn btn--ghost" data-act="cancel">取消</button>
      <button class="btn btn--primary" data-act="go" disabled>安装</button>
    `;
    const modal = openModal({ title: "从中央库安装到项目", body, footer, width: 640 });

    const skillList = body.querySelector<HTMLElement>("[data-skilllist]")!;
    const slotsHost = body.querySelector<HTMLElement>("[data-slots]")!;
    const modesHost = body.querySelector<HTMLElement>("[data-modes]")!;
    const modeHint = body.querySelector<HTMLElement>("[data-modehint]")!;
    const search = body.querySelector<HTMLInputElement>(".proj-search input")!;
    const goBtn = footer.querySelector<HTMLButtonElement>("[data-act='go']")!;
    footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());

    let pickedSkillId: string | null = null;
    let pickedSlot = slots[0]?.dir ?? null;
    let mode: "copy" | "symlink" = "copy";

    const allLibSkills = [...snap.skills.values()].sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
    function paintSkills() {
      const kw = search.value.trim().toLowerCase();
      const items = allLibSkills.filter(
        (s) => !kw || s.name.toLowerCase().includes(kw) || s.description.toLowerCase().includes(kw),
      );
      skillList.innerHTML =
        items.length === 0
          ? `<p class="field__hint">技能库为空或没有匹配 —— 先到「中央技能库」添加来源。</p>`
          : items
              .map(
                (s) => `
            <div class="ip-row proj-scan-row" data-skill="${escapeHtml(s.id)}" data-checked="${s.id === pickedSkillId}">
              <span class="proj-check" data-icon="check" data-size="13"></span>
              <div class="ip-row__main">
                <div class="ip-row__name">${escapeHtml(s.name)}</div>
                <div class="ip-row__path">${escapeHtml(s.description || tildePath(s.absolute_path))}</div>
              </div>
            </div>`,
              )
              .join("");
      for (const row of skillList.querySelectorAll<HTMLElement>("[data-skill]")) {
        row.addEventListener("click", () => {
          pickedSkillId = row.dataset.skill!;
          for (const r of skillList.querySelectorAll<HTMLElement>("[data-skill]")) {
            r.setAttribute("data-checked", String(r.dataset.skill === pickedSkillId));
          }
          goBtn.disabled = false;
        });
      }
      paintIcons(skillList);
    }
    search.addEventListener("input", paintSkills);
    paintSkills();

    slotsHost.innerHTML = slots
      .map(
        (sl) => `
        <button class="proj-slot ${sl.dir === pickedSlot ? "is-picked" : ""}" data-slot="${escapeHtml(sl.dir)}" title="${escapeHtml(sl.display)}">
          ${escapeHtml(sl.dir)}${sl.exists ? "" : `<i>未创建</i>`}
        </button>`,
      )
      .join("");
    for (const btn of slotsHost.querySelectorAll<HTMLButtonElement>("[data-slot]")) {
      btn.addEventListener("click", () => {
        pickedSlot = btn.dataset.slot!;
        for (const b of slotsHost.querySelectorAll("[data-slot]")) b.classList.toggle("is-picked", b === btn);
      });
    }

    const MODES: { key: "copy" | "symlink"; title: string; desc: string }[] = [
      { key: "copy", title: "副本", desc: "拷贝一份进项目，随 git 提交，与库互不影响" },
      { key: "symlink", title: "软链", desc: "链回中央仓库，库里更新项目立即生效；不随 git 走" },
    ];
    function paintModes() {
      modesHost.innerHTML = MODES.map(
        (m) => `
        <button class="proj-mode ${m.key === mode ? "is-picked" : ""}" data-mode="${m.key}">
          <span class="proj-mode__title">${m.title}</span>
          <span class="proj-mode__desc">${m.desc}</span>
        </button>`,
      ).join("");
      modeHint.textContent = MODES.find((m) => m.key === mode)!.desc;
      for (const btn of modesHost.querySelectorAll<HTMLButtonElement>("[data-mode]")) {
        btn.addEventListener("click", () => {
          mode = btn.dataset.mode as "copy" | "symlink";
          paintModes();
        });
      }
    }
    paintModes();

    goBtn.addEventListener("click", async (e) => {
      if (!pickedSkillId || !pickedSlot) return;
      const btn = e.currentTarget as HTMLButtonElement;
      btn.disabled = true;
      btn.textContent = "安装中…";
      try {
        await api.exportSkillToProject(pickedSkillId, projectId, pickedSlot, mode);
        modal.close();
        toast("已安装到项目", "success");
        load();
      } catch (err) {
        toast(`安装失败：${err}`, "error");
        btn.disabled = false;
        btn.textContent = "安装";
      }
    });
  }

  load();
}

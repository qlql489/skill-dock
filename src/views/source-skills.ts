// Source skills detail page — shows a source's skills with a file tree browser.
//
// Layout (three columns), identical to agent-skills.ts:
//   [skill list + search + install status] | [file tree (collapsible)] | [skill info + content]
//
// Unlike the agent-skills page, skill data comes from the in-memory store
// (store.skills filtered by source_id), not from list_target_skills. The file
// tree / content viewer reuse readSkillTree / readSkillFile with the skill's
// absolute_path as root.
//
// Each skill item carries an install-status badge:
//   - "已装 N" (green) when the skill is linked to N targets
//   - "未安装" (grey) when not linked to any target
// The right column's info bar additionally shows the skill description and the
// list of targets it is installed to.

import { api } from "../api";
import { store } from "../store";
import { paintIcons } from "../icon";
import { escapeHtml, marked } from "./library";
import type { Skill, FileTreeNode } from "../types";

export function renderSourceSkills(mount: HTMLElement, sourceId: string): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  view.className = "agent-detail";
  mount.append(view);

  const snap = store.get();
  const source = snap.sources.find((s) => s.id === sourceId);
  if (!source) {
    view.innerHTML = `<div class="empty"><h2>未找到该来源</h2></div>`;
    return;
  }

  // All skills belonging to this source, sorted by name.
  const sourceSkills: Skill[] = snap.skills
    .filter((s) => s.source_id === sourceId)
    .sort((a, b) => a.name.localeCompare(b.name));

  // --- State ---
  let allSkills: Skill[] = sourceSkills;
  let selectedSkill: Skill | null = null;
  let skillRootPath = "";
  let tree: FileTreeNode[] = [];
  let expandedPaths = new Set<string>();
  let selectedFile = "";
  let treeVisible = true;
  let searchQuery = "";

  // --- Header with back button ---
  const head = document.createElement("div");
  head.className = "agent-detail__head";
  head.innerHTML = `
    <button class="btn btn--ghost btn--sm" id="source-back"><span data-icon="arrowLeft" data-size="14"></span>返回</button>
    <div class="agent-detail__title">${escapeHtml(source.name)} 的 skills</div>
  `;
  view.append(head);
  head.querySelector("#source-back")!.addEventListener("click", () => {
    // 来源页已并入技能库「按来源」tab：先切 tab 再返回。
    try { localStorage.setItem("skill-dock:display-mode", "by-source"); } catch { /* ignore */ }
    location.hash = "#/library";
  });

  // --- Three-column body ---
  const body = document.createElement("div");
  body.className = "agent-detail__body";
  view.append(body);

  // Left: skill list + search
  const skillListCol = document.createElement("div");
  skillListCol.className = "agent-detail__skills";
  body.append(skillListCol);

  // Center: file tree (collapsible)
  const treeCol = document.createElement("div");
  treeCol.className = "agent-detail__tree";
  body.append(treeCol);

  // Right: skill info + content viewer
  const contentCol = document.createElement("div");
  contentCol.className = "agent-detail__content";
  body.append(contentCol);

  // --- Initial load ---
  if (allSkills.length === 0) {
    skillListCol.innerHTML = `<div class="agent-detail__empty">没有 skill</div>`;
    treeCol.innerHTML = "";
    contentCol.innerHTML = `<div class="agent-detail__empty">选择一个 skill 查看内容</div>`;
  } else {
    renderSkillList();
    selectSkill(allSkills[0]);
  }

  function getFilteredSkills(): Skill[] {
    if (!searchQuery) return allSkills;
    const q = searchQuery.toLowerCase();
    return allSkills.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        (s.description ?? "").toLowerCase().includes(q),
    );
  }

  /** Count how many targets a skill is installed to. */
  function installedCount(skillId: string): number {
    return store.get().installations.filter((i) => i.skill_id === skillId).length;
  }

  /** Targets a skill is installed to. */
  function installedTargetsFor(skillId: string) {
    const snap = store.get();
    const targetIds = new Set(
      snap.installations
        .filter((i) => i.skill_id === skillId)
        .map((i) => i.target_id),
    );
    return snap.targets.filter((t) => targetIds.has(t.id));
  }

  function renderSkillList() {
    skillListCol.innerHTML = "";

    // Search box at the top.
    const searchBox = document.createElement("div");
    searchBox.className = "skill-search";
    searchBox.innerHTML = `
      <span class="skill-search__icon" data-icon="search" data-size="13"></span>
      <input class="skill-search__input" data-input="search" type="text" placeholder="搜索 skill…" value="${escapeHtml(searchQuery)}" />
    `;
    skillListCol.append(searchBox);
    const input = searchBox.querySelector<HTMLInputElement>("[data-input='search']")!;
    input.addEventListener("input", () => {
      searchQuery = input.value;
      renderSkillListRows();
    });
    if (searchQuery) input.focus();

    renderSkillListRows();
    paintIcons(skillListCol);
  }

  function renderSkillListRows() {
    const oldItems = skillListCol.querySelectorAll(".skill-item");
    oldItems.forEach((el) => el.remove());

    const filtered = getFilteredSkills();
    if (filtered.length === 0) {
      const empty = document.createElement("div");
      empty.className = "agent-detail__empty";
      empty.style.padding = "var(--s-3)";
      empty.textContent = searchQuery ? "没有匹配" : "没有 skill";
      skillListCol.append(empty);
      return;
    }
    for (const s of filtered) {
      const item = document.createElement("div");
      item.className = "skill-item";
      if (selectedSkill?.id === s.id) item.classList.add("skill-item--active");

      const instCount = installedCount(s.id);
      const instBadge = instCount > 0
        ? `<span class="skill-item__badge skill-item__badge--installed" title="已安装到 ${instCount} 个 agent">已装 ${instCount}</span>`
        : `<span class="skill-item__badge skill-item__badge--not-installed" title="未安装到任何 agent">未安装</span>`;

      item.innerHTML = `
        <span class="skill-item__icon" data-icon="file" data-size="14"></span>
        <span class="skill-item__name">${escapeHtml(s.name)}</span>
        ${instBadge}
      `;
      item.addEventListener("click", () => selectSkill(s));
      skillListCol.append(item);
    }
    paintIcons(skillListCol);
  }

  async function selectSkill(skill: Skill) {
    selectedSkill = skill;
    skillRootPath = skill.absolute_path;
    selectedFile = "SKILL.md";
    expandedPaths = new Set();
    renderSkillListRows(); // update active highlight

    treeCol.innerHTML = `<div class="agent-detail__loading">加载中…</div>`;
    renderContentHeader();
    contentCol.innerHTML += `<div class="agent-detail__loading">加载中…</div>`;
    try {
      tree = await api.readSkillTree(skillRootPath);
      for (const node of tree) {
        if (node.is_dir) expandedPaths.add(node.path);
      }
      renderTree();
      await loadFile("SKILL.md");
    } catch (e) {
      treeCol.innerHTML = `<div class="agent-detail__empty">读取文件树失败</div>`;
      contentCol.innerHTML = `<div class="agent-detail__empty">${escapeHtml(String(e))}</div>`;
    }
  }

  function renderTree() {
    treeCol.innerHTML = "";

    // Tree toolbar: toggle-hide button + file count.
    const toolbar = document.createElement("div");
    toolbar.className = "tree-toolbar";
    const fileCount = countFiles(tree);
    toolbar.innerHTML = `
      <span class="tree-toolbar__count">${fileCount} 个文件</span>
      <button class="btn btn--ghost btn--icon btn--sm" data-action="toggle-tree" title="隐藏文件树"><span data-icon="arrowLeft" data-size="13"></span></button>
    `;
    treeCol.append(toolbar);
    toolbar.querySelector("[data-action='toggle-tree']")!.addEventListener("click", () => {
      treeVisible = false;
      updateTreeVisibility();
    });

    const treeRoot = document.createElement("div");
    treeRoot.className = "tree-root";
    for (const node of tree) {
      treeRoot.append(renderTreeNode(node, 0));
    }
    treeCol.append(treeRoot);
    paintIcons(treeCol);
  }

  function countFiles(nodes: FileTreeNode[]): number {
    let count = 0;
    for (const n of nodes) {
      if (!n.is_dir) count++;
      else count += countFiles(n.children);
    }
    return count;
  }

  function updateTreeVisibility() {
    if (treeVisible) {
      treeCol.style.display = "";
      body.classList.remove("tree-hidden");
      renderContentHeader();
    } else {
      treeCol.style.display = "none";
      body.classList.add("tree-hidden");
      renderContentHeader();
    }
  }

  function renderTreeNode(node: FileTreeNode, depth: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "tree-node";
    row.style.paddingLeft = `${depth * 14 + 8}px`;

    if (node.path === selectedFile) row.classList.add("tree-node--selected");

    if (node.is_dir) {
      const isExpanded = expandedPaths.has(node.path);
      row.innerHTML = `
        <span class="tree-node__caret" style="color:var(--ink-mute)">${isExpanded ? "▾" : "▸"}</span>
        <span class="tree-node__icon" data-icon="folder" data-size="13"></span>
        <span class="tree-node__label">${escapeHtml(node.name)}</span>
      `;
      row.addEventListener("click", () => {
        if (isExpanded) expandedPaths.delete(node.path);
        else expandedPaths.add(node.path);
        renderTree();
      });
      const wrapper = document.createElement("div");
      wrapper.append(row);
      if (isExpanded) {
        for (const child of node.children) {
          wrapper.append(renderTreeNode(child, depth + 1));
        }
      }
      return wrapper;
    } else {
      row.innerHTML = `
        <span class="tree-node__caret" style="visibility:hidden">▸</span>
        <span class="tree-node__icon" data-icon="file" data-size="13"></span>
        <span class="tree-node__label">${escapeHtml(node.name)}</span>
      `;
      row.addEventListener("click", () => loadFile(node.path));
      return row;
    }
  }

  /** Render the info bar at the top of the content column. Shows the skill's
   *  description, file path, install status, and the list of installed targets.
   *  Also has a "show tree" button when the tree is hidden. */
  function renderContentHeader() {
    const oldHeader = contentCol.querySelector(".skill-info");
    oldHeader?.remove();

    const info = document.createElement("div");
    info.className = "skill-info skill-info--source";

    // Path info.
    let pathHtml = "";
    if (selectedSkill) {
      pathHtml = `
        <div class="skill-info__row">
          <span class="skill-info__label">文件路径</span>
          <span class="skill-info__path" title="${escapeHtml(skillRootPath)}">${escapeHtml(skillRootPath)}</span>
        </div>
      `;
    }

    // Description + installed targets.
    let descHtml = "";
    if (selectedSkill) {
      const instTargets = installedTargetsFor(selectedSkill.id);
      const targetChips = instTargets.length > 0
        ? instTargets.map((t) => `<span class="skill-info__target" title="${escapeHtml(t.skills_dir)}">${escapeHtml(t.name)}</span>`).join("")
        : `<span class="skill-info__target skill-info__target--none">未安装到任何 agent</span>`;
      descHtml = `
        ${selectedSkill.description ? `<div class="skill-info__desc">${escapeHtml(selectedSkill.description)}</div>` : ""}
        <div class="skill-info__targets">${targetChips}</div>
      `;
    }

    // "Show tree" button when tree is hidden.
    const showTreeBtn = !treeVisible
      ? `<button class="btn btn--ghost btn--sm" data-action="show-tree" title="显示文件树"><span data-icon="folder" data-size="13"></span>文件树</button>`
      : "";

    info.innerHTML = `
      <div class="skill-info__top">
        <div class="skill-info__paths">${pathHtml}</div>
        ${showTreeBtn}
      </div>
      <div class="skill-info__detail">${descHtml}</div>
    `;

    contentCol.prepend(info);
    info.querySelector("[data-action='show-tree']")?.addEventListener("click", () => {
      treeVisible = true;
      updateTreeVisibility();
      renderTree();
    });
    paintIcons(info);
  }

  async function loadFile(filePath: string) {
    selectedFile = filePath;
    renderTree(); // update selection highlight
    // Keep the info header, replace everything below it.
    const header = contentCol.querySelector(".skill-info");
    contentCol.innerHTML = "";
    if (header) contentCol.append(header);
    const loading = document.createElement("div");
    loading.className = "agent-detail__loading";
    loading.textContent = "加载中…";
    contentCol.append(loading);
    try {
      const content = await api.readSkillFile(skillRootPath, filePath);
      loading.remove();
      renderContent(filePath, content);
    } catch (e) {
      loading.remove();
      const err = document.createElement("div");
      err.className = "agent-detail__empty";
      err.textContent = String(e);
      contentCol.append(err);
    }
  }

  function renderContent(filePath: string, content: string) {
    const isMd = filePath.endsWith(".md");
    if (isMd) {
      const rendered = document.createElement("div");
      rendered.className = "md-body";
      try {
        rendered.innerHTML = marked.parse(content, { async: false }) as string;
      } catch {
        rendered.textContent = content;
      }
      contentCol.append(rendered);
    } else {
      const pre = document.createElement("pre");
      pre.className = "code-viewer";
      pre.textContent = content;
      contentCol.append(pre);
    }
  }

  paintIcons(view);
}

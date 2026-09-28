// Skill 详情页 —— 从库页或 Agent 页点进来的统一整页视图（左侧导航保持）。
// 两种模式：
//   · 库内 skill（#/skill/:id）—— 完整版式：Agents 安装区 + 编辑/删除；
//   · 未纳管 skill（#/local-skill/:path，来自本机skill管理）—— 同版式但
//     没有 Agents 安装区（不在技能库，无从安装），也没有编辑/删除。
//
// 布局：
//   顶部    返回 + 名称徽章 + 操作（安装到目标 / 访达 / 删除）
//   信息区  来源、路径、大小、哈希、标签、软链指向；已安装的 Agent 以
//           token 呈现，点击即取消安装（未安装的 agent 以描边 token 呈现，
//           点击即安装）
//   文件区  左侧文件树，点击文件右侧看内容；SKILL.md 默认渲染并支持编辑

import { api } from "../api";
import { store, sourceById, tildePath, formatBytes, formatRelative } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { openModal } from "../modal";
import { ask } from "@tauri-apps/plugin-dialog";
import { escapeHtml, marked } from "./library";
import { agentIconMarkup, paintAgentIcons } from "../agent-icons";
import type { FileTreeNode, LocalSkillMeta, PreviewCell, SkillTargetDiff } from "../types";

/** 返回目标：从哪个页面进入详情，返回时就回哪个页面（SPA 内记忆）。 */
let backHash: string | null = null;

/** 兼容旧调用点：库页/其它页面调用它即可跳转到详情路由。
 *  `back` 可选——传入来源页 hash（如 agent 页），返回按钮会优先回到那里。 */
export function openSkillDetail(skillId: string, back?: string): void {
  backHash = back ?? null;
  location.hash = `#/skill/${encodeURIComponent(skillId)}`;
}

/** 本机skill管理 → 未纳管 skill 的详情跳转（按磁盘路径）。 */
export function openLocalSkillDetail(realPath: string, back?: string): void {
  backHash = back ?? null;
  location.hash = `#/local-skill/${encodeURIComponent(realPath)}`;
}

/** git 仓库地址（https/git@/git:// 三种形态）→ 浏览器可打开的仓库页；
 *  非 GitHub 地址返回 null。 */
function githubBrowseUrl(url: string): string | null {
  const m = url.match(/^(?:https?:\/\/|git@|git:\/\/)(?:www\.)?github\.com[/:]([\w.-]+\/[\w.-]+?)(?:\.git)?\/?$/i);
  return m ? `https://github.com/${m[1]}` : null;
}

export function renderSkillPage(mount: HTMLElement, skillId: string): void {
  renderDetail(mount, { skillId });
}

/** 未纳管 skill 的详情：同一套版式，但没有 Agents 安装区 —— 未纳管的
 *  skill 不在中央技能库里，没有"装到哪些 Agent"可切换，也不提供编辑/删除
 *  （想纳入管理请回「本机skill管理」页点「纳入管理」）。 */
export function renderLocalSkillPage(mount: HTMLElement, realPath: string): void {
  renderDetail(mount, { localPath: realPath });
}

function renderDetail(mount: HTMLElement, opts: { skillId?: string; localPath?: string }): void {
  const managed = !!opts.skillId;
  const skill = opts.skillId ? store.get().skills.find((s) => s.id === opts.skillId) : undefined;
  // 未纳管模式下没有库内记录，一切以磁盘路径为准。
  const basePath = skill?.absolute_path ?? opts.localPath ?? "";
  const fallbackName = basePath.split("/").filter(Boolean).pop() ?? basePath;

  mount.innerHTML = "";
  const view = document.createElement("div");
  view.className = "skpage";

  if (managed && !skill) {
    view.innerHTML = `
      <div class="empty">
        <div class="empty__icon" data-icon="alert" data-size="22"></div>
        <h2 class="empty__title">找不到这个 skill</h2>
        <p class="empty__sub">它可能已被删除或来源被移除。</p>
        <div class="empty__actions">
          <button class="btn btn--ghost" data-back><span data-icon="arrowLeft" data-size="13"></span>返回</button>
        </div>
      </div>`;
    view.querySelector("[data-back]")!.addEventListener("click", () => history.back());
    mount.append(view);
    paintIcons(view);
    return;
  }
  const skillRef = skill!;
  const initialName = managed ? skillRef.name : fallbackName;
  const initialDesc = managed ? (skillRef.description ?? "") : "";

  // ============ 头部 ============
  const head = document.createElement("div");
  head.className = "skpage__head";
  head.innerHTML = `
    <button class="btn btn--ghost btn--icon" data-back title="返回"><span data-icon="arrowLeft" data-size="15"></span></button>
    <div class="skpage__titlewrap">
      <h1 class="skpage__title">
        <span data-title-name>${escapeHtml(initialName)}</span>
        ${managed && skillRef.is_shared ? `<span class="tag tag--mono" title="位于中央仓库，多 agent 共用">共用</span>` : ""}
        ${managed && skillRef.is_symlink ? `<span class="tag" title="这个 skill 目录本身是软链接">软链</span>` : ""}
        ${managed ? "" : `<span class="tag" title="不在中央技能库里，可在「本机skill管理」页纳入管理">未纳管</span>`}
      </h1>
      <p class="skpage__desc" data-desc ${initialDesc ? "" : "hidden"}>${escapeHtml(initialDesc)}</p>
    </div>
    <div style="flex:1"></div>
    <button class="btn btn--ghost btn--sm" data-action="reveal"><span data-icon="external" data-size="13"></span>访达</button>
    ${managed ? `<button class="btn btn--danger btn--ghost btn--sm" data-action="delete"><span data-icon="delete" data-size="13"></span>删除</button>` : ""}
  `;
  head.querySelector("[data-back]")!.addEventListener("click", () => {
    if (backHash) {
      const target = backHash;
      backHash = null;
      location.hash = target;
    } else {
      history.back();
    }
  });
  head.querySelector("[data-action='reveal']")!.addEventListener("click", () => api.revealInFinder(basePath));
  head.querySelector("[data-action='delete']")?.addEventListener("click", async () => {
    const snap = store.get();
    const linkCount = snap.installations.filter((i) => i.skill_id === skillRef.id && (i.status === "ok" || i.status === "")).length;
    const src = sourceById(skillRef.source_id);
    const gitNote = src?.kind === "github"
      ? "\n⚠ 该 skill 位于 Git 克隆内，之后「更新来源」可能把它重新拉回来。"
      : "";
    // window.confirm 在 Tauri WebView 里不弹框直接返回 true，必须用插件对话框。
    if (!(await ask(
      `彻底删除「${skillRef.name}」？\n` +
      `· 移除 ${linkCount} 个 agent 上的软链\n` +
      `· 删除目录：${tildePath(skillRef.absolute_path)}（移入废纸篓）` +
      gitNote +
      `\n应用内不保留副本，确定继续？`,
      { title: "彻底删除 skill", kind: "warning" },
    ))) return;
    try {
      const removed = await api.deleteSkillForever(skillRef.id);
      await store.refresh();
      toast(`已删除（移除 ${removed} 个软链）`, "success");
      history.back();
    } catch (e) {
      toast(`删除失败：${e}`, "error");
    }
  });
  view.append(head);

  // ============ 信息区 + Agent tokens ============
  const infoZone = document.createElement("div");
  infoZone.className = "skpage__info";
  view.append(infoZone);

  // Agent 冲突态缓存（失效/占用/同名占位）：paintInfo 先画基础开关态，
  // preview_cells 返回后带冲突标记重画一遍。
  let cellsByTarget = new Map<string, PreviewCell>();
  let cellsToken = 0;

  async function refreshCells() {
    if (!managed) return;
    const token = ++cellsToken;
    try {
      const snap = store.get();
      if (!snap.skills.some((s) => s.id === skillRef.id)) return;
      const targets = snap.targets.filter((t) => t.enabled);
      const cells = await api.previewCells([skillRef.id], targets.map((t) => t.id));
      if (token !== cellsToken) return;
      cellsByTarget = new Map(cells.map((c) => [c.target_id, c]));
      paintInfo();
    } catch {
      /* 拿不到冲突预览就保持基础开关态 */
    }
  }

  function conflictTip(st: string, cell?: PreviewCell): string {
    const d = cell?.detail ? `\n${cell.detail}` : "";
    if (st === "stale") return `软链失效或指向别处${d}\n点击查看对比，可一键修复`;
    if (st === "occupied") return `名字被其他软链占用${d}\n点击查看对比，可替换`;
    return `同名目录或文件已存在（应用不会自动覆盖）${d}\n点击查看文件对比`;
  }

  // 未纳管模式：异步补齐 frontmatter 名称/描述与大小/修改时间。
  let localMeta: LocalSkillMeta | null = null;

  function paintInfo() {
    if (!managed) {
      paintLocalInfo();
      return;
    }
    const snap = store.get();
    const cur = snap.skills.find((s) => s.id === skillRef.id);
    if (!cur) return; // 已删除 —— 等待路由离开
    const src = sourceById(cur.source_id);

    // Agent 网格：图标 + 名称 + 开关，铺在简介下方（对标 AgentToggleSection）。
    // 有冲突（失效/占用/同名占位）的格子换成冲突徽章，点击看原因与文件对比。
    const enabledTargets = snap.targets.filter((t) => t.enabled);
    const tiles: string[] = [];
    for (const t of enabledTargets) {
      const inst = snap.installations.find(
        (i) => i.skill_id === cur.id && i.target_id === t.id && (i.status === "ok" || i.status === ""),
      );
      const cell = cellsByTarget.get(t.id);
      const st = cell?.state ?? "absent";
      const conflict = st === "stale" || st === "occupied" || st === "blocked";
      const on = !!inst && !conflict;
      const flag = st === "stale" ? "失效" : conflict ? "冲突" : "";
      const tip = conflict
        ? escapeHtml(conflictTip(st, cell))
        : on
          ? `已链接到 ${escapeHtml(tildePath(inst!.link_path))} — 点击取消安装`
          : "未安装 — 点击安装";
      tiles.push(
        `<button class="agent-tile ${on ? "is-on" : "is-off"} ${conflict ? "is-conflict" : ""}"
           data-tid="${escapeHtml(t.id)}" data-state="${st}" title="${tip}">
           ${agentIconMarkup(t.id, t.name, "agent-icon--chip")}
           <span class="agent-tile__name">${escapeHtml(t.name)}</span>
           ${flag
             ? `<span class="agent-tile__flag">${flag}</span>`
             : `<span class="switch ${on ? "is-on" : ""} switch--static" aria-hidden="true"><span class="switch__thumb"></span></span>`}
         </button>`,
      );
    }
    const installedCount = tiles.filter((h) => h.includes("is-on")).length;

    // GitHub 来源：徽章可点击，跳浏览器打开仓库页（location 可能是 https/git@/git:// 形态）。
    const repoUrl = src?.kind === "github" ? githubBrowseUrl(src.location) : null;
    const chipText = src ? (src.kind === "github" ? "git" : src.kind === "local" ? "本地" : src.kind) : "";
    const srcChip = !src
      ? ""
      : repoUrl
        ? `<button class="src2-chip src2-chip--link" style="margin-left:6px" data-open-repo title="在浏览器打开 ${escapeHtml(repoUrl)}">git ↗</button>`
        : `<span class="src2-chip" style="margin-left:6px">${escapeHtml(chipText)}</span>`;

    infoZone.innerHTML = `
      <div class="surface skpage__meta">
        <div class="kv">
          <span class="kv__k">来源</span><span class="kv__v">${escapeHtml(src?.name ?? "—")}${srcChip}</span>
          <span class="kv__k">路径</span><span class="kv__v" style="font-family:var(--font-mono);font-size:12px" title="${escapeHtml(cur.absolute_path)}">${escapeHtml(tildePath(cur.absolute_path))}</span>
          ${cur.is_symlink && cur.symlink_target ? `<span class="kv__k">软链指向</span><span class="kv__v" style="font-family:var(--font-mono);font-size:12px" title="${escapeHtml(cur.symlink_target)}">${escapeHtml(tildePath(cur.symlink_target))}</span>` : ""}
          <span class="kv__k">大小</span><span class="kv__v">${formatBytes(cur.size_bytes)}</span>
          <span class="kv__k">修改</span><span class="kv__v">${formatRelative(cur.modified_at)}</span>
          <span class="kv__k">哈希</span><span class="kv__v" style="font-family:var(--font-mono);font-size:10px">${escapeHtml(cur.content_hash.slice(0, 16))}</span>
        </div>
      </div>
      <div class="surface skpage__agents">
        <div class="kicker" style="margin-bottom:12px">
          <span class="kicker__num">Agents</span>
          <span style="font-size:12px;color:var(--ink-mute);margin-left:8px">已装 ${installedCount} / ${enabledTargets.length} · 点击卡片切换安装</span>
        </div>
        <div class="agent-tile-grid">${tiles.join("")}</div>
      </div>
    `;

    paintAgentIcons(infoZone);

    infoZone.querySelector("[data-open-repo]")?.addEventListener("click", () => {
      if (!repoUrl) return;
      api.openExternal(repoUrl).catch((e) => toast(`打开失败：${e?.message ?? e}`, "error"));
    });

    infoZone.querySelectorAll<HTMLButtonElement>(".agent-tile").forEach((tok) => {
      tok.addEventListener("click", async () => {
        const tid = tok.dataset.tid!;
        const st = tok.dataset.state;
        // 冲突格：不直接安装（装了也只会报错），先看原因与文件对比。
        if (st === "stale" || st === "occupied" || st === "blocked") {
          openConflictModal(tid);
          return;
        }
        const wasOn = tok.classList.contains("is-on");
        tok.disabled = true;
        try {
          if (wasOn) {
            await api.uninstallSkill(cur.id, tid);
            toast(`已从 ${targetName(snap.targets, tid)} 卸载`, "success");
          } else {
            await api.installSkill(cur.id, tid, "fail");
            toast(`已安装到 ${targetName(snap.targets, tid)}`, "success");
          }
          await store.refresh();
          refreshCells();
        } catch (e: any) {
          const msg = String(e?.message ?? e);
          if (!wasOn && msg.includes("已被占用")) {
            if (await ask(`${skillRef.name} → ${targetName(snap.targets, tid)}：${msg}\n\n替换掉现有的软链？`, {
              title: "替换现有软链",
              kind: "warning",
            })) {
              try {
                await api.installSkill(cur.id, tid, "replace");
                await store.refresh();
                refreshCells();
                toast(`已替换并安装`, "success");
              } catch (e2: any) {
                toast(`替换失败：${e2?.message ?? e2}`, "error");
              }
            }
          } else {
            toast(`操作失败：${msg}`, "error");
          }
        } finally {
          tok.disabled = false;
        }
      });
    });

    // 冲突弹窗：冲突原因 + 与托管目录的逐文件对比（内容哈希/大小/修改时间）。
    function openConflictModal(tid: string): void {
      const snap = store.get();
      const target = snap.targets.find((t) => t.id === tid);
      if (!target) return;
      const cell = cellsByTarget.get(tid);
      const stateLabel =
        cell?.state === "stale" ? "链接失效（安装记录还在，但软链指向别处）"
        : cell?.state === "occupied" ? "名字被其他软链占用"
        : "同名目录或文件已存在（不是本应用创建的软链，不会自动覆盖）";

      const body = document.createElement("div");
      body.innerHTML = `
        <p class="field__hint" style="margin:0 0 10px">
          <b style="color:var(--ink)">${escapeHtml(skillRef.name)}</b> →
          <b style="color:var(--ink)">${escapeHtml(target.name)}</b>
          <span class="tag" style="margin-left:6px">${stateLabel}</span>
        </p>
        ${cell?.detail ? `<div class="cmp-reason">${escapeHtml(cell.detail)}</div>` : ""}
        <div class="cmp-zone" data-diff style="margin-top:12px">对比文件中…</div>
      `;
      const footer = document.createElement("div");
      footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
      const fixBtn =
        cell?.state === "stale"
          ? `<button class="btn btn--primary" data-act="fix"><span data-icon="link" data-size="13"></span>修复链接（改指本 skill）</button>`
          : cell?.state === "occupied"
            ? `<button class="btn btn--primary" data-act="fix"><span data-icon="link" data-size="13"></span>替换并安装</button>`
            : "";
      footer.innerHTML = `
        <button class="btn btn--ghost" data-act="reveal" data-path="" disabled>在访达中打开</button>
        <span style="flex:1"></span>
        <button class="btn btn--ghost" data-act="close">关闭</button>
        ${fixBtn}
      `;

      const modal = openModal({ title: `${skillRef.name} 的安装冲突`, body, footer, width: 680 });
      paintIcons(footer);

      footer.querySelector("[data-act='close']")!.addEventListener("click", () => modal.close());
      footer.querySelector("[data-act='reveal']")!.addEventListener("click", (e) => {
        const p = (e.currentTarget as HTMLElement).dataset.path;
        if (p) api.revealInFinder(p).catch((err) => toast(`打开失败：${err}`, "error"));
      });
      footer.querySelector("[data-act='fix']")?.addEventListener("click", async (e) => {
        const btn = e.currentTarget as HTMLButtonElement;
        btn.disabled = true;
        try {
          await api.installSkill(skillRef.id, tid, cell?.state === "occupied" ? "replace" : "fail");
          await store.refresh();
          refreshCells();
          toast(cell?.state === "occupied" ? "已替换并安装" : "已修复链接", "success");
          modal.close();
        } catch (err: any) {
          toast(`失败：${err?.message ?? err}`, "error");
        } finally {
          btn.disabled = false;
        }
      });

      const zone = body.querySelector<HTMLElement>("[data-diff]")!;
      api.diffSkillTarget(skillRef.id, tid)
        .then((d) => renderDiff(zone, footer, d))
        .catch((e) => { zone.innerHTML = `<p class="field__hint">对比失败：${escapeHtml(String(e))}</p>`; });
    }

    function fmtTime(ms: number | null): string {
      if (!ms) return "—";
      const dt = new Date(ms);
      const p = (n: number) => String(n).padStart(2, "0");
      return `${dt.getFullYear()}-${p(dt.getMonth() + 1)}-${p(dt.getDate())} ${p(dt.getHours())}:${p(dt.getMinutes())}`;
    }

    function renderDiff(zone: HTMLElement, footer: HTMLElement, d: SkillTargetDiff): void {
      const revealBtn = footer.querySelector<HTMLButtonElement>("[data-act='reveal']");
      if (d.compared_dir && revealBtn) {
        revealBtn.dataset.path = d.compared_dir;
        revealBtn.disabled = false;
        revealBtn.title = tildePath(d.compared_dir);
      }
      if (d.note) {
        zone.innerHTML = `<p class="field__hint" style="margin:0">${escapeHtml(d.note)}</p>`;
        return;
      }
      const rank: Record<SkillTargetDiff["rows"][number]["status"], number> = { changed: 0, only_source: 1, only_target: 2, same: 3 };
      const label: Record<string, string> = { same: "一致", changed: "内容不同", only_source: "仅仓库有", only_target: "仅 agent 有" };
      const rows = [...d.rows].sort((a, b) => rank[a.status] - rank[b.status] || a.path.localeCompare(b.path));
      const c = { same: 0, changed: 0, only_source: 0, only_target: 0 };
      for (const r of rows) c[r.status]++;
      const cellHtml = (side: "source" | "target", r: SkillTargetDiff["rows"][number]): string => {
        const size = side === "source" ? r.source_size : r.target_size;
        const mtime = side === "source" ? r.source_mtime : r.target_mtime;
        if (size == null && mtime == null) return `<span class="cmp-na">—</span>`;
        return `<div>${fmtTime(mtime)}</div><div style="color:var(--ink-mute)">${formatBytes(size ?? 0)}</div>`;
      };
      zone.innerHTML = `
        <div class="cmp-sum">
          共 ${d.rows.length} 个文件：一致 <b>${c.same}</b> · 内容不同 <b class="is-warn">${c.changed}</b> ·
          仅仓库 <b>${c.only_source}</b> · 仅 agent 侧 <b>${c.only_target}</b>
        </div>
        <div class="cmp-paths">
          <div><span class="cmp-side">仓库版</span>${escapeHtml(tildePath(skillRef.absolute_path))}</div>
          <div><span class="cmp-side">agent 侧</span>${escapeHtml(tildePath(d.compared_dir ?? ""))}</div>
        </div>
        <div class="cmp-scroll">
        <table class="cmp-table">
          <thead><tr><th>文件</th><th>状态</th><th>仓库版修改时间</th><th>agent 版修改时间</th><th>大小（仓 / agent）</th></tr></thead>
          <tbody>
            ${rows.map((r) => `
              <tr class="cmp-row--${r.status}">
                <td class="cmp-path" title="${escapeHtml(r.path)}">${escapeHtml(r.path)}</td>
                <td><span class="cmp-badge cmp-badge--${r.status}">${label[r.status]}</span></td>
                <td>${cellHtml("source", r)}</td>
                <td>${cellHtml("target", r)}</td>
                <td class="cmp-size">${r.source_size != null && r.target_size != null ? `${formatBytes(r.source_size)} / ${formatBytes(r.target_size)}` : "—"}</td>
              </tr>`).join("")}
          </tbody>
        </table>
        </div>
      `;
    }
  }

  function targetName(targets: { id: string; name: string }[], tid: string): string {
    return targets.find((t) => t.id === tid)?.name ?? tid;
  }

  // 未纳管：只有一张信息面 —— 没有 Agents 安装区，也没有编辑/删除入口。
  function paintLocalInfo() {
    infoZone.innerHTML = `
      <div class="surface skpage__meta">
        <div class="kv">
          <span class="kv__k">路径</span><span class="kv__v" style="font-family:var(--font-mono);font-size:12px" title="${escapeHtml(basePath)}">${escapeHtml(tildePath(basePath))}</span>
          <span class="kv__k">大小</span><span class="kv__v">${localMeta ? formatBytes(localMeta.size_bytes) : "—"}</span>
          <span class="kv__k">修改</span><span class="kv__v">${localMeta?.modified_at ? formatRelative(localMeta.modified_at) : "—"}</span>
        </div>
        <p class="field__hint" style="margin:10px 0 0">这个 skill 不在中央技能库里，所以没有 Agents 安装区；想纳入管理，请到「本机skill管理」页对它点「纳入管理」。</p>
      </div>
    `;
  }

  // ============ 文件区 ============
  const filesZone = document.createElement("div");
  filesZone.className = "skpage__files";
  view.append(filesZone);

  const treeCol = document.createElement("div");
  treeCol.className = "skpage__tree";
  const viewerCol = document.createElement("div");
  viewerCol.className = "skpage__viewer";
  filesZone.append(treeCol, viewerCol);

  let currentFile = "SKILL.md";
  let viewerMode: "render" | "source" | "edit" = "render";
  let originalContent = "";

  async function loadTree() {
    treeCol.innerHTML = `<div class="skpage__tree-loading">加载文件树…</div>`;
    try {
      const nodes = await api.readSkillTree(basePath);
      treeCol.innerHTML = "";
      const title = document.createElement("div");
      title.className = "skpage__tree-title";
      title.textContent = "文件";
      treeCol.append(title);
      if (nodes.length === 0) {
        treeCol.insertAdjacentHTML("beforeend", `<div class="skpage__tree-loading">空目录</div>`);
        return;
      }
      rootNodes = nodes;
      expandedDirs.clear();
      paintTree();
    } catch (e) {
      treeCol.innerHTML = `<div class="skpage__tree-loading">加载失败：${escapeHtml(String(e))}</div>`;
    }
  }

  /** 文件树根节点 + 已展开目录集合（默认全折叠，点击目录展开/收起）。 */
  let rootNodes: FileTreeNode[] = [];
  const expandedDirs = new Set<string>();

  function paintTree() {
    // 树行插在标题之后，重绘时保留展开状态与 active 高亮。
    treeCol.querySelectorAll(".sktree-row").forEach((el) => el.remove());
    const title = treeCol.querySelector(".skpage__tree-title")!;
    let cursor: Element = title;
    for (const r of collectRows(rootNodes, 0)) {
      cursor.after(r);
      cursor = r;
    }
    paintIcons(treeCol);
  }

  function collectRows(nodes: FileTreeNode[], depth: number): HTMLElement[] {
    const rows: HTMLElement[] = [];
    for (const n of nodes) {
      const row = document.createElement("div");
      row.className = "sktree-row";
      if (n.path === currentFile) row.classList.add("is-active");
      row.style.paddingLeft = `${10 + depth * 14}px`;
      if (n.is_dir) {
        const expanded = expandedDirs.has(n.path);
        row.innerHTML = `
          <span class="sktree-caret" style="color:var(--ink-mute)">${expanded ? "▾" : "▸"}</span>
          <span class="icon" data-icon="folder" data-size="12"></span>
          <span class="sktree-row__name">${escapeHtml(n.name)}</span>
        `;
        row.addEventListener("click", () => {
          if (expandedDirs.has(n.path)) expandedDirs.delete(n.path);
          else expandedDirs.add(n.path);
          paintTree();
        });
        rows.push(row);
        if (expanded) rows.push(...collectRows(n.children, depth + 1));
      } else {
        row.innerHTML = `
          <span class="sktree-caret" style="visibility:hidden">▸</span>
          <span class="icon" data-icon="file" data-size="12"></span>
          <span class="sktree-row__name">${escapeHtml(n.name)}</span>
        `;
        row.addEventListener("click", () => {
          currentFile = n.path;
          viewerMode = /\.(md|markdown)$/i.test(n.path) ? "render" : "source";
          loadFile();
          paintTree();
        });
        rows.push(row);
      }
    }
    return rows;
  }

  async function loadFile() {
    if (viewerMode !== "edit") {
      viewerCol.innerHTML = `<div class="skpage__viewer-loading">载入 ${escapeHtml(currentFile)} …</div>`;
    }
    try {
      const content = await api.readSkillFile(basePath, currentFile);
      originalContent = content;
      paintViewer(content);
    } catch (e) {
      viewerCol.innerHTML = `<div class="skpage__viewer-loading">载入失败：${escapeHtml(String(e))}</div>`;
    }
  }

  function paintViewer(content: string) {
    viewerCol.innerHTML = "";
    const isMd = /\.(md|markdown)$/i.test(currentFile);
    const isSkillMd = currentFile === "SKILL.md";

    const bar = document.createElement("div");
    bar.className = "skpage__viewer-bar";
    bar.innerHTML = `
      <span class="skpage__viewer-path" title="${escapeHtml(currentFile)}">${escapeHtml(currentFile)}</span>
      <span style="flex:1"></span>
      ${isMd && viewerMode !== "edit" ? `<button class="btn btn--ghost btn--sm" data-act="mode">${viewerMode === "render" ? "源码" : "渲染"}</button>` : ""}
      ${managed && isSkillMd && viewerMode !== "edit" ? `<button class="btn btn--ghost btn--sm" data-act="edit"><span data-icon="edit" data-size="12"></span>编辑</button>` : ""}
      ${viewerMode === "edit" ? `
        <button class="btn btn--ghost btn--sm" data-act="cancel">放弃</button>
        <button class="btn btn--primary btn--sm" data-act="save"><span data-icon="save" data-size="12"></span>保存</button>` : ""}
    `;
    viewerCol.append(bar);

    const modeBtn = bar.querySelector("[data-act='mode']");
    modeBtn?.addEventListener("click", () => {
      viewerMode = viewerMode === "render" ? "source" : "render";
      paintViewer(originalContent);
    });
    bar.querySelector("[data-act='edit']")?.addEventListener("click", () => {
      viewerMode = "edit";
      paintViewer(originalContent);
    });
    bar.querySelector("[data-act='cancel']")?.addEventListener("click", () => {
      viewerMode = isMd ? "render" : "source";
      paintViewer(originalContent);
    });
    bar.querySelector("[data-act='save']")?.addEventListener("click", async () => {
      const ta = viewerCol.querySelector("textarea")!;
      try {
        await api.writeSkillMd(skillRef.id, ta.value);
        toast("已保存", "success");
        viewerMode = "render";
        await store.refresh();
        await loadFile();
      } catch (e) {
        toast(`保存失败：${e}`, "error");
      }
    });

    if (viewerMode === "edit") {
      const ta = document.createElement("textarea");
      ta.className = "skill-md-editor skpage__editor";
      ta.value = content;
      viewerCol.append(ta);
      return;
    }

    if (isMd && viewerMode === "render") {
      const pane = document.createElement("div");
      pane.className = "skill-md skpage__md";
      if (!content.trim()) {
        pane.innerHTML = `<div style="color:var(--ink-mute);font-style:italic">（空文件）</div>`;
      } else {
        const html = marked.parse(content, { async: false }) as string;
        pane.innerHTML = html;
      }
      viewerCol.append(pane);
    } else {
      const pre = document.createElement("pre");
      pre.className = "skpage__code";
      pre.textContent = content || "（空文件）";
      viewerCol.append(pre);
    }
  }

  // ---- 启动 + store 订阅（只刷新信息区，文件区状态不丢）----
  // ---- 启动 ----
  // 关键：把页面挂到 mount 上（此前漏了这一行，整个页面渲染进了游离节点，
  // 表现为详情页空白且无任何报错）。
  mount.append(view);
  paintInfo();
  if (managed) refreshCells();
  loadTree();
  loadFile();
  paintIcons(view);

  if (managed) {
    const unsub = store.subscribe(paintInfo);
    mount.addEventListener("cleanup", () => unsub(), { once: true });
  } else {
    // 未纳管：异步补齐 frontmatter 名称/描述与大小/修改时间。
    api.localSkillMeta(basePath)
      .then((meta) => {
        localMeta = meta;
        head.querySelector("[data-title-name]")!.textContent = meta.name || fallbackName;
        const descEl = head.querySelector<HTMLElement>("[data-desc]")!;
        if (meta.description) {
          descEl.textContent = meta.description;
          descEl.hidden = false;
        }
        paintInfo();
      })
      .catch(() => { /* 目录读不到：保留回退名，文件区会显示加载失败 */ });
  }
}

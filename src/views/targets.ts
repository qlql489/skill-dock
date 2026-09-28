// Targets view — Agents 页：浏览 + 管理。
//
// 原「设置 → Agents」的管理功能已合并进本页：
//   · 已启用列表可拖拽调整顺序（顺序全局生效）
//   · 行尾开关控制启用/停用；自定义目标（UUID id）可移除
//   · 未启用集合折叠在下方面板，打开开关即启用
//   · 顶部「重新检测」重新扫描本机 Agent
// 排序规则：启用 → 追加到已启用列表末尾；停用 → 移到未启用集合最前
//（持久化顺序里两者是同一个位置：启用/未启用边界位）。
// 点击行仍进入该 Agent 的 skills 列表页；分类 chips 点击带筛选进入。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { ask, open as openDialog } from "@tauri-apps/plugin-dialog";
import { escapeHtml } from "./library";
import { openModal } from "../modal";
import { agentIconMarkup } from "../agent-icons";
import type { AgentTarget, BuiltinDirInfo, PathInsight, SkillLocationKind, DiscoveredAgentSkill } from "../types";

type TargetsView = "list" | "cards";
const VIEW_KEY = "skill-dock:targets-view";

function loadView(): TargetsView {
  try {
    return localStorage.getItem(VIEW_KEY) === "cards" ? "cards" : "list";
  } catch {
    return "list";
  }
}

const KIND_CHIPS: { kind: SkillLocationKind; label: string; hint: string }[] = [
  { kind: "local", label: "本地", hint: "真实目录，未由 SkillDock 安装" },
  { kind: "managed", label: "已安装", hint: "由 SkillDock 创建的软链接" },
  { kind: "external_symlink", label: "符号链接", hint: "外部创建的软链接" },
];

/** 一个 target 的分类计数。kind 字段缺失时按旧 DTO 的 is_symlink 兜底。 */
interface KindCounts {
  local: number;
  managed: number;
  external_symlink: number;
  total: number;
}

function entryKind(e: DiscoveredAgentSkill): SkillLocationKind {
  if (e.kind === "local" || e.kind === "managed" || e.kind === "external_symlink") return e.kind;
  return e.is_symlink ? "external_symlink" : "local";
}

function aggregate(entries: DiscoveredAgentSkill[]): Map<string, KindCounts> {
  const map = new Map<string, KindCounts>();
  for (const e of entries) {
    if (!e.has_skill_md) continue;
    const c = map.get(e.target_id) ?? { local: 0, managed: 0, external_symlink: 0, total: 0 };
    c[entryKind(e)]++;
    c.total++;
    map.set(e.target_id, c);
  }
  return map;
}

/** 行内「+N」额外地址徽章：tooltip 列出全部额外目录。 */
function extrasBadge(tgt: AgentTarget): string {
  const n = tgt.extra_dirs?.length ?? 0;
  if (!n) return "";
  const list = tgt.extra_dirs.map((d) => tildePath(d)).join("\n");
  return ` <span class="tgt-extra-badge" title="额外技能地址：\n${escapeHtml(list)}">+${n}</span>`;
}

// 内置目标的 id 都是 kebab-case 字符串；用户手加的自定义目标是 UUID。
const isCustomTarget = (t: AgentTarget) => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}/i.test(t.id);

/**
 * 启停并按规则归位：启用 → 已启用块末尾；停用 → 未启用块最前。
 * 两者的目标位置相同（启用/未启用边界位），所以用同一个顺序公式：
 * [其余已启用(原相对顺序), 该 agent, 其余未启用(原相对顺序)]。
 */
async function setTargetEnabled(id: string, enabled: boolean): Promise<void> {
  const targets = store.get().targets;
  const enabledIds = targets.filter((t) => t.enabled && t.id !== id).map((t) => t.id);
  const disabledIds = targets.filter((t) => !t.enabled && t.id !== id).map((t) => t.id);
  await api.updateTarget(id, enabled, null, null, null, null, null);
  await api.reorderTargets([...enabledIds, id, ...disabledIds]);
  await store.refresh();
}

/** 已启用列表内拖拽落点：重排已启用块，未启用块整体保持在其后。 */
async function reorderEnabled(orderedEnabledIds: string[]): Promise<void> {
  const disabledIds = store.get().targets.filter((t) => !t.enabled).map((t) => t.id);
  await api.reorderTargets([...orderedEnabledIds, ...disabledIds]);
  await store.refresh();
}

/** 内置 agent 出厂目录清单（弹窗提示 + 重置判定用），进程内缓存。 */
let builtinDirsCache: BuiltinDirInfo[] | null = null;
async function getBuiltinDirs(): Promise<BuiltinDirInfo[]> {
  return (builtinDirsCache ??= await api.classifyBuiltinDirs());
}

/** 宽松比较：去尾斜杠后的路径字符串相等（仅用于「是否仍是内置默认」的展示判定）。 */
const looseEq = (a: string, b: string) => a.replace(/\/+$/, "") === b.replace(/\/+$/, "");

/**
 * 添加（existing=null）/编辑 Agent 弹窗。
 *
 * 目录规则（后端同样强校验，唯一性覆盖所有 agent 的默认目录 + 额外地址）：
 *   · 默认目录：新安装都落这里；内置 agent 漂移后可一键重置回内置值
 *   · 额外地址：可多个，该 agent 也会从这些目录读技能，但不接收安装
 * 输入实时调用 classify_target_path：识别归属 agent、统计技能数、查重。
 */
function openTargetDialog(existing: AgentTarget | null): void {
  const body = document.createElement("div");
  body.className = "tgt-form";
  body.innerHTML = `
    <div class="field">
      <label class="field__label">名称</label>
      <input class="input" data-tf-name placeholder="如 My Agent" />
    </div>
    <div class="field">
      <label class="field__label">默认目录 <span class="tgt-form__sub">新安装的技能都落在这里</span></label>
      <div class="row" style="gap:6px">
        <input class="input" data-tf-dir placeholder="~/.myagent/skills" />
        <button type="button" class="btn btn--ghost" data-tf-browse title="选择文件夹"><span data-icon="folder" data-size="14"></span></button>
      </div>
      <div class="tgt-form__insight" data-tf-insight hidden></div>
      <div class="tgt-form__reset" data-tf-reset-row hidden>
        <span data-tf-reset-hint></span>
        <button type="button" class="btn btn--ghost btn--sm" data-tf-reset>重置为默认</button>
      </div>
    </div>
    <div class="field">
      <label class="field__label">描述</label>
      <input class="input" data-tf-desc placeholder="可选" />
    </div>
    <div class="field">
      <label class="field__label">额外技能地址 <span class="tgt-form__sub">该 Agent 也会从这些目录读技能（不接收安装）</span></label>
      <div class="tgt-form__extras" data-tf-extras></div>
      <div class="row" style="gap:6px">
        <input class="input" data-tf-extra-input placeholder="再添加一个技能目录…" />
        <button type="button" class="btn btn--ghost" data-tf-extra-add>添加</button>
      </div>
      <div class="tgt-form__insight" data-tf-extra-insight hidden></div>
    </div>
  `;
  const nameInput = body.querySelector<HTMLInputElement>("[data-tf-name]")!;
  const dirInput = body.querySelector<HTMLInputElement>("[data-tf-dir]")!;
  const descInput = body.querySelector<HTMLInputElement>("[data-tf-desc]")!;
  const insightEl = body.querySelector<HTMLElement>("[data-tf-insight]")!;
  const resetRow = body.querySelector<HTMLElement>("[data-tf-reset-row]")!;
  const resetHint = body.querySelector<HTMLElement>("[data-tf-reset-hint]")!;
  const extrasHost = body.querySelector<HTMLElement>("[data-tf-extras]")!;
  const extraInput = body.querySelector<HTMLInputElement>("[data-tf-extra-input]")!;
  const extraInsight = body.querySelector<HTMLElement>("[data-tf-extra-insight]")!;
  const extraAddBtn = body.querySelector<HTMLButtonElement>("[data-tf-extra-add]")!;
  if (existing) {
    nameInput.value = existing.name;
    dirInput.value = existing.skills_dir;
    descInput.value = existing.description;
  }

  const footer = document.createElement("div");
  footer.className = "row";
  footer.style.cssText = "justify-content:flex-end;gap:8px;margin-top:14px";
  footer.innerHTML = `
    <button class="btn btn--ghost" data-tf-cancel>取消</button>
    <button class="btn btn--primary" data-tf-save>保存</button>
  `;
  const saveBtn = footer.querySelector<HTMLButtonElement>("[data-tf-save]")!;

  /** 额外地址的工作副本（保存时整表提交后端）。 */
  const extras: string[] = existing ? [...(existing.extra_dirs ?? [])] : [];

  function updateSaveState(): void {
    saveBtn.disabled = !nameInput.value.trim() || !dirInput.value.trim();
  }

  /** 一条识别结果的渲染；kind 决定 own_default 在默认目录里中性、在额外地址里报错。 */
  function renderInsightInto(el: HTMLElement, ins: PathInsight | null, kind: "default" | "extra"): void {
    if (!ins) {
      el.hidden = true;
      el.innerHTML = "";
      return;
    }
    el.hidden = false;
    if (ins.used_by) {
      el.className = "tgt-form__insight is-bad";
      el.innerHTML = `<span data-icon="alert" data-size="14"></span><span>该目录已被「${escapeHtml(ins.used_by)}」使用 —— 多个 Agent 不能共用一个技能目录。</span>`;
      paintIcons(el);
      return;
    }
    if (ins.own_default && kind === "extra") {
      el.className = "tgt-form__insight is-bad";
      el.innerHTML = `<span data-icon="alert" data-size="14"></span><span>这是该 Agent 的默认目录，不能再添加为额外地址。</span>`;
      paintIcons(el);
      return;
    }
    const icon = ins.matched_id ? agentIconMarkup(ins.matched_id, ins.matched_name ?? "", "agent-icon--row") : "";
    const bits: string[] = [];
    if (ins.matched_name) {
      bits.push(ins.matched_how === "exact"
        ? `识别为 <b>${escapeHtml(ins.matched_name)}</b> 的技能目录`
        : `按目录特征推测为 <b>${escapeHtml(ins.matched_name)}</b>（自定义位置）`);
    } else {
      bits.push("未识别的目录，将作为自定义位置保存");
    }
    if (ins.skill_count !== null) bits.push(`发现 ${ins.skill_count} 个技能`);
    else if (!ins.exists) bits.push("目录不存在，保存时自动创建");
    else if (!ins.is_dir) bits.push("该路径不是目录");
    const good = !!ins.matched_name || (ins.skill_count ?? 0) > 0;
    el.className = `tgt-form__insight ${good ? "is-ok" : "is-warn"}`;
    el.innerHTML = `${icon}<span>${bits.join(" · ")}</span>`;
    paintIcons(el);
  }

  /** 输入防抖 → classify_target_path → 渲染识别条（只接受最新一次结果）。 */
  function wireClassify(input: HTMLInputElement, el: HTMLElement, kind: "default" | "extra", onResult?: (ins: PathInsight | null) => void): void {
    let timer: number | undefined;
    let seq = 0;
    input.addEventListener("input", () => {
      clearTimeout(timer);
      timer = window.setTimeout(async () => {
        const raw = input.value.trim();
        if (!raw) {
          seq++;
          renderInsightInto(el, null, kind);
          onResult?.(null);
          return;
        }
        const mine = ++seq;
        el.hidden = false;
        el.className = "tgt-form__insight";
        el.textContent = "识别中…";
        try {
          const ins = await api.classifyTargetPath(raw, existing?.id ?? null);
          if (mine !== seq) return;
          renderInsightInto(el, ins, kind);
          onResult?.(ins);
          // 新增时默认目录识别成功且名称/描述还空着 → 自动填充
          if (!existing && kind === "default" && ins.matched_name) {
            if (!nameInput.value.trim()) nameInput.value = ins.matched_name;
            if (!descInput.value.trim() && ins.matched_desc) descInput.value = ins.matched_desc;
          }
        } catch {
          if (mine === seq) {
            renderInsightInto(el, null, kind);
            onResult?.(null);
          }
        }
      }, 350);
    });
  }

  // ---- 额外地址列表 ----
  let lastExtraInsight: PathInsight | null = null;
  function renderExtras(): void {
    extrasHost.innerHTML = "";
    extras.forEach((dir, i) => {
      const row = document.createElement("div");
      row.className = "tgt-form__extra-row";
      row.innerHTML = `
        <span class="tgt-form__extra-path" title="${escapeHtml(dir)}">${escapeHtml(tildePath(dir))}</span>
        <button type="button" class="btn btn--ghost btn--icon" title="移除这个地址（目录内容不受影响）"><span data-icon="close" data-size="12"></span></button>
      `;
      row.querySelector("button")!.addEventListener("click", () => {
        extras.splice(i, 1);
        renderExtras();
      });
      extrasHost.append(row);
    });
    paintIcons(extrasHost);
  }
  function updateExtraAddState(): void {
    const raw = extraInput.value.trim();
    const bad = !raw
      || !!lastExtraInsight?.used_by
      || (!!lastExtraInsight?.own_default)
      || extras.some((d) => looseEq(d, raw));
    extraAddBtn.disabled = bad;
  }
  wireClassify(extraInput, extraInsight, "extra", (ins) => {
    lastExtraInsight = ins;
    updateExtraAddState();
  });
  extraInput.addEventListener("input", updateExtraAddState);
  extraAddBtn.addEventListener("click", () => {
    const raw = extraInput.value.trim();
    if (!raw || lastExtraInsight?.used_by || lastExtraInsight?.own_default) return;
    if (extras.some((d) => looseEq(d, raw))) return;
    extras.push(raw);
    extraInput.value = "";
    lastExtraInsight = null;
    renderInsightInto(extraInsight, null, "extra");
    renderExtras();
    updateExtraAddState();
  });

  // ---- 默认目录识别 + 内置重置提示 ----
  let lastDefaultInsight: PathInsight | null = null;
  wireClassify(dirInput, insightEl, "default", (ins) => { lastDefaultInsight = ins; });
  body.querySelector("[data-tf-browse]")!.addEventListener("click", async () => {
    try {
      const sel = await openDialog({ directory: true, multiple: false, title: "选择技能目录" });
      const picked = Array.isArray(sel) ? sel[0] : sel;
      if (picked) {
        dirInput.value = picked;
        dirInput.dispatchEvent(new Event("input"));
      }
    } catch { /* 用户取消 */ }
  });
  if (existing) {
    getBuiltinDirs().then((list) => {
      const b = list.find((x) => x.id === existing.id);
      if (!b || looseEq(existing.skills_dir, b.dir)) return;
      resetRow.hidden = false;
      resetHint.textContent = `内置默认：${tildePath(b.dir)}`;
    }).catch(() => { /* 提示拿不到就算了 */ });
  }
  const resetBtn = body.querySelector<HTMLButtonElement>("[data-tf-reset]")!;
  resetBtn?.addEventListener("click", async () => {
    if (!existing) return;
    resetBtn.disabled = true;
    try {
      const r = await api.resetTargetDir(existing.id);
      await store.refresh();
      toast(`默认目录已重置${r.moved_links ? `，迁移了 ${r.moved_links} 个技能链接` : ""}`, "success");
      modal.close();
    } catch (error) {
      toast(`重置失败：${error}`, "error");
      resetBtn.disabled = false;
    }
  });

  footer.querySelector("[data-tf-cancel]")!.addEventListener("click", () => modal.close());
  saveBtn.addEventListener("click", async () => {
    const name = nameInput.value.trim();
    const dir = dirInput.value.trim();
    if (!name || !dir) return;
    saveBtn.disabled = true;
    try {
      if (existing) {
        const r = await api.updateTarget(existing.id, null, name, dir, extras, descInput.value.trim(), null);
        await store.refresh();
        const bits = ["已更新"];
        if (r.moved_links > 0) bits.push(`迁移了 ${r.moved_links} 个技能链接`);
        else if (r.left_links > 0) bits.push(`${r.left_links} 个条目未搬动（旧位置有非软链内容），台账已改指新目录`);
        if (r.dropped_links > 0) bits.push(`${r.dropped_links} 个被移除地址的安装记录已清（磁盘未动）`);
        toast(bits.join("，"), "success");
      } else {
        const tag = lastDefaultInsight?.matched_tag ?? null;
        const desc = descInput.value.trim() || lastDefaultInsight?.matched_desc || "";
        const created = await api.addTarget(name, dir, desc, tag);
        if (extras.length > 0) {
          await api.updateTarget(created.id, null, null, null, extras, null, null);
        }
        await store.refresh();
        toast(`已添加「${name}」`, "success");
      }
      modal.close();
    } catch (error) {
      toast(`${existing ? "保存" : "添加"}失败：${error}`, "error");
      saveBtn.disabled = false;
    }
  });

  const modal = openModal({ title: existing ? "编辑 Agent" : "添加 Agent", body, footer, width: 460 });
  renderExtras();
  updateSaveState();
  updateExtraAddState();
  if (existing) dirInput.dispatchEvent(new Event("input")); // 编辑态立即识别当前默认目录
  nameInput.focus();
}

export function renderTargets(mount: HTMLElement): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  mount.append(view);

  let viewMode: TargetsView = loadView();
  // 停用操作后自动展开「未启用」面板，让用户看到它落到最上面。
  let openDisabledPanel = false;

  // 计数只在本页进入时扫一次磁盘；store 重绘复用缓存，不重复扫描。
  let counts: Map<string, KindCounts> | null = null;
  api.discoverAgentSkills()
    .then((entries) => {
      counts = aggregate(entries);
      paint();
    })
    .catch(() => {
      counts = new Map();
      paint();
    });

  function paint() {
    const snap = store.get();
    view.innerHTML = "";

    // ---- 头部：管理操作 + 视图切换 ----
    const head = document.createElement("div");
    head.className = "view-head";
    head.innerHTML = `
      <div></div>
      <div class="view-head__actions">
        <button class="btn btn--ghost btn--sm" data-add-target><span data-icon="plus" data-size="14"></span>添加 Agent</button>
        <button class="btn btn--ghost btn--sm" data-redetect><span data-icon="refresh" data-size="14"></span>重新检测</button>
        <button class="btn btn--ghost btn--sm" data-discover><span data-icon="search" data-size="14"></span>本机skill管理</button>
        <div class="view-icon-toggle" role="group" aria-label="显示模式">
          <button data-mode="list" title="列表视图"><span data-icon="list" data-size="15"></span></button>
          <button data-mode="cards" title="卡片视图"><span data-icon="grid" data-size="15"></span></button>
        </div>
      </div>
    `;
    for (const b of head.querySelectorAll<HTMLButtonElement>("[data-mode]")) {
      if (b.dataset.mode === viewMode) b.setAttribute("data-active", "true");
      else b.removeAttribute("data-active");
      b.addEventListener("click", () => {
        viewMode = b.dataset.mode as TargetsView;
        try { localStorage.setItem(VIEW_KEY, viewMode); } catch { /* ignore */ }
        paint();
      });
    }
    head.querySelector<HTMLButtonElement>("[data-add-target]")?.addEventListener("click", () => {
      openTargetDialog(null);
    });
    head.querySelector<HTMLButtonElement>("[data-redetect]")?.addEventListener("click", async (event) => {
      const button = event.currentTarget as HTMLButtonElement;
      button.disabled = true;
      try {
        const count = await api.redetectTargets();
        await store.refresh();
        toast(`检测到 ${count} 个 Agent`, "success");
      } catch (error) {
        toast(`检测失败：${error}`, "error");
      } finally {
        button.disabled = false;
      }
    });
    head.querySelector<HTMLButtonElement>("[data-discover]")?.addEventListener("click", () => {
      location.hash = "#/discover";
    });
    view.append(head);

    const enabledTargets = snap.targets.filter((t) => t.enabled);
    const disabledTargets = snap.targets.filter((t) => !t.enabled);

    // ---- 已启用区块 ----
    const enabledSection = document.createElement("section");
    enabledSection.className = "targets-section";
    enabledSection.innerHTML = `
      <div class="settings-group__eyebrow targets-section__label">已启用 · ${enabledTargets.length}${viewMode === "list" && enabledTargets.length > 1 ? " · 可拖拽排序" : ""}</div>
    `;
    if (enabledTargets.length === 0) {
      enabledSection.insertAdjacentHTML("beforeend", `
        <div class="targets-section__hint">没有启用的 Agent —— 在下方「未启用」面板里打开开关即可启用。</div>
      `);
    } else {
      const host = document.createElement("div");
      host.className = viewMode === "cards" ? "tcard-grid" : "src-list";
      enabledTargets.forEach((t, i) => host.append(renderTarget(t, i, viewMode === "list")));
      enabledSection.append(host);
    }
    view.append(enabledSection);

    // ---- 未启用面板（折叠）----
    if (disabledTargets.length > 0) {
      const panel = document.createElement("details");
      panel.className = "settings-agent-unavailable targets-section";
      panel.open = openDisabledPanel;
      panel.innerHTML = `
        <summary class="settings-group__head settings-agent-unavailable__summary">
          <div>
            <div class="settings-group__eyebrow">未启用 · ${disabledTargets.length}</div>
            <h3>未启用的 Agent</h3>
          </div>
          <span class="settings-agent-unavailable__chevron" data-icon="chevronRight" data-size="16"></span>
        </summary>
      `;
      const list = document.createElement("div");
      list.className = "src-list";
      disabledTargets.forEach((t) => list.append(renderDisabledRow(t)));
      panel.append(list);
      view.append(panel);
    }

    wireSwitches();
    if (viewMode === "list") wireDrag();
    paintIcons(view);
  }

  // ------------------------------------------------------------------
  // 启停开关（已启用/未启用两个区块共用）
  // ------------------------------------------------------------------
  function wireSwitches(): void {
    view.querySelectorAll<HTMLButtonElement>("[data-toggle]").forEach((button) => {
      button.addEventListener("click", async (event) => {
        event.stopPropagation();
        const id = button.dataset.toggle!;
        const target = store.get().targets.find((t) => t.id === id);
        if (!target) return;
        button.disabled = true;
        try {
          // 停用 → 展开未启用面板，让用户看到它落到最上面。
          if (target.enabled) openDisabledPanel = true;
          await setTargetEnabled(id, !target.enabled);
        } catch (error) {
          toast(`切换失败：${error}`, "error");
          button.disabled = false;
        }
      });
    });
    // 编辑 Agent（名称/路径/描述）—— 启用与未启用行共用
    view.querySelectorAll<HTMLButtonElement>("[data-edit]").forEach((button) => {
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        const target = store.get().targets.find((t) => t.id === button.dataset.edit);
        if (target) openTargetDialog(target);
      });
    });
    // 移除自定义目标（目录内容不受影响）
    view.querySelectorAll<HTMLButtonElement>("[data-remove]").forEach((button) => {
      button.addEventListener("click", async (event) => {
        event.stopPropagation();
        const id = button.dataset.remove!;
        const target = store.get().targets.find((t) => t.id === id);
        if (!target) return;
        if (!(await ask(`移除自定义目标「${target.name}」？目录内容不受影响。`, {
          title: "移除自定义目标",
          kind: "warning",
          okLabel: "移除",
          cancelLabel: "取消",
        }))) return;
        try {
          await api.removeTarget(id);
          await store.refresh();
          toast("已移除", "success");
        } catch (error) {
          toast(`移除失败：${error}`, "error");
        }
      });
    });
  }

  // ------------------------------------------------------------------
  // 拖拽排序（仅已启用列表的列表视图）
  // ------------------------------------------------------------------
  function wireDrag(): void {
    let dragId: string | null = null;
    view.querySelectorAll<HTMLElement>(".src-row--agent[data-tid][draggable]").forEach((row) => {
      row.addEventListener("dragstart", (e) => {
        dragId = row.dataset.tid!;
        // WebKit 对空 dataTransfer 的拖拽会取消，必须带数据才能拖起来。
        e.dataTransfer?.setData("text/plain", dragId);
        if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
        row.classList.add("is-dragging");
      });
      row.addEventListener("dragend", () => {
        dragId = null;
        row.classList.remove("is-dragging");
        view.querySelectorAll(".drag-before, .drag-after").forEach((n) => n.classList.remove("drag-before", "drag-after"));
      });
      row.addEventListener("dragover", (e) => {
        if (!dragId || dragId === row.dataset.tid) return;
        e.preventDefault();
        const rect = row.getBoundingClientRect();
        const before = e.clientY < rect.top + rect.height / 2;
        row.classList.toggle("drag-before", before);
        row.classList.toggle("drag-after", !before);
      });
      row.addEventListener("dragleave", () => row.classList.remove("drag-before", "drag-after"));
      row.addEventListener("drop", async (e) => {
        e.preventDefault();
        const before = row.classList.contains("drag-before");
        row.classList.remove("drag-before", "drag-after");
        if (!dragId || dragId === row.dataset.tid) return;
        const dragged = dragId;
        dragId = null;
        const ids = store.get().targets.filter((t) => t.enabled).map((t) => t.id);
        ids.splice(ids.indexOf(dragged), 1);
        const at = ids.indexOf(row.dataset.tid!);
        ids.splice(before ? at : at + 1, 0, dragged);
        try {
          await reorderEnabled(ids);
        } catch (error) {
          toast(`排序失败：${error}`, "error");
        }
      });
    });
  }

  // ------------------------------------------------------------------
  // 分类统计 chips：既是计数展示也是入口，点击进入该 Agent 并预置筛选。
  // ------------------------------------------------------------------
  function renderStats(tgt: AgentTarget): HTMLElement {
    const el = document.createElement("div");
    el.className = viewMode === "cards" ? "tcard__stats" : "src-row__stats";
    const c = counts?.get(tgt.id);
    el.innerHTML = KIND_CHIPS.map(({ kind, label, hint }) => {
      const n = c ? c[kind] : null;
      const title = c ? `${label}：${hint}` : "扫描中…";
      const zero = n === 0;
      return `
        <button class="stat-chip stat-chip--${kind}${zero ? " stat-chip--zero" : ""}"
                data-kind="${kind}" data-target="${escapeHtml(tgt.id)}"
                title="${escapeHtml(title)}" ${n === null || zero ? "disabled" : ""}>
          <span class="stat-chip__dot"></span>${label}
          <span class="stat-chip__num">${n ?? "·"}</span>
        </button>`;
    }).join("");
    for (const b of el.querySelectorAll<HTMLButtonElement>(".stat-chip:not([disabled])")) {
      b.addEventListener("click", (ev) => {
        ev.stopPropagation();
        location.hash = `#/agent-skills/${encodeURIComponent(b.dataset.target!)}?kind=${b.dataset.kind}`;
      });
    }
    return el;
  }

  // ------------------------------------------------------------------
  // 已启用目标：列表行（可拖拽 + 开关）/ 卡片（开关）—— 点击进入列表页
  // ------------------------------------------------------------------
  function renderTarget(tgt: AgentTarget, i: number, draggable: boolean): HTMLElement {
    const isCard = viewMode === "cards";
    const el = document.createElement("div");
    el.className = (isCard ? "tcard" : "src-row src-row--agent") + (draggable ? "" : " src-row--static");
    el.dataset.tid = tgt.id;
    if (!isCard && draggable) el.draggable = true;
    el.style.cursor = "pointer";

    const tools = document.createElement("div");
    tools.className = isCard ? "tcard__tools" : "src-row__actions";
    tools.innerHTML = `
      <button class="switch ${tgt.enabled ? "is-on" : ""}" data-toggle="${escapeHtml(tgt.id)}"
              role="switch" aria-checked="${tgt.enabled}" title="${tgt.enabled ? "停用" : "启用"} ${escapeHtml(tgt.name)}">
        <span class="switch__thumb"></span>
      </button>
      <button class="btn btn--ghost btn--icon" data-edit="${escapeHtml(tgt.id)}" title="编辑名称与技能目录"><span data-icon="edit" data-size="13"></span></button>
      ${isCustomTarget(tgt) ? `<button class="btn btn--ghost btn--icon" data-remove="${escapeHtml(tgt.id)}" title="移除这个自定义目标"><span data-icon="delete" data-size="13"></span></button>` : ""}
    `;

    if (isCard) {
      const headEl = document.createElement("div");
      headEl.className = "tcard__head";
      headEl.innerHTML = `<h3 class="tcard__name">${agentIconMarkup(tgt.id, tgt.name, "agent-icon--row")}<span>${escapeHtml(tgt.name)}</span></h3>`;
      headEl.append(tools);
      el.append(headEl);
      const path = document.createElement("div");
      path.className = "tcard__path";
      path.title = tgt.skills_dir;
      path.innerHTML = `${escapeHtml(tildePath(tgt.skills_dir))}${extrasBadge(tgt)}`;
      el.append(path);
      el.append(renderStats(tgt));
    } else {
      el.innerHTML = `
        <span class="src-row__grip" ${draggable ? `data-icon="sort" data-size="13" title="拖拽排序"` : ""}></span>
        <div class="src-row__idx">${String(i + 1).padStart(2, "0")}</div>
        <div class="src-row__main">
          <h3 class="src-row__name src-row__name--agent">${agentIconMarkup(tgt.id, tgt.name, "agent-icon--row")}<span>${escapeHtml(tgt.name)}</span></h3>
          <div class="src-row__path">${escapeHtml(tildePath(tgt.skills_dir))}${extrasBadge(tgt)}</div>
        </div>
      `;
      el.append(renderStats(tgt));
      el.append(tools);
    }

    el.addEventListener("click", () => {
      location.hash = `#/agent-skills/${encodeURIComponent(tgt.id)}`;
    });
    return el;
  }

  // ------------------------------------------------------------------
  // 未启用面板里的行：无拖拽、无计数扫描开销，开关启用后落到已启用末尾
  // ------------------------------------------------------------------
  function renderDisabledRow(tgt: AgentTarget): HTMLElement {
    const el = document.createElement("div");
    el.className = "src-row src-row--agent src-row--disabled-row";
    el.dataset.tid = tgt.id;
    el.innerHTML = `
      <span class="src-row__grip"></span>
      <div class="src-row__idx"></div>
      <div class="src-row__main">
        <h3 class="src-row__name src-row__name--agent">${agentIconMarkup(tgt.id, tgt.name, "agent-icon--row")}<span>${escapeHtml(tgt.name)}</span></h3>
        <div class="src-row__path">${escapeHtml(tildePath(tgt.skills_dir))}${extrasBadge(tgt)}</div>
      </div>
      <div class="src-row__actions">
        <span class="settings-agent-row__state is-muted">${tgt.detected ? "已停用" : "未安装"}</span>
        <button class="switch" data-toggle="${escapeHtml(tgt.id)}" role="switch" aria-checked="false" title="启用 ${escapeHtml(tgt.name)}">
          <span class="switch__thumb"></span>
        </button>
        <button class="btn btn--ghost btn--icon" data-edit="${escapeHtml(tgt.id)}" title="编辑名称与技能目录"><span data-icon="edit" data-size="13"></span></button>
        ${isCustomTarget(tgt) ? `<button class="btn btn--ghost btn--icon" data-remove="${escapeHtml(tgt.id)}" title="移除这个自定义目标"><span data-icon="delete" data-size="13"></span></button>` : ""}
      </div>
    `;
    return el;
  }

  paint();
  const unsub = store.subscribe(paint);
  mount.addEventListener("cleanup", () => unsub(), { once: true });
}

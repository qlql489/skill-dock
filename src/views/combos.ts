// 组合 —— 命名的 skill 集合，支持整批应用到多个 agent / 一键停用。
//
// UI 说明：与库页面同风格的水平胶囊条（放置于库页顶部），点击胶囊=过滤
// 库到该组合；胶囊右侧的 ⚙ 打开编辑弹窗。编辑弹窗两个 tab：「成员」勾选
// 组合里有哪些 skill；「应用到 Agents」勾选装到哪些 agent（带四态预览），
// 保存时勾选的补齐安装、取消勾选的移除链接。配色按 id 稳定散列。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { ask } from "@tauri-apps/plugin-dialog";
import { openModal } from "../modal";
import { agentIconMarkup, paintAgentIcons } from "../agent-icons";
import { escapeHtml } from "./library";

/** 由 id 稳定生成一个 HSL 色相。 */
function hueFor(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) % 360;
  return h;
}

function comboDot(id: string): string {
  return `<span class="combo-dot" style="background:hsl(${hueFor(id)} 62% 52%)"></span>`;
}

// ---------------------------------------------------------------------------
// 顶部组合条
// ---------------------------------------------------------------------------

export interface StripOptions {
  /** 当前过滤的组合 id；null = 不过滤。 */
  activeId: string | null;
  onSelect: (id: string | null) => void;
}

/** 把组合条画进 host。host 内部完全受管（每次重画）。 */
export function renderComboStrip(host: HTMLElement, opts: StripOptions): void {
  // `?? []` 兜底：后端二进制较旧缺少 groups 字段时页面不能整体挂掉。
  const groups = (store.get().groups ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
  host.innerHTML = "";

  const bar = document.createElement("div");
  bar.className = "combo-bar";

  const mkPill = (label: string, groupId: string | null, count?: number) => {
    // div+role=button 而非原生 button：胶囊内还嵌着 ⚙ 子按钮（见下），
    // 原生 button 嵌套会把内层元素的点击/AXPress 全部路由到外层。
    const p = document.createElement("div");
    p.className = "combo-pill";
    p.setAttribute("role", "button");
    p.tabIndex = 0;
    if ((opts.activeId ?? null) === groupId) p.setAttribute("data-active", "true");
    p.innerHTML = `
      ${groupId ? comboDot(groupId) : ""}
      <span>${escapeHtml(label)}</span>
      ${count != null ? `<span class="combo-pill__count">${count}</span>` : ""}
    `;
    p.addEventListener("click", () => opts.onSelect(groupId));
    p.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        (p as HTMLElement).click();
      }
    });
    return p;
  };

  bar.append(mkPill("全部", null));
  for (const g of groups) {
    // ⚙ 必须是筛选胶囊的「兄弟」而不是子元素：role=button 嵌套会被 AX
    // 折叠成一个节点（内层永远点不到，键盘也不可达）。
    const wrap = document.createElement("span");
    wrap.className = "combo-pill-group";

    const pill = document.createElement("div");
    pill.className = "combo-pill";
    pill.setAttribute("role", "button");
    pill.tabIndex = 0;
    if (opts.activeId === g.id) pill.setAttribute("data-active", "true");
    pill.title = "点击按此组合过滤";
    pill.innerHTML = `
      ${comboDot(g.id)}
      <span>${escapeHtml(g.name)}</span>
      <span class="combo-pill__count">${g.skill_ids.length}</span>
    `;
    pill.addEventListener("click", () => opts.onSelect(g.id));
    pill.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        opts.onSelect(g.id);
      }
    });

    const gear = document.createElement("div");
    gear.className = "combo-pill combo-pill--gear";
    gear.setAttribute("role", "button");
    gear.tabIndex = 0;
    gear.title = "编辑成员与应用 Agents";
    gear.innerHTML = `<span data-icon="settings" data-size="11"></span>`;
    gear.addEventListener("click", () => openComboEditor(g.id));
    gear.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openComboEditor(g.id);
      }
    });

    wrap.append(pill, gear);
    bar.append(wrap);
  }

  const addBtn = document.createElement("button");
  addBtn.className = "combo-pill combo-pill--add";
  addBtn.innerHTML = `<span data-icon="plus" data-size="12"></span><span>新建组合</span>`;
  addBtn.addEventListener("click", () => openComboEditor(null));
  bar.append(addBtn);

  host.append(bar);
  paintIcons(host);
}

// ---------------------------------------------------------------------------
// 编辑器：成员 / 应用到 Agents 两个 tab
// ---------------------------------------------------------------------------

/** 单个 agent 的预览统计（四态）。 */
interface TargetPreview {
  linked: number;
  stale: number;
  conflict: number;
  problems: { name: string; tag: string; detail: string | null }[];
}

export function openComboEditor(groupId: string | null): void {
  const snap = store.get();
  const group = groupId ? (snap.groups ?? []).find((g) => g.id === groupId) : null;
  const memberIds = new Set(group?.skill_ids ?? []);
  const enabledTargets = snap.targets.filter((t) => t.enabled);

  const body = document.createElement("div");
  body.innerHTML = `
    <div class="field">
      <label class="field__label">名称</label>
      <input class="input" data-name placeholder="例如：前端开发常用" value="${group ? escapeHtml(group.name) : ""}" />
    </div>
    <div class="tabs combo-editor__tabs" data-tabs>
      <button class="tab" data-tab="members" data-active="true">成员</button>
      <button class="tab" data-tab="agents">应用到 Agents</button>
    </div>
    <div data-pane="members">
      <div class="search" style="margin:12px 0 10px">
        <span class="search__icon" data-icon="search" data-size="13"></span>
        <input class="input" data-filter placeholder="筛选 skill…" />
      </div>
      <div class="combo-member-list" data-members></div>
    </div>
    <div data-pane="agents" hidden>
      <p class="field__hint" style="margin:12px 0 8px">
        勾选 = 把组合安装到该 agent；取消勾选 = 停用（移除已装链接）。改动在点「保存」后生效。
      </p>
      <div class="ip-list combo-targets" data-agents></div>
      <div class="combo-preview-sum" data-agents-summary></div>
    </div>
  `;
  const memberList = body.querySelector("[data-members]")! as HTMLElement;
  const agentsList = body.querySelector("[data-agents]")! as HTMLElement;
  const agentsSummary = body.querySelector("[data-agents-summary]")! as HTMLElement;

  // ---- Tab 切换：Agents 页内容按需构建；成员变过才重新拉预览，避免
  //      冲掉用户已改的勾选。 ----
  let agentsBuilt = false;
  let lastPreviewKey: string | null = null;
  body.querySelector("[data-tabs]")!.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-tab]");
    if (!btn) return;
    for (const t of body.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
      if (t === btn) t.setAttribute("data-active", "true");
      else t.removeAttribute("data-active");
    }
    const showAgents = btn.dataset.tab === "agents";
    body.querySelector<HTMLElement>('[data-pane="members"]')!.hidden = showAgents;
    body.querySelector<HTMLElement>('[data-pane="agents"]')!.hidden = !showAgents;
    if (showAgents) {
      if (!agentsBuilt) {
        agentsBuilt = true;
        buildAgentRows();
      }
      const key = [...memberIds].sort().join(",");
      if (lastPreviewKey !== key) refreshAgentPreview();
    }
  });

  function paintMembers(query: string) {
    const q = query.trim().toLowerCase();
    const skills = snap.skills.filter((s) => !q || s.name.toLowerCase().includes(q));
    memberList.innerHTML =
      skills.length === 0
        ? `<div class="empty" style="padding:18px"><p class="empty__sub">没有匹配的 skill</p></div>`
        : "";
    for (const s of skills) {
      const row = document.createElement("label");
      row.className = "combo-member";
      const checked = memberIds.has(s.id);
      row.innerHTML = `
        <input type="checkbox" ${checked ? "checked" : ""} />
        <span class="combo-member__name">${escapeHtml(s.name)}</span>
        <span class="combo-member__src">${escapeHtml(store.get().sources.find((x) => x.id === s.source_id)?.name ?? "")}</span>
      `;
      row.addEventListener("change", () => {
        const now = (row.querySelector("input") as HTMLInputElement).checked;
        if (now) memberIds.add(s.id);
        else memberIds.delete(s.id);
      });
      memberList.append(row);
    }
  }
  paintMembers("");
  body.querySelector<HTMLInputElement>("[data-filter]")!.addEventListener("input", (e) => {
    paintMembers((e.target as HTMLInputElement).value);
  });

  // ---- 应用到 Agents：行 = 勾选 + agent + 当前状态统计 + 可展开问题明细。
  //      预览基于当前（可能尚未保存的）成员勾选。 ----
  const hasLinks = new Set<string>(); // 最近一次预览里已有链接的 agent
  let previewToken = 0;
  let lastBadAgents = 0;

  function buildAgentRows() {
    agentsList.innerHTML = "";
    if (enabledTargets.length === 0) {
      agentsList.innerHTML = `<div class="empty" style="padding:18px"><p class="empty__sub">还没有启用的 agent —— 先到 agents 页启用。</p></div>`;
      agentsSummary.textContent = "";
      return;
    }
    for (const t of enabledTargets) {
      const item = document.createElement("div");
      item.className = "ip-row combo-target";
      item.dataset.tid = t.id;
      item.innerHTML = `
        <input type="checkbox" class="ip-check" />
        ${agentIconMarkup(t.id, t.name, "agent-icon--row")}
        <div class="ip-row__main">
          <div class="ip-row__name">${escapeHtml(t.name)}</div>
          <div class="ip-row__path">${escapeHtml(tildePath(t.skills_dir))}</div>
        </div>
        <span class="combo-target__stat" data-stat></span>
        <button class="btn btn--ghost btn--icon combo-target__toggle" data-toggle hidden title="展开问题明细"><span data-icon="arrowRight" data-size="12"></span></button>
        <div class="combo-target__cells" data-cells hidden></div>
      `;
      const box = item.querySelector("input") as HTMLInputElement;
      item.addEventListener("click", (e) => {
        if ((e.target as HTMLElement).closest("[data-toggle]")) return;
        box.checked = !box.checked;
        item.setAttribute("data-checked", String(box.checked));
        paintAgentsSummary();
      });
      item.querySelector("[data-toggle]")!.addEventListener("click", (e) => {
        e.stopPropagation();
        const cells = item.querySelector("[data-cells]")! as HTMLElement;
        const opening = cells.hidden;
        cells.hidden = !opening;
        (item.querySelector("[data-toggle] .icon") as HTMLElement)?.style.setProperty(
          "transform", opening ? "rotate(90deg)" : "",
        );
      });
      agentsList.append(item);
    }
    paintIcons(agentsList);
    paintAgentIcons(agentsList);
  }

  async function refreshAgentPreview(): Promise<void> {
    const token = ++previewToken;
    const members = [...memberIds].filter((id) => snap.skills.some((s) => s.id === id));
    lastPreviewKey = [...memberIds].sort().join(",");
    hasLinks.clear();
    agentsSummary.classList.remove("is-warn");

    if (enabledTargets.length === 0) return;

    // 重置所有行的勾选与统计。
    for (const row of agentsList.querySelectorAll<HTMLElement>(".ip-row")) {
      const box = row.querySelector("input") as HTMLInputElement;
      box.disabled = false;
      box.checked = false;
      row.setAttribute("data-checked", "false");
      const stat = row.querySelector<HTMLElement>("[data-stat]")!;
      stat.innerHTML = "…";
      const cells = row.querySelector<HTMLElement>("[data-cells]")!;
      cells.hidden = true;
      cells.innerHTML = "";
      const toggle = row.querySelector<HTMLElement>("[data-toggle]")!;
      toggle.hidden = true;
    }

    if (members.length === 0) {
      for (const row of agentsList.querySelectorAll<HTMLElement>(".ip-row")) {
        (row.querySelector("input") as HTMLInputElement).disabled = true;
        row.querySelector<HTMLElement>("[data-stat]")!.textContent = "组合还没有成员";
      }
      agentsSummary.textContent = "先在「成员」里勾选 skill，再应用到 agent。";
      return;
    }

    try {
      const cells = await api.previewCells(members, enabledTargets.map((t) => t.id));
      if (token !== previewToken) return; // 已有更新的预览，丢弃旧结果
      const byT = new Map<string, TargetPreview>();
      for (const c of cells) {
        const p = byT.get(c.target_id) ?? { linked: 0, stale: 0, conflict: 0, problems: [] };
        if (c.state === "linked") p.linked++;
        else if (c.state === "stale") { p.stale++; p.problems.push({ name: c.skill_name, tag: "失效", detail: c.detail }); }
        else if (c.state !== "absent") { p.conflict++; p.problems.push({ name: c.skill_name, tag: "冲突", detail: c.detail }); }
        byT.set(c.target_id, p);
      }
      lastBadAgents = 0;
      for (const t of enabledTargets) {
        const p = byT.get(t.id) ?? { linked: 0, stale: 0, conflict: 0, problems: [] };
        const row = agentsList.querySelector<HTMLElement>(`.combo-target[data-tid='${t.id}']`);
        if (!row) continue;
        const bad = p.stale + p.conflict;
        if (p.linked > 0 || p.stale > 0) {
          // 已链接或「失效」（记录在但链接指向旧位置）都算应用过：初始勾选，
          // 保存时失效格会随常规安装分区自动修复。
          hasLinks.add(t.id);
          (row.querySelector("input") as HTMLInputElement).checked = true;
          row.setAttribute("data-checked", "true");
        }
        const stat = row.querySelector<HTMLElement>("[data-stat]")!;
        if (p.linked === members.length && bad === 0) {
          stat.innerHTML = `<span class="is-ok">全部已装 ✓</span>`;
        } else {
          const parts: string[] = [];
          if (p.linked > 0) parts.push(`<span class="is-ok">已装 ${p.linked}/${members.length}</span>`);
          else parts.push(`<span style="color:var(--ink-mute)">未安装</span>`);
          if (p.stale) parts.push(`<span class="is-warn2">失效 ${p.stale}</span>`);
          if (p.conflict) parts.push(`<span class="is-bad">冲突 ${p.conflict}</span>`);
          stat.innerHTML = parts.join(" ");
        }
        if (p.problems.length) {
          const cellsEl = row.querySelector<HTMLElement>("[data-cells]")!;
          cellsEl.innerHTML = p.problems.map((pb) =>
            `<div class="combo-cell-row is-bad"><b>${escapeHtml(pb.name)}</b><span class="combo-cell-row__tag">${pb.tag}</span><span>${escapeHtml(pb.detail ?? "")}</span></div>`).join("");
          row.querySelector<HTMLElement>("[data-toggle]")!.hidden = false;
          lastBadAgents++;
        }
      }
      paintAgentsSummary();
    } catch (e) {
      if (token !== previewToken) return;
      agentsSummary.textContent = `预览失败：${e}`;
      agentsSummary.classList.add("is-warn");
    }
  }

  function paintAgentsSummary() {
    if (enabledTargets.length === 0) return;
    const boxes = [...agentsList.querySelectorAll<HTMLInputElement>("input.ip-check")];
    const checked = boxes.filter((b) => b.checked).length;
    agentsSummary.textContent =
      `已勾选 ${checked}/${enabledTargets.length} 个 agent` +
      (lastBadAgents ? ` · ${lastBadAgents} 个 agent 有失效/冲突链接，可展开查看` : "");
  }

  const footer = document.createElement("div");
  footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
  footer.innerHTML = `
    ${group ? `<button class="btn btn--danger" data-act="del" style="margin-right:auto">删除组合</button>` : ""}
    <button class="btn btn--ghost" data-act="cancel">取消</button>
    <button class="btn btn--primary" data-act="save">${group ? "保存" : "创建"}</button>
  `;

  const modal = openModal({ title: group ? "编辑组合" : "新建组合", body, footer, width: 520 });

  footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());
  footer.querySelector("[data-act='del']")?.addEventListener("click", async () => {
    if (!group || !(await ask(`删除组合「${group.name}」？（不影响已安装的 skill）`, {
      title: "删除组合",
      kind: "warning",
      okLabel: "删除",
      cancelLabel: "取消",
    }))) return;
    try {
      await api.deleteGroup(group.id);
      await store.refresh();
      toast("已删除组合", "success");
      modal.close();
    } catch (e) {
      toast(`删除失败：${e}`, "error");
    }
  });
  footer.querySelector("[data-act='save']")!.addEventListener("click", async (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    const name = body.querySelector<HTMLInputElement>("[data-name]")!.value.trim();
    if (!name) return toast("请填写组合名", "error");
    btn.disabled = true;
    try {
      // 1) 成员落库（新建时要先拿到 id 再写成员）。
      let finalId = group?.id ?? null;
      if (group) {
        await api.updateGroup(group.id, name, [...memberIds]);
      } else {
        await api.createGroup(name);
        await store.refresh();
        const created = store.get().groups.find((g) => g.name === name);
        if (!created) throw new Error("创建后找不到组合");
        finalId = created.id;
        if (memberIds.size > 0) await api.updateGroup(finalId, null, [...memberIds]);
      }

      // 2) Agents tab 同步：勾选的补齐安装（失效格自动修复）、取消勾选但
      //    原本有链接的停用。仅当 Agents 页构建过才执行——没看过就不会有
      //    改动。被他人软链占用的格子不进常规安装分区（Fail 会先炸），而是
      //    分到显式替换分区，替换前经用户确认；拒绝则整格跳过不报错。
      let agentNote = "";
      let agentFailures = 0;
      if (agentsBuilt && finalId) {
        const checked = [...agentsList.querySelectorAll<HTMLInputElement>("input.ip-check:not(:disabled)")]
          .filter((b) => b.checked)
          .map((b) => (b.closest(".combo-target") as HTMLElement)?.dataset.tid ?? "")
          .filter(Boolean);
        const toRemove = [...hasLinks].filter((id) => !checked.includes(id));
        const members = [...memberIds].filter((id) => snap.skills.some((s) => s.id === id));

        let install: [string, string][] = [];
        for (const tid of checked) for (const sid of members) install.push([sid, tid]);
        const remove: [string, string][] = [];
        for (const tid of toRemove) for (const sid of members) remove.push([sid, tid]);
        let replace: [string, string][] = [];

        if (install.length > 0) {
          const cells = await api.previewCells(members, checked);
          const occupied = cells.filter((c) => c.state === "occupied");
          if (occupied.length > 0) {
            const seen = new Set<string>();
            const names: string[] = [];
            for (const c of occupied) {
              const k = `${c.skill_name} @ ${c.target_name}`;
              if (!seen.has(k)) { seen.add(k); names.push(k); }
            }
            const list = names.slice(0, 3).join("、") + (names.length > 3 ? ` 等 ${names.length} 处` : "");
            // 占用格不进常规安装分区（Fail 会先炸）：要么显式替换，要么整格跳过。
            // 注意不能用 window.confirm——Tauri WebView 里它不弹框、直接返回 true。
            install = install.filter(([sid, tid]) =>
              !occupied.some((c) => c.skill_id === sid && c.target_id === tid));
            if (await ask(`有 ${occupied.length} 个位置被其他软链占用（${list}）。\n保存将替换这些软链以指向本组合的 skill，继续？`, {
              title: "替换占用软链",
              kind: "warning",
            })) {
              replace = occupied.map((c) => [c.skill_id, c.target_id] as [string, string]);
            }
          }
        }

        if (install.length > 0 || replace.length > 0 || remove.length > 0) {
          const r = await api.applyCells(install, remove, replace);
          agentFailures = r.failures.length;
          const parts: string[] = [];
          if (checked.length > 0) parts.push(`应用 ${checked.length} 个 agent`);
          if (replace.length > 0) parts.push(`替换 ${replace.length} 个占用软链`);
          if (remove.length > 0) parts.push(`停用 ${toRemove.length} 个 agent`);
          agentNote = parts.join("；");
        }
      }

      await store.refresh();
      if (agentsBuilt) refreshAgentPreview(); // 弹窗不关时（有失败）让统计回到真实状态
      if (agentFailures > 0) {
        // 成员已保存；应用失败留在弹窗里，用户可重试保存。
        toast(`已保存，但应用时有 ${agentFailures} 个失败`, "error");
      } else {
        toast(agentNote ? `已保存：${agentNote}` : group ? "已保存" : "已创建组合", "success");
        modal.close();
      }
    } catch (err) {
      toast(`保存失败：${err}`, "error");
    } finally {
      btn.disabled = false;
    }
  });
}

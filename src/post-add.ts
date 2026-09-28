// Post-add picker — after a source is added and scanned, this pops a modal
// that asks how to install the discovered skills: "install all" (batch to
// chosen agents) or "pick later" (manual install from the skills page).
//
// For local sources, "install all" can optionally watch the folder so skills
// added later auto-install (reuses the auto-sync watcher mechanism).
//
// Flow: the add-picker (step 1) calls markPendingPostAdd(sourceId, kind) when
// the user submits the address form. When the `source-scanned` event fires
// (scan done), main.ts calls maybePromptPostAdd(sourceId), which consumes the
// pending marker and opens the modal now that we know the skill count.

import { api } from "./api";
import { store, tildePath } from "./store";
import { openModal } from "./modal";
import { paintIcons } from "./icon";
import { toast } from "./toast";
import { getAddDefaultInstallTargets } from "./prefs";
import { agentIconMarkup, paintAgentIcons } from "./agent-icons";
import { escapeHtml } from "./views/library";

/** Pending post-add sources: sourceId → kind. Kind tells us whether to offer
 *  the watch option (local only). */
const pending = new Map<string, "local" | "github">();

/** Called by the add-picker right after a source is registered. The actual
 *  modal is deferred until the scan completes. */
export function markPendingPostAdd(sourceId: string, kind: "local" | "github"): void {
  pending.set(sourceId, kind);
}

/** Drop a pending marker without prompting — used when a clone fails or the
 *  source is removed before its scan completes. */
export function cancelPendingPostAdd(sourceId: string): void {
  pending.delete(sourceId);
}

/** Called from the `source-scanned` event handler. If the source is one we
 *  just added, pop the post-add config modal. Returns true if a prompt was
 *  shown. */
export async function maybePromptPostAdd(sourceId: string): Promise<boolean> {
  const kind = pending.get(sourceId);
  if (!kind) return false;
  pending.delete(sourceId);
  await store.refresh();
  const src = store.get().sources.find((s) => s.id === sourceId);
  if (!src) return false;
  openPostAddPicker(sourceId, src.name, src.skill_count, kind);
  return true;
}

/** Re-open the step-2 modal for a source that is already registered (the
 *  add-picker's re-add path). Returns false if the source vanished. */
export async function openPostAddForSource(sourceId: string): Promise<boolean> {
  await store.refresh();
  const src = store.get().sources.find((s) => s.id === sourceId);
  if (!src) return false;
  openPostAddPicker(sourceId, src.name, src.skill_count, src.kind);
  return true;
}

/** Step-2 modal: choose how to install the discovered skills. */
function openPostAddPicker(
  sourceId: string,
  name: string,
  skillCount: number,
  kind: "local" | "github",
): void {
  const snap = store.get();
  const canWatch = kind === "local";
  // 开关打开 = 添加 skills 默认安装到全部已启用的 Agents（设置页可关）。
  const precheckAll = getAddDefaultInstallTargets();
  const enabledTargets = snap.targets.filter((t) => t.enabled);
  const noTargets = enabledTargets.length === 0;

  const body = document.createElement("div");
  body.style.cssText = "display:flex;flex-direction:column;gap:18px";

  // --- 导入范围：先挑哪些 skill 进库（Git 来源可以只取其中一部分） ---
  const sourceSkills = snap.skills.filter((s) => s.source_id === sourceId);
  const excludedRel = new Set<string>();
  let importSection: HTMLElement;
  {
    const sec = document.createElement("div");
    sec.innerHTML = `
      <div style="display:flex;align-items:center;margin-bottom:10px">
        <div style="font-size:13px;font-weight:600;color:var(--ink)">扫描到 ${sourceSkills.length} 个 skill，导入哪些</div>
        <span style="flex:1"></span>
        <button class="btn btn--ghost btn--sm" data-act="sel-all">全选</button>
        <button class="btn btn--ghost btn--sm" data-act="sel-none">清空</button>
      </div>
      <div class="combo-member-list post-add__skills" data-import-list></div>
    `;
    const listEl = sec.querySelector("[data-import-list]")!;

    function paintImports() {
      listEl.innerHTML = "";
      if (sourceSkills.length === 0) {
        listEl.innerHTML = `<div style="font-size:12px;color:var(--ink-mute);padding:6px">没有扫到 skill —— 确认目录里有 SKILL.md 后可重新扫描。</div>`;
        return;
      }
      for (const s of sourceSkills) {
        const rel = String(s.relative_path);
        const row = document.createElement("label");
        row.className = "combo-member";
        row.innerHTML = `
          <input type="checkbox" data-rel="${escapeHtml(rel)}" ${excludedRel.has(rel) ? "" : "checked"} />
          <span class="combo-member__name">${escapeHtml(s.name)}</span>
          <span class="combo-member__src">${escapeHtml(rel)}</span>
        `;
        row.addEventListener("change", () => {
          const on = (row.querySelector("input") as HTMLInputElement).checked;
          if (on) excludedRel.delete(rel);
          else excludedRel.add(rel);
        });
        listEl.append(row);
      }
    }
    paintImports();
    sec.querySelector("[data-act='sel-all']")!.addEventListener("click", () => { excludedRel.clear(); paintImports(); });
    sec.querySelector("[data-act='sel-none']")!.addEventListener("click", () => {
      for (const s of sourceSkills) excludedRel.add(String(s.relative_path));
      paintImports();
    });
    importSection = sec;
    body.append(importSection);
  }

  // --- Install-strategy section ---
  const stratSection = document.createElement("div");
  stratSection.innerHTML = `
    <div style="font-size:13px;font-weight:600;color:var(--ink);margin-bottom:10px">
      安装这 ${skillCount} 个 skill
    </div>
    <div class="col" style="gap:10px" data-strat-list></div>
  `;
  const stratList = stratSection.querySelector("[data-strat-list]")!;
  type Strat = { value: "all" | "pick"; title: string; desc: string; recommended?: boolean };
  const strats: Strat[] = [
    { value: "all", title: "全部安装", desc: "把这 ${N} 个 skill 装到下方勾选的 agent。".replace("${N}", String(skillCount)), recommended: true },
    { value: "pick", title: "选择安装", desc: "先添加到库，稍后在 skills 页逐个安装。" },
  ];
  // "全部安装" is meaningless without any enabled agent — default to "pick".
  let chosen: "all" | "pick" = noTargets ? "pick" : "all";
  const stratItems: Record<string, HTMLDivElement> = {};
  const applyStratVisual = (val: string) => {
    for (const el of Object.values(stratItems)) {
      const active = el.dataset.value === val;
      el.style.borderColor = active ? "var(--accent)" : "var(--border)";
      el.style.background = active ? "var(--accent-soft)" : "var(--bg)";
      el.style.boxShadow = active ? "var(--shadow-glow)" : "none";
      el.querySelector<HTMLElement>(".strat-check")!.style.opacity = active ? "1" : "0";
    }
    // Show/hide the target list + watch option depending on the choice.
    const targetsPanel = body.querySelector<HTMLElement>("[data-targets-panel]");
    if (targetsPanel) targetsPanel.style.display = val === "all" ? "" : "none";
    const pickHint = body.querySelector<HTMLElement>("[data-pick-hint]");
    if (pickHint) pickHint.style.display = val === "pick" ? "" : "none";
  };
  for (const s of strats) {
    const item = document.createElement("div");
    item.dataset.value = s.value;
    // role=button 让 AX/键盘能触达（和 add-picker 类型卡片一致）
    item.setAttribute("role", "button");
    item.tabIndex = 0;
    item.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        (item as HTMLDivElement).click();
      }
    });
    item.style.cssText = `
      display:flex;align-items:flex-start;gap:12px;
      padding:14px 16px;border:1px solid var(--border);border-radius:10px;
      background:var(--bg);cursor:pointer;
      transition:border-color var(--t-fast) var(--ease-out),
                 background var(--t-fast) var(--ease-out),
                 box-shadow var(--t-fast) var(--ease-out);
      position:relative;
    `;
    item.innerHTML = `
      <div style="flex:1;min-width:0">
        <div style="display:flex;align-items:center;gap:8px">
          <span style="font-family:var(--font-display);color:var(--ink);font-size:15px;font-weight:600">${s.title}</span>
          ${s.recommended ? `<span class="tag" style="font-size:10px">推荐</span>` : ""}
        </div>
        <div style="font-size:12px;color:var(--ink-mute);margin-top:4px;line-height:1.5">${s.desc}</div>
      </div>
      <span class="icon strat-check" data-icon="check" data-size="14" style="width:20px;height:20px;display:grid;place-items:center;background:var(--accent);color:#fff;border-radius:50%;opacity:0;transition:opacity var(--t-fast);flex-shrink:0"></span>
    `;
    item.addEventListener("click", () => {
      if (chosen === s.value) return;
      chosen = s.value;
      applyStratVisual(chosen);
    });
    stratItems[s.value] = item;
    stratList.append(item);
  }
  paintIcons(stratSection);
  body.append(stratSection);

  // --- Targets panel (only relevant for "全部安装") ---
  const targetsPanel = document.createElement("div");
  targetsPanel.dataset.targetsPanel = "";
  targetsPanel.style.cssText = "display:flex;flex-direction:column;gap:10px";
  targetsPanel.innerHTML = `
    <div style="font-size:13px;font-weight:600;color:var(--ink)">安装到哪些 agent</div>
    <div class="col" style="gap:6px" data-target-list></div>
    <label class="post-add__watch" style="display:flex;align-items:flex-start;gap:10px;padding:10px 12px;border:1px solid var(--border);border-radius:8px;background:var(--bg-elev);cursor:pointer;${canWatch ? "" : "display:none"}" data-watch-wrap>
      <input type="checkbox" data-watch checked style="accent-color:var(--accent);margin-top:2px" />
      <div style="flex:1;min-width:0">
        <div style="font-size:13px;color:var(--ink);font-weight:500">监听新增 skill，自动安装</div>
        <div style="font-size:12px;color:var(--ink-mute);margin-top:3px;line-height:1.5">文件夹里之后新增的 skill 会自动装到上面勾选的 agent。</div>
      </div>
    </label>
  `;
  const targetList = targetsPanel.querySelector("[data-target-list]")!;
  if (noTargets) {
    const hint = document.createElement("div");
    hint.style.cssText = "font-size:12px;color:var(--ink-mute);padding:4px 0";
    hint.textContent = "还没有启用的 agent — 前往 agents 页启用后再来。";
    targetList.append(hint);
  } else {
    for (const t of enabledTargets) {
      const item = document.createElement("label");
      item.style.cssText = `
        display:flex;align-items:center;gap:12px;
        padding:10px 12px;border:1px solid var(--border);
        background:var(--bg);cursor:pointer;
      `;
      item.innerHTML = `
        <input type="checkbox" data-tid="${t.id}" ${precheckAll ? "checked" : ""} style="accent-color:var(--accent)" />
        ${agentIconMarkup(t.id, t.name, "agent-icon--row")}
        <div style="flex:1;min-width:0">
          <div style="font-family:var(--font-display);color:var(--ink);font-size:15px">${escapeHtml(t.name)}</div>
          <div style="font-family:var(--font-mono);font-size:12px;color:var(--ink-mute);letter-spacing:0.04em">${escapeHtml(tildePath(t.skills_dir))}</div>
        </div>
      `;
      targetList.append(item);
    }
    paintAgentIcons(targetList);
  }
  body.append(targetsPanel);

  // --- "选择安装" hint ---
  const pickHint = document.createElement("div");
  pickHint.dataset.pickHint = "";
  pickHint.style.cssText = "font-size:12.5px;color:var(--ink-mute);padding:8px 12px;background:var(--bg-elev);border-radius:8px;line-height:1.6;display:none";
  pickHint.textContent = `已添加 ${skillCount} 个 skill 到库。稍后在「skills」页面，点单个 skill 的 + 按钮即可安装到目标。`;
  body.append(pickHint);

  applyStratVisual(chosen);

  // --- Footer ---
  const footer = document.createElement("div");
  footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;align-items:center;flex:1";
  footer.innerHTML = `
    <button class="btn btn--ghost" data-act="later">跳过</button>
    <button class="btn btn--primary" data-act="apply">确定</button>
  `;

  const modal = openModal({ title: `配置「${name}」`, body, footer });
  paintIcons(footer);

  footer.querySelector("[data-act='later']")!.addEventListener("click", () => modal.close());
  footer.querySelector("[data-act='apply']")!.addEventListener("click", async () => {
    const btn = footer.querySelector("[data-act='apply']") as HTMLButtonElement;
    btn.disabled = true;
    try {
      // ① 先落实导入范围：未勾选的进隐藏集（重扫也不会回来）。
      if (excludedRel.size > 0) {
        await api.hideSourceSkills(sourceId, [...excludedRel]);
        if (chosen === "all" && excludedRel.size === sourceSkills.length) {
          toast("全部 skill 已排除，来源已留空库", "info");
          await store.refresh();
          return;
        }
      }

      if (chosen === "pick") {
        // Nothing to do — source already scanned into the library.
        await store.refresh();
        toast(`已添加 ${skillCount} 个 skill，可去 skills 页安装`, "success");
        return;
      }
      // "全部安装"
      const targetIds = Array.from(
        body.querySelectorAll<HTMLInputElement>("input[type='checkbox'][data-tid]:checked"),
      ).map((c) => c.dataset.tid!);
      if (targetIds.length === 0) {
        toast("请至少勾选一个 agent", "error");
        btn.disabled = false;
        return;
      }
      const watchChecked = canWatch
        ? (body.querySelector<HTMLInputElement>("input[data-watch]")?.checked ?? false)
        : false;

      if (canWatch && watchChecked) {
        // set_source_auto_sync installs all current skills to the targets AND
        // starts the folder watcher — one call does both.
        await api.setSourceAutoSync(sourceId, targetIds);
      } else {
        // Batch-install all skills to each chosen target, no watcher.
        let totalInstalled = 0;
        const failures: string[] = [];
        for (const tid of targetIds) {
          const r = await api.installSourceGroup(sourceId, tid);
          totalInstalled += r.installed;
          failures.push(...r.failed);
        }
        if (failures.length) {
          toast(`安装完成：${totalInstalled} 成功，${failures.length} 失败`, "error");
        }
      }
      await store.refresh();
      toast("来源已配置", "success");
    } catch (e) {
      toast(`配置失败：${e}`, "error");
    } finally {
      modal.close();
    }
  });
}

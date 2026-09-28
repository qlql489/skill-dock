// 待办概览 —— 一个页面说清"现在有什么等着处理"：
// 未安装的 skill / 失效或冲突的软链 / 有远端更新的来源 / 未检测到的 agent。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { escapeHtml } from "./library";
import type { PendingReport } from "../types";

export function renderPending(mount: HTMLElement): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  view.className = "pending-view";

  const head = document.createElement("div");
  head.className = "view-head";
  head.innerHTML = `
    <div>
      <div class="kicker">概览</div>
      <h1 class="view-head__title">待办事项</h1>
    </div>
    <div class="view-head__actions">
      <button class="btn btn--ghost btn--sm" data-act="cleanup"><span data-icon="delete" data-size="13"></span>清理失效记录</button>
      <button class="btn btn--ghost btn--sm" data-act="refresh"><span data-icon="refresh" data-size="13"></span>刷新</button>
    </div>
  `;
  view.append(head);

  const board = document.createElement("div");
  board.className = "pending-board";
  board.innerHTML = `<div class="empty"><p class="empty__sub">加载中…</p></div>`;
  view.append(board);
  mount.append(view);

  async function load() {
    let report: PendingReport;
    try {
      report = await api.pendingItems();
    } catch (e) {
      board.innerHTML = `<div class="empty"><p class="empty__sub">加载失败：${escapeHtml(String(e))}</p></div>`;
      return;
    }
    paint(report);
  }

  function paint(r: PendingReport) {
    board.innerHTML = "";

    const sections: {
      key: string;
      title: string;
      icon: string;
      tone: "bad" | "warn" | "info";
      count: number;
      rows: () => HTMLElement[];
    }[] = [
      {
        key: "broken",
        title: "失效 / 冲突软链",
        icon: "alert",
        tone: "bad",
        count: r.broken_links.length,
        rows: () =>
          r.broken_links.map((b) => {
            const row = document.createElement("div");
            row.className = "pending-row";
            row.innerHTML = `
              <span class="pending-row__dot is-bad"></span>
              <div style="flex:1;min-width:0">
                <div class="pending-row__name">${escapeHtml(b.skill_name)} <span style="color:var(--ink-mute)">→ ${escapeHtml(b.target_name ?? "")}</span></div>
                <div class="pending-row__detail">${escapeHtml(tildePath(b.detail))}</div>
              </div>
              <button class="btn btn--sm btn--ghost" data-fix><span data-icon="link" data-size="12"></span>重装链接</button>
            `;
            row.querySelector("[data-fix]")!.addEventListener("click", async (e) => {
              (e.currentTarget as HTMLButtonElement).disabled = true;
              try {
                const res = await api.applyCells([[b.skill_id, b.target_id!]], [], []);
                if (res.failures.length) toast(res.failures[0].error, "error");
                else toast("已重建链接", "success");
              } catch (err) {
                toast(`失败：${err}`, "error");
              } finally {
                (e.currentTarget as HTMLButtonElement).disabled = false;
              }
            });
            return row;
          }),
      },
      {
        key: "outdated",
        title: "有远端更新的来源",
        icon: "download",
        tone: "warn",
        count: r.outdated_sources.length,
        rows: () =>
          r.outdated_sources.map((name) => {
            const row = document.createElement("a");
            row.className = "pending-row pending-row--link";
            row.innerHTML = `
              <span class="pending-row__dot is-warn"></span>
              <div style="flex:1;min-width:0">
                <div class="pending-row__name">${escapeHtml(name)}</div>
                <div class="pending-row__detail">远端有新提交，到中央技能库「按来源」里查看更新后才会拉取（不会自动拉）。</div>
              </div>
              <span data-icon="arrowRight" data-size="14"></span>
            `;
            row.addEventListener("click", () => {
              // 来源页已并入技能库「按来源」tab：先切 tab 再跳。
              try { localStorage.setItem("skill-dock:display-mode", "by-source"); } catch { /* ignore */ }
              location.hash = "#/library";
            });
            return row as unknown as HTMLElement;
          }),
      },
      {
        key: "undetected",
        title: "未检测到的 agent",
        icon: "target",
        tone: "warn",
        count: r.undetected_targets.length,
        rows: () =>
          r.undetected_targets.map((name) => {
            const row = document.createElement("a");
            row.className = "pending-row pending-row--link";
            row.innerHTML = `
              <span class="pending-row__dot is-warn"></span>
              <div style="flex:1;min-width:0">
                <div class="pending-row__name">${escapeHtml(name)}</div>
                <div class="pending-row__detail">已启用但本机未检测到该 agent，装到它上面的 skill 可能不可见。</div>
              </div>
              <span data-icon="arrowRight" data-size="14"></span>
            `;
            row.addEventListener("click", () => (location.hash = "#/targets"));
            return row as unknown as HTMLElement;
          }),
      },
      {
        key: "uninstalled",
        title: "从未安装的 skill",
        icon: "package",
        tone: "info",
        count: Math.min(r.uninstalled.length, 500),
        rows: () =>
          r.uninstalled.slice(0, 60).map((u) => {
            const row = document.createElement("a");
            row.className = "pending-row pending-row--link";
            row.innerHTML = `
              <span class="pending-row__dot is-info"></span>
              <div style="flex:1;min-width:0">
                <div class="pending-row__name">${escapeHtml(u.skill_name)}</div>
                <div class="pending-row__detail">点击进入详情，用 agent 徽章或 token 安装。</div>
              </div>
              <span data-icon="arrowRight" data-size="14"></span>
            `;
            row.addEventListener("click", () => {
              location.hash = `#/skill/${encodeURIComponent(u.skill_id)}`;
            });
            return row as unknown as HTMLElement;
          }),
      },
    ];

    if (r.broken_links.length + r.outdated_sources.length + r.undetected_targets.length + r.uninstalled.length === 0) {
      board.innerHTML = `
        <div class="empty">
          <div class="empty__icon" data-icon="check" data-size="22"></div>
          <h2 class="empty__title">一切就绪</h2>
          <p class="empty__sub">没有等待处理的事项。</p>
        </div>`;
      paintIcons(board);
      return;
    }

    for (const s of sections) {
      if (s.count === 0) continue;
      const panel = document.createElement("section");
      panel.className = `pending-panel tone-${s.tone}`;
      panel.innerHTML = `
        <header class="pending-panel__head">
          <span class="icon" data-icon="${s.icon}" data-size="15"></span>
          <h2>${s.title}</h2>
          <span class="pending-panel__count">${s.count}</span>
        </header>
        <div class="pending-panel__body" data-rows></div>
      `;
      const rowsHost = panel.querySelector("[data-rows]")!;
      for (const el of s.rows()) rowsHost.append(el);
      if (s.key === "uninstalled" && r.uninstalled.length > 60) {
        const more = document.createElement("div");
        more.className = "pending-more";
        more.textContent = `还有 ${r.uninstalled.length - 60} 个未安装的 skill，去 skills 页查看全部`;
        more.addEventListener("click", () => (location.hash = "#/library"));
        rowsHost.append(more);
      }
      board.append(panel);
    }
    paintIcons(board);
  }

  head.querySelector("[data-act='refresh']")!.addEventListener("click", load);
  head.querySelector("[data-act='cleanup']")!.addEventListener("click", async () => {
    try {
      const n = await api.cleanupInstallations(["missing", "conflict"]);
      await store.refresh();
      toast(n > 0 ? `已清理 ${n} 条失效记录` : "没有可清理的记录", n > 0 ? "success" : "info");
      load();
    } catch (e) {
      toast(`清理失败：${e}`, "error");
    }
  });

  // store 变化时自动刷新数据
  const unsub = store.subscribe(() => load());
  mount.addEventListener("cleanup", () => unsub(), { once: true });

  paintIcons(view);
  load();
}

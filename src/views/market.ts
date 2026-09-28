// Market view — Skill 市场(skills.sh / 腾讯 SkillHub / ClawHub)。
//
// 顶部源切换(skills.sh 默认);skills.sh 空搜索词显示榜单(总榜/趋势/热门),
// 另两家各有默认榜单(SkillHub 按下载量、ClawHub 走官方 trending)。
// 点「安装」把该 skill 拉取(GitHub 克隆或 zip 下载→安全解压)并复制进常驻的
// 「Skill 市场」本地来源,之后与其他来源的 skill 一样软链安装到各 agent。
// 「已安装」判定 = 登记表(registry)命中 且 库里还存在对应目录的 skill。

import { api } from "../api";
import { store, tildePath } from "../store";
import { paintIcons } from "../icon";
import { toast } from "../toast";
import { escapeHtml } from "./library";
import type { MarketEntry, MarketProvider, MarketRecord } from "../types";

type Board = "all" | "trending" | "hot";
const SEARCH_DEBOUNCE_MS = 450;
/** 前端内存缓存 TTL：榜单/搜索都是低频变化数据，90 秒足够新鲜。 */
const CACHE_TTL_MS = 90_000;
/** key = provider|board|query。模块级缓存跨页面切换存活。 */
const marketCache = new Map<string, { at: number; entries: MarketEntry[] }>();
function cacheKey(provider: MarketProvider, board: Board, query: string) {
  return `${provider}|${board}|${query.trim()}`;
}
/** 榜单单页最多渲染条数(仅 skillssh 需要截断:它的榜单一次给 600 条)。 */
const BOARD_DISPLAY_LIMIT = 50;

const PROVIDERS: { value: MarketProvider; label: string; hint: string }[] = [
  { value: "skillssh", label: "skills.sh", hint: "国际主流,安装量排行(Vercel)" },
  { value: "skillhub", label: "SkillHub", hint: "腾讯云,中文技能,国内直连" },
  { value: "clawhub", label: "ClawHub", hint: "OpenClaw 生态官方市场" },
];

export function renderMarket(mount: HTMLElement): void {
  mount.innerHTML = "";
  const view = document.createElement("div");
  mount.append(view);

  let provider: MarketProvider = "skillssh";
  let board: Board = "all";
  let query = "";
  let entries: MarketEntry[] = [];
  let registry: MarketRecord[] = [];
  let loading = false;
  let errorMsg = "";
  const installing = new Set<string>(); // entry.id
  let reqSeq = 0;
  let debounceTimer: number | null = null;

  // ---- Persistent head:源切换 + 搜索框 + board tab + 外链 ----
  const providerTabs = document.createElement("div");
  providerTabs.className = "tabs";
  providerTabs.style.marginBottom = "14px";
  for (const p of PROVIDERS) {
    const b = document.createElement("button");
    b.className = "tab";
    b.dataset.provider = p.value;
    b.title = p.hint;
    b.textContent = p.label;
    b.addEventListener("click", () => {
      if (provider === p.value) return;
      provider = p.value;
      // 换源后原搜索词不通用,清掉回到榜单态。
      query = "";
      searchInput.value = "";
      syncTabs();
      void load();
    });
    providerTabs.append(b);
  }
  view.append(providerTabs);

  const head = document.createElement("div");
  head.className = "view-head";
  head.innerHTML = `
    <div class="row" style="flex:1;gap:10px">
      <div class="row" style="flex:1;max-width:460px;gap:8px;position:relative">
        <span class="icon" data-icon="search" data-size="14" style="position:absolute;left:12px;color:var(--ink-mute);pointer-events:none"></span>
        <input class="input" data-input="search" placeholder="搜索当前市场的技能…"
               style="padding-left:32px" spellcheck="false" />
      </div>
      <div class="tabs" data-board-tabs>
        <button class="tab" data-board="all">总榜</button>
        <button class="tab" data-board="trending">趋势</button>
        <button class="tab" data-board="hot">热门</button>
      </div>
    </div>
    <div class="view-head__actions">
      <button class="btn btn--ghost" data-action="refresh" title="立即重新获取（绕过缓存）"><span data-icon="refresh" data-size="14"></span>刷新</button>
      <button class="btn btn--ghost" data-action="open-market" title="在浏览器打开当前市场">
        <span data-icon="external" data-size="14"></span><span data-market-label>skills.sh</span>
      </button>
    </div>
  `;
  view.append(head);

  const note = document.createElement("div");
  note.className = "mkt-note";
  view.append(note);

  const list = document.createElement("div");
  list.className = "mkt-list";
  view.append(list);

  const searchInput = head.querySelector<HTMLInputElement>("[data-input='search']")!;
  const marketUrl = () =>
    provider === "skillhub" ? "https://skillhub.cloud.tencent.com/skills" : provider === "clawhub" ? "https://clawhub.ai" : "https://www.skills.sh";
  const providerLabel = () => PROVIDERS.find((p) => p.value === provider)?.label ?? provider;

  // ------------------------------------------------------------------
  // 数据
  // ------------------------------------------------------------------

  async function loadRegistry() {
    try {
      registry = await api.marketRegistry();
    } catch {
      registry = [];
    }
  }

  /** entry.id 已在库里 = 登记表命中且 market 来源下该目录仍存在。 */
  function installedKeys(): Set<string> {
    const snap = store.get();
    const marketLoc = `${snap.data_dir.replace(/\/$/, "")}/market`;
    const src = snap.sources.find((s) => s.kind === "local" && s.location === marketLoc);
    const out = new Set<string>();
    if (!src) return out;
    const dirs = new Set(
      snap.skills.filter((s) => s.source_id === src.id).map((s) => s.relative_path.replace(/\/+$/, "")),
    );
    for (const r of registry) {
      if (dirs.has(r.dir)) out.add(`${r.provider || "skillssh"}:${r.source}/${r.skill_id}`);
    }
    return out;
  }

  async function load() {
    const seq = ++reqSeq;
    const key = cacheKey(provider, board, query);
    const cached = marketCache.get(key);
    const fresh = cached && Date.now() - cached.at < CACHE_TTL_MS;
    if (cached) {
      loading = false;
      errorMsg = "";
      entries = cached.entries;
      paint();
      if (fresh) return;
    } else {
      loading = true;
      errorMsg = "";
      paint();
    }
    try {
      const q = query.trim();
      const result = q
        ? await api.marketSearch(provider, q, 60)
        : await api.marketLeaderboard(provider, board);
      if (seq !== reqSeq) return;
      marketCache.set(key, { at: Date.now(), entries: result });
      entries = result;
      loading = false;
      paint();
    } catch (e) {
      if (seq !== reqSeq) return;
      if (!cached) {
        entries = [];
        errorMsg = String(e);
      }
      loading = false;
      paint();
    }
  }

  function scheduleSearch() {
    if (debounceTimer !== null) window.clearTimeout(debounceTimer);
    debounceTimer = window.setTimeout(() => {
      debounceTimer = null;
      query = searchInput.value;
      syncTabs();
      void load();
    }, SEARCH_DEBOUNCE_MS);
  }

  function syncTabs() {
    const searching = !!query.trim();
    // board tab 仅 skillssh 有意义。
    const boardTabs = head.querySelector<HTMLElement>("[data-board-tabs]")!;
    boardTabs.style.display = provider === "skillssh" ? "" : "none";
    for (const b of boardTabs.querySelectorAll<HTMLButtonElement>("[data-board]")) {
      const isBoard = provider === "skillssh" && !searching && b.dataset.board === board;
      if (isBoard) b.setAttribute("data-active", "true");
      else b.removeAttribute("data-active");
      b.disabled = searching || provider !== "skillssh";
    }
    for (const b of providerTabs.querySelectorAll<HTMLButtonElement>("[data-provider]")) {
      if (b.dataset.provider === provider) b.setAttribute("data-active", "true");
      else b.removeAttribute("data-active");
    }
    const label = head.querySelector<HTMLElement>("[data-market-label]");
    if (label) label.textContent = providerLabel();
    note.textContent = `安装 = 把该 skill 复制到本机「Skill 市场」来源(${tildePath(`${store.get().data_dir}/market`)})，之后在「skills」库里软链安装到各 agent；更新在市场重新点安装即可。`;
  }

  // ------------------------------------------------------------------
  // 渲染
  // ------------------------------------------------------------------

  function paint() {
    syncTabs();
    list.innerHTML = "";

    if (loading && entries.length === 0) {
      list.innerHTML = `<div class="empty"><p class="empty__sub">正在加载市场数据…</p></div>`;
      return;
    }
    if (errorMsg) {
      list.innerHTML = `
        <div class="empty">
          <div class="empty__icon" data-icon="alert" data-size="22"></div>
          <h2 class="empty__title">市场数据拉取失败</h2>
          <p class="empty__sub">${escapeHtml(errorMsg)}</p>
        </div>`;
      paintIcons(list);
      return;
    }
    if (entries.length === 0) {
      list.innerHTML = `
        <div class="empty">
          <div class="empty__icon" data-icon="search" data-size="22"></div>
          <h2 class="empty__title">没有找到匹配的 skill</h2>
          <p class="empty__sub">换个关键词试试,或清空搜索看榜单。</p>
        </div>`;
      paintIcons(list);
      return;
    }

    const installed = installedKeys();
    const searching = !!query.trim();
    const capped = provider === "skillssh" && !searching;
    const visible = capped ? entries.slice(0, BOARD_DISPLAY_LIMIT) : entries;
    visible.forEach((entry, i) => list.append(renderRow(entry, i + 1, installed.has(entry.id))));
    if (capped && entries.length > visible.length) {
      const foot = document.createElement("div");
      foot.className = "mkt-note";
      foot.style.padding = "10px 16px";
      foot.textContent = `榜单仅展示前 ${BOARD_DISPLAY_LIMIT} 条,更多请用搜索。`;
      list.append(foot);
    }
    paintIcons(list);
  }

  function renderRow(entry: MarketEntry, idx: number, installed: boolean): HTMLElement {
    const row = document.createElement("div");
    row.className = "mkt-row";
    const busy = installing.has(entry.id);
    row.innerHTML = `
      <div class="mkt-idx">${String(idx).padStart(2, "0")}</div>
      <div class="mkt-main">
        <h3 class="mkt-name">
          ${escapeHtml(entry.name)}
          ${entry.is_official ? `<span class="mkt-official" title="官方出品">官方</span>` : ""}
        </h3>
        ${entry.description ? `<div class="mkt-desc" title="${escapeHtml(entry.description)}">${escapeHtml(entry.description)}</div>` : ""}
        ${entry.source ? `<button class="mkt-src" data-action="open-repo" title="打开 ${escapeHtml(entry.source)}">
          <span class="icon" data-icon="git" data-size="11" style="vertical-align:-1px"></span>
          ${escapeHtml(entry.source)}
        </button>` : ""}
      </div>
      <div class="mkt-installs" title="安装/下载量(来自市场)">
        <span class="icon" data-icon="download" data-size="12" style="vertical-align:-1px"></span>
        ${formatInstalls(entry.installs)}
      </div>
      <div class="mkt-action"></div>
    `;
    const action = row.querySelector<HTMLElement>(".mkt-action")!;
    if (installed) {
      action.innerHTML = `<span class="mkt-installed"><span class="icon" data-icon="check" data-size="12"></span>已安装</span>`;
    } else {
      const btn = document.createElement("button");
      btn.className = "btn btn--primary btn--sm";
      btn.disabled = busy;
      btn.innerHTML = busy
        ? `<span class="icon" data-icon="refresh" data-size="13"></span>安装中…`
        : `<span class="icon" data-icon="download" data-size="13"></span>安装`;
      btn.addEventListener("click", () => void install(entry));
      action.append(btn);
    }

    row.querySelector("[data-action='open-repo']")?.addEventListener("click", (e) => {
      e.stopPropagation();
      const url = entry.provider === "skillssh"
        ? `https://github.com/${entry.source}`
        : entry.provider === "clawhub"
          ? `https://clawhub.ai/${entry.source}/skills/${entry.skill_id}`
          : `https://skillhub.cloud.tencent.com/skills/${entry.skill_id}`;
      void api.openPath(url).catch((err) => toast(String(err), "error"));
    });
    return row;
  }

  async function install(entry: MarketEntry) {
    if (installing.has(entry.id)) return;
    installing.add(entry.id);
    paint();
    try {
      const outcome = await api.installMarketSkill(entry.provider, entry.source, entry.skill_id);
      await Promise.all([store.refresh(), loadRegistry()]);
      toast(
        outcome.replaced
          ? `已更新「${outcome.name}」(${outcome.files} 个文件)`
          : `已安装「${outcome.name}」到 Skill 市场(${outcome.files} 个文件),可到 skills 页安装到 agent`,
        "success",
      );
    } catch (e) {
      toast(`安装失败:${e}`, "error");
    } finally {
      installing.delete(entry.id);
      paint();
    }
  }

  // ------------------------------------------------------------------
  // 事件绑定
  // ------------------------------------------------------------------

  searchInput.addEventListener("input", scheduleSearch);
  head.querySelector("[data-action='refresh']")?.addEventListener("click", () => {
    marketCache.delete(cacheKey(provider, board, query));
    void load();
  });
  head.querySelector("[data-action='open-market']")?.addEventListener("click", () => {
    void api.openPath(marketUrl()).catch((err) => toast(String(err), "error"));
  });
  for (const b of head.querySelectorAll<HTMLButtonElement>("[data-board]")) {
    b.addEventListener("click", () => {
      const val = b.dataset.board as Board;
      if (provider !== "skillssh" || query.trim() || val === board) return;
      board = val;
      syncTabs();
      void load();
    });
  }

  // 路由切换时清理定时器;store 刷新(如后台扫描完成)时重绘按钮状态。
  const unsubscribe = store.subscribe(() => {
    if (!loading) paint();
  });
  mount.addEventListener("cleanup", () => {
    unsubscribe();
    if (debounceTimer !== null) window.clearTimeout(debounceTimer);
  });

  // 首帧:登记表 + 榜单并行拉。
  void (async () => {
    await loadRegistry();
    await load();
  })();
}

function formatInstalls(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

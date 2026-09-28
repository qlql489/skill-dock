// Settings shell — application preferences.  The shell owns navigation;
// section content is supplied by renderers so each section can grow without
// coupling it to the route. (Agent 管理已合并到 Agents 页，设置只留通用偏好。)

import { api } from "../api";
import { paintIcons } from "../icon";
import { store, tildePath } from "../store";
import { toast } from "../toast";
import { ACCENT_PRESETS, applyAccentColor, changeAccent, getAccent } from "../appearance";
import { getAddDefaultInstallTargets, setAddDefaultInstallTargets, getVaultPath, setVaultPath } from "../prefs";
import { openModal } from "../modal";
import {
  UPDATER_ENABLED,
  UPDATER_DISABLED_REASON,
  checkForAppUpdate,
  downloadAndInstallUpdate,
  getAppVersion,
  getUpdateState,
  onUpdateState,
  relaunchToUpdate,
} from "../updater";

export type SettingsSectionId = "general";

export interface SettingsSectionContext {
  /** Section currently shown in the right-hand pane. */
  section: SettingsSectionId;
  /** The reserved right-hand content mount (`.settings-page__body`). */
  body: HTMLElement;
  /** Navigate to another settings section without knowing the hash format. */
  navigate: (section: SettingsSectionId) => void;
}

export type SettingsSectionRenderer = (context: SettingsSectionContext) => void;

export interface SettingsRenderOptions {
  section?: SettingsSectionId;
  renderers?: Partial<Record<SettingsSectionId, SettingsSectionRenderer>>;
}

const SECTION_META: Record<SettingsSectionId, { label: string; description: string; icon: string }> = {
  general: {
    label: "通用",
    description: "应用行为、外观与默认设置",
    icon: "settings",
  },
};

function isSettingsSection(value: string | undefined): value is SettingsSectionId {
  return value === "general";
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function defaultSectionRenderer(context: SettingsSectionContext): void {
  renderGeneralSection(context);
}

function renderGeneralSection({ body }: SettingsSectionContext): void {
  const snap = store.get();
  const githubSources = snap.sources.filter((source) => source.kind === "github").length;
  body.innerHTML = `
    <div class="settings-groups">
      <section class="settings-group" aria-labelledby="settings-appearance-title">
        <header class="settings-group__head">
          <div>
            <h3 id="settings-appearance-title">外观</h3>
          </div>
        </header>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>主题颜色</strong>
            <span>选择应用的主题色，立即生效并自动保存。</span>
          </div>
          <div class="accent-picker">
            <div class="accent-picker__presets">
              ${ACCENT_PRESETS.map(
                (p) => `
                <button class="accent-swatch" data-accent="${p.value}"
                        style="background:${p.value}"
                        title="${p.name}" aria-label="${p.name}">
                  <span class="accent-swatch__check">✓</span>
                </button>`,
              ).join("")}
              <label class="accent-swatch accent-swatch--custom" title="自定义颜色">
                <input type="color" data-accent-custom value="${escapeHtml(getAccent())}" />
                <span class="accent-swatch__check">✓</span>
              </label>
            </div>
            <code class="accent-picker__value" data-accent-value>${escapeHtml(getAccent())}</code>
          </div>
        </div>
      </section>

      <section class="settings-group" aria-labelledby="settings-add-title">
        <header class="settings-group__head">
          <div>
            <h3 id="settings-add-title">添加来源</h3>
          </div>
        </header>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>添加 skills 默认安装到已启用的 Agents</strong>
            <span>打开后，添加来源时的「全部安装」步骤会自动勾选 Agents 页里所有已启用的 Agents，点确定即完成安装。关闭后每次手动挑选。</span>
          </div>
          <button class="switch" data-add-default-targets role="switch" aria-checked="true" title="切换默认安装到已启用的 Agents">
            <span class="switch__thumb"></span>
          </button>
        </div>
      </section>

      <section class="settings-group" aria-labelledby="settings-sync-title">
        <header class="settings-group__head">
          <div>
            <h3 id="settings-sync-title">同步</h3>
          </div>
          <span class="settings-group__status"><span class="badge-dot badge-dot--success"></span>本机</span>
        </header>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>中央仓库</strong>
            <span>「纳入管理」默认复制技能的目的地，多个 Agent 通过软链共用这里的技能。只改配置不迁移文件。</span>
          </div>
          <div class="settings-row__control settings-row__control--path">
            <code data-vault-code title="${escapeHtml(getVaultPath())}">${escapeHtml(tildePath(getVaultPath()))}</code>
            <button class="btn btn--ghost" data-change-vault><span data-icon="edit" data-size="14"></span>更改路径</button>
            <button class="btn btn--ghost" data-open-vault><span data-icon="folder" data-size="14"></span>打开文件夹</button>
          </div>
        </div>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>数据目录</strong>
            <span>状态文件、技能索引和 Git 缓存的存储位置（app 内部数据，不是 skill 本体）。</span>
          </div>
          <div class="settings-row__control settings-row__control--path">
            <code title="${escapeHtml(snap.data_dir)}">${escapeHtml(tildePath(snap.data_dir))}</code>
            <button class="btn btn--ghost" data-change-data title="目录迁移需要确认后执行"><span data-icon="edit" data-size="14"></span>更改目录</button>
            <button class="btn btn--ghost" data-open-data><span data-icon="folder" data-size="14"></span>打开文件夹</button>
          </div>
        </div>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>状态文件</strong>
            <span>保存来源、Agent、安装关系和组合配置。</span>
          </div>
          <div class="settings-row__control settings-row__control--path">
            <code title="${escapeHtml(snap.state_path)}">${escapeHtml(tildePath(snap.state_path))}</code>
            <button class="btn btn--ghost" data-reveal-state><span data-icon="external" data-size="14"></span>在访达中显示</button>
          </div>
        </div>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>默认同步方式</strong>
            <span>Agent 目录通过符号链接指向来源 Skill，修改可实时生效。</span>
          </div>
          <div class="settings-segment" aria-label="默认同步方式">
            <span class="settings-segment__item is-active"><span data-icon="link" data-size="14"></span>符号链接</span>
            <span class="settings-segment__item is-disabled" title="当前版本暂不支持文件复制">文件复制</span>
          </div>
        </div>
      </section>

      <section class="settings-group" aria-labelledby="settings-update-title">
        <header class="settings-group__head">
          <div>
            <h3 id="settings-update-title">应用更新</h3>
          </div>
        </header>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>当前版本</strong>
            <span>启动后每 10 分钟自动检查新版本;下载安装始终需要手动确认。</span>
            <span class="settings-row__result" data-app-update-status aria-live="polite"></span>
          </div>
          <div class="app-update-version">
            <span class="settings-version" data-app-version>…</span>
            <span class="app-update-badge" data-app-update-badge hidden></span>
          </div>
        </div>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>应用更新</strong>
            <span data-app-update-hint>检查并安装 SkillDock 的新版本,安装完成后重启生效。</span>
          </div>
          <div class="settings-row__control app-update-control">
            <div class="app-update-progress" data-app-update-progress hidden>
              <div class="app-update-progress__fill" data-app-update-progress-fill></div>
            </div>
            <button class="btn btn--primary" data-app-update-action><span data-icon="refresh" data-size="14"></span>检查更新</button>
          </div>
        </div>
        <div class="settings-row">
          <div class="settings-row__copy">
            <strong>来源更新</strong>
            <span>检查 ${githubSources} 个 Git 来源是否有新的 Skill 或提交。</span>
            <span class="settings-row__result" data-update-result aria-live="polite"></span>
          </div>
          <button class="btn btn--primary" data-check-updates><span data-icon="refresh" data-size="14"></span>检查更新</button>
        </div>
      </section>
    </div>
  `;

  body.querySelector<HTMLButtonElement>("[data-open-data]")?.addEventListener("click", async () => {
    try { await api.openPath(snap.data_dir); }
    catch (error) { toast(`打开失败：${error}`, "error"); }
  });

  // ---- 中央仓库（纳入管理默认目的地）：查看 / 修改 / 打开 ----
  body.querySelector<HTMLButtonElement>("[data-open-vault]")?.addEventListener("click", async () => {
    try { await api.openPath(getVaultPath()); }
    catch (error) { toast(`打开失败：${error}`, "error"); }
  });
  body.querySelector<HTMLButtonElement>("[data-change-vault]")?.addEventListener("click", () => {
    const field = document.createElement("div");
    field.className = "col";
    field.style.gap = "8px";
    field.innerHTML = `
      <p class="field__hint">纳入管理弹窗的默认目的地。支持 ~ 开头的路径；留空恢复 SkillDock 默认中央仓库。只改配置，不移动已有文件。</p>
      <input class="input" data-vault-input value="${escapeHtml(tildePath(getVaultPath()))}"
             placeholder="~/.skill-dock/skills" spellcheck="false" />
    `;
    const footer = document.createElement("div");
    footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;width:100%";
    footer.innerHTML = `
      <button class="btn btn--ghost" data-act="cancel">取消</button>
      <button class="btn btn--primary" data-act="save">保存</button>
    `;
    const modal = openModal({ title: "中央仓库路径", body: field, footer, width: 520 });
    footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());
    const save = async (e: Event) => {
      const btn = e.currentTarget as HTMLButtonElement;
      const input = field.querySelector<HTMLInputElement>("[data-vault-input]")!;
      btn.disabled = true;
      try {
        // 后端负责展开 ~ / 校验绝对路径，原样透传。
        await setVaultPath(input.value.trim());
        const code = body.querySelector<HTMLElement>("[data-vault-code]");
        if (code) {
          code.textContent = tildePath(getVaultPath());
          code.title = getVaultPath();
        }
        toast("中央仓库路径已更新", "success");
        modal.close();
      } catch (error) {
        toast(`保存失败：${error}`, "error");
        btn.disabled = false;
      }
    };
    footer.querySelector("[data-act='save']")!.addEventListener("click", save);
    field.querySelector("[data-vault-input]")?.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key === "Enter") save(e);
    });
    setTimeout(() => field.querySelector<HTMLInputElement>("[data-vault-input]")?.focus(), 50);
  });

  // ---- 添加来源：默认勾选上次安装的 Agents ----
  const addTargetsSwitch = body.querySelector<HTMLButtonElement>("[data-add-default-targets]");
  if (addTargetsSwitch) {
    const paintSwitch = () => {
      const on = getAddDefaultInstallTargets();
      addTargetsSwitch.classList.toggle("is-on", on);
      addTargetsSwitch.setAttribute("aria-checked", String(on));
      addTargetsSwitch.title = on ? "已开启：添加 skills 默认安装到已启用的 Agents" : "已关闭：每次手动挑选 Agents";
    };
    paintSwitch();
    addTargetsSwitch.addEventListener("click", async () => {
      try {
        await setAddDefaultInstallTargets(!getAddDefaultInstallTargets());
      } catch (error) {
        toast(`保存失败：${error}`, "error");
      }
      paintSwitch();
    });
  }

  // ---- 主题色 ----
  const syncAccentVisual = () => {
    const cur = getAccent().toLowerCase();
    body.querySelectorAll<HTMLElement>(".accent-swatch").forEach((sw) => {
      const v = (sw.dataset.accent ?? "").toLowerCase();
      sw.classList.toggle("is-active", !!v && v === cur);
    });
    const valueEl = body.querySelector<HTMLElement>("[data-accent-value]");
    if (valueEl) valueEl.textContent = cur;
  };
  syncAccentVisual();

  body.querySelectorAll<HTMLButtonElement>(".accent-swatch[data-accent]").forEach((sw) => {
    sw.addEventListener("click", async () => {
      if (await changeAccent(sw.dataset.accent!)) syncAccentVisual();
    });
  });
  body.querySelector<HTMLInputElement>("[data-accent-custom]")?.addEventListener("input", async (e) => {
    const v = (e.target as HTMLInputElement).value;
    if (await changeAccent(v)) syncAccentVisual();
  });
  body.querySelector<HTMLButtonElement>("[data-change-data]")?.addEventListener("click", () => {
    toast("数据目录迁移会移动状态文件和 Git 缓存；确认迁移规则后再执行。", "info");
  });
  body.querySelector<HTMLButtonElement>("[data-reveal-state]")?.addEventListener("click", async () => {
    try { await api.revealInFinder(snap.state_path); }
    catch (error) { toast(`打开失败：${error}`, "error"); }
  });
  body.querySelector<HTMLButtonElement>("[data-check-updates]")?.addEventListener("click", async (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    const result = body.querySelector<HTMLElement>("[data-update-result]");
    button.disabled = true;
    if (result) result.textContent = "正在检查…";
    try {
      const report = await api.checkUpdates();
      const count = report.updates.length;
      if (result) result.textContent = count > 0 ? `发现 ${count} 个来源有更新。` : "所有来源均为最新。";
      toast(count > 0 ? `发现 ${count} 个来源有更新` : "没有可用更新", count > 0 ? "info" : "success");
    } catch (error) {
      if (result) result.textContent = `检查失败：${error}`;
      toast(`检查失败：${error}`, "error");
    } finally {
      button.disabled = false;
    }
  });

  // ---- 应用更新:单按钮状态机,渲染跟随 src/updater.ts 的全局状态 ----
  const versionEl = body.querySelector<HTMLElement>("[data-app-version]");
  const badgeEl = body.querySelector<HTMLElement>("[data-app-update-badge]");
  const statusEl = body.querySelector<HTMLElement>("[data-app-update-status]");
  const hintEl = body.querySelector<HTMLElement>("[data-app-update-hint]");
  const progressEl = body.querySelector<HTMLElement>("[data-app-update-progress]");
  const progressFillEl = body.querySelector<HTMLElement>("[data-app-update-progress-fill]");
  const actionBtn = body.querySelector<HTMLButtonElement>("[data-app-update-action]");
  void getAppVersion().then((v) => {
    if (versionEl) versionEl.textContent = `v${v}`;
  });
  const paintAppUpdate = () => {
    const s = getUpdateState();
    if (badgeEl) {
      if (s.status === "available" || s.status === "downloading") {
        badgeEl.hidden = false;
        badgeEl.textContent = `新版本 v${s.version}`;
      } else if (s.status === "ready") {
        badgeEl.hidden = false;
        badgeEl.textContent = "待重启生效";
      } else if (s.status === "latest") {
        badgeEl.hidden = false;
        badgeEl.textContent = "已是最新";
      } else {
        badgeEl.hidden = true;
      }
    }
    if (statusEl) {
      const isError = s.status === "error";
      statusEl.classList.toggle("is-error", isError);
      statusEl.textContent = isError ? (s.message ?? "检查失败") : "";
    }
    if (progressEl && progressFillEl) {
      progressEl.hidden = s.status !== "downloading";
      progressFillEl.style.width = `${s.progress}%`;
    }
    if (!actionBtn) return;
    actionBtn.disabled = s.status === "checking" || s.status === "downloading";
    let icon = "refresh";
    let label = "检查更新";
    if (s.status === "checking") {
      label = "检查中…";
    } else if (s.status === "available") {
      icon = "download";
      label = `下载并安装 v${s.version}`;
    } else if (s.status === "downloading") {
      icon = "download";
      label = `下载中 ${s.progress}%`;
    } else if (s.status === "ready") {
      label = "重启应用";
    }
    actionBtn.innerHTML = `<span data-icon="${icon}" data-size="14"></span>${escapeHtml(label)}`;
    paintIcons(actionBtn);
  };
  paintAppUpdate();
  const unsubscribeAppUpdate = onUpdateState(paintAppUpdate);
  // 离开设置页时退订。#view-mount 上的 cleanup 事件不冒泡,document 捕获阶段接得住。
  document.addEventListener("cleanup", () => unsubscribeAppUpdate(), { once: true, capture: true });
  if (hintEl && !UPDATER_ENABLED) hintEl.textContent = UPDATER_DISABLED_REASON;
  actionBtn?.addEventListener("click", () => {
    const s = getUpdateState();
    if (s.status === "ready") void relaunchToUpdate();
    else if (s.status === "available") void downloadAndInstallUpdate();
    else if (s.status !== "checking" && s.status !== "downloading") void checkForAppUpdate({ silent: false });
  });
}

/**
 * Render the settings shell.
 *
 * `renderers` is intentionally optional: the page is useful as a navigable
 * shell today, while callers can inject concrete section content incrementally
 * without changing the sidebar or route handling.
 */
export function renderSettings(
  mount: HTMLElement,
  sectionOrOptions: SettingsSectionId | SettingsRenderOptions = "general",
): void {
  const options: SettingsRenderOptions =
    typeof sectionOrOptions === "string" ? { section: sectionOrOptions } : sectionOrOptions;
  let activeSection: SettingsSectionId = isSettingsSection(options.section) ? options.section : "general";

  mount.innerHTML = "";
  const view = document.createElement("div");
  view.className = "settings-page";
  mount.append(view);

  const sidebar = document.createElement("aside");
  sidebar.className = "settings-page__sidebar";
  sidebar.innerHTML = `
    <button class="settings-page__back btn btn--ghost btn--sm" data-settings-back>
      <span data-icon="arrowLeft" data-size="14"></span>
      <span>返回应用</span>
    </button>
    <div class="settings-page__heading">
      <div class="kicker">偏好设置</div>
      <h1 class="settings-page__title">设置</h1>
    </div>
    <nav class="settings-page__nav" aria-label="设置分类">
      ${Object.entries(SECTION_META).map(([id, meta]) => `
        <button class="settings-page__nav-item" data-settings-section="${id}" type="button">
          <span class="settings-page__nav-icon" data-icon="${meta.icon}" data-size="15"></span>
          <span class="settings-page__nav-label">${meta.label}</span>
        </button>
      `).join("")}
    </nav>
  `;

  const content = document.createElement("section");
  content.className = "settings-page__content";
  const body = document.createElement("div");
  body.className = "settings-page__body";
  content.append(body);

  view.append(sidebar, content);

  const navigate = (next: SettingsSectionId) => {
    if (next === activeSection) return;
    location.hash = `#/settings/${next}`;
  };

  function paintSection(): void {
    sidebar.querySelectorAll<HTMLButtonElement>("[data-settings-section]").forEach((button) => {
      button.toggleAttribute("data-active", button.dataset.settingsSection === activeSection);
    });
    body.dataset.section = activeSection;
    body.innerHTML = "";
    const renderer = options.renderers?.[activeSection] ?? defaultSectionRenderer;
    renderer({ section: activeSection, body, navigate });
    paintIcons(view);
  }

  sidebar.querySelector<HTMLButtonElement>("[data-settings-back]")?.addEventListener("click", () => {
    location.hash = "#/library";
  });
  sidebar.querySelectorAll<HTMLButtonElement>("[data-settings-section]").forEach((button) => {
    button.addEventListener("click", () => {
      const next = button.dataset.settingsSection;
      if (!isSettingsSection(next)) return;
      navigate(next);
    });
  });

  paintSection();
}

// Add picker — step 1 of the "add source" flow. Triggered by the global "+"
// button in the nav rail. Presents a type chooser (folder vs GitHub) and an
// address form. On submit, registers the source (with empty auto-sync targets)
// and defers to post-add.ts for the step-2 modal (agents + mode), which fires
// after the background scan completes. If the address matches a source that is
// already registered, registration is skipped and step 2 opens for the
// existing source instead (pending its in-flight clone/scan if still running).

import { api } from "./api";
import { store, tildePath } from "./store";
import { openModal } from "./modal";
import { paintIcons } from "./icon";
import { toast } from "./toast";
import { markPendingPostAdd, openPostAddForSource } from "./post-add";
import type { Source } from "./types";

/** Derive a display name from a path or URL: the last segment after trimming
 *  trailing slashes. e.g. "~/projects/my-skills" → "my-skills",
 *  "https://github.com/foo/bar" → "bar". */
function defaultName(pathOrUrl: string): string {
  const trimmed = pathOrUrl.replace(/\/+$/, "").replace(/\.git$/, "");
  const parts = trimmed.split("/");
  return parts[parts.length - 1] || pathOrUrl;
}

/** 拖入的文件读成 base64（dragDropEnabled=false 时 HTML5 drop 拿不到磁盘路径，
 *  只能把字节交给后端落临时 zip 再走统一导入）。 */
function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const bytes = new Uint8Array(reader.result as ArrayBuffer);
      let bin = "";
      const CHUNK = 0x8000;
      for (let i = 0; i < bytes.length; i += CHUNK) {
        bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
      }
      resolve(btoa(bin));
    };
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(file);
  });
}

export function openAddPicker(): void {
  const body = document.createElement("div");
  body.style.cssText = "display:flex;flex-direction:column;gap:18px";

  // --- Type chooser ---
  const typeSection = document.createElement("div");
  typeSection.innerHTML = `<div class="col" style="gap:10px" data-type-list></div>`;
  const typeList = typeSection.querySelector("[data-type-list]")!;
  type TypeKind = "local" | "github" | "zip";
  const types: { value: TypeKind; title: string; desc: string; icon: string }[] = [
    { value: "local", title: "本地文件夹", desc: "指向磁盘上的目录，递归扫描 SKILL.md。", icon: "folder" },
    { value: "github", title: "Git 仓库", desc: "克隆到本地后扫描，支持后续拉取更新。", icon: "git" },
    { value: "zip", title: "压缩包", desc: "导入 .zip / .skill 归档；安全解压到数据目录后作为本地目录管理。", icon: "package" },
  ];
  let chosenType: TypeKind = "local";
  // 本地文件夹的导入方式：原地引用（现状）或拷贝一份进中央仓库。
  let localMode: "inplace" | "copy" = "inplace";
  const typeItems: Record<TypeKind, HTMLDivElement> = {} as any;
  const applyTypeVisual = (val: TypeKind) => {
    for (const el of Object.values(typeItems)) {
      const active = el.dataset.value === val;
      el.style.borderColor = active ? "var(--accent)" : "var(--border)";
      el.style.background = active ? "var(--accent-soft)" : "var(--bg)";
      el.style.boxShadow = active ? "var(--shadow-glow)" : "none";
      const iconEl = el.querySelector<HTMLElement>(".type-icon")!;
      iconEl.style.color = active ? "var(--accent)" : "var(--ink-dim)";
      const checkEl = el.querySelector<HTMLElement>(".type-check")!;
      checkEl.style.opacity = active ? "1" : "0";
    }
  };
  for (const t of types) {
    const item = document.createElement("div");
    item.dataset.value = t.value;
    // role=button 让 AX/键盘能触达（和下方导入方式卡片一致），否则辅助工具切不了类型
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
      <span class="icon type-icon" data-icon="${t.icon}" data-size="18" style="margin-top:1px;color:var(--ink-dim);transition:color var(--t-fast)"></span>
      <div style="flex:1;min-width:0">
        <div style="font-family:var(--font-display);color:var(--ink);font-size:15px;font-weight:600">${t.title}</div>
        <div style="font-size:12px;color:var(--ink-mute);margin-top:4px;line-height:1.5">${t.desc}</div>
      </div>
      <span class="icon type-check" data-icon="check" data-size="14" style="width:20px;height:20px;display:grid;place-items:center;background:var(--accent);color:#fff;border-radius:50%;opacity:0;transition:opacity var(--t-fast)"></span>
    `;
    item.addEventListener("click", () => {
      if (chosenType === t.value) return;
      chosenType = t.value;
      applyTypeVisual(chosenType);
      renderFields();
    });
    typeItems[t.value] = item;
    typeList.append(item);
  }
  paintIcons(typeSection);
  applyTypeVisual(chosenType);
  body.append(typeSection);

  // --- Dynamic fields container (changes based on type) ---
  const fields = document.createElement("div");
  fields.style.cssText = "display:flex;flex-direction:column;gap:14px";
  body.append(fields);

  function renderFields() {
    fields.innerHTML = "";
    if (chosenType === "local") {
      fields.innerHTML = `
        <div class="field">
          <label class="field__label">文件夹路径</label>
          <div class="row" style="gap:6px">
            <input class="input" data-input="local-path" placeholder="~/projects/my-skills" />
            <button type="button" class="btn btn--ghost" data-action="pick"><span data-icon="folder" data-size="14"></span></button>
          </div>
          <div class="field__hint">提示：试试 <code>~/.claude/skills</code> 直接导入现有 skill 库。</div>
        </div>
        <div class="field">
          <label class="field__label">导入方式</label>
          <div class="row" style="gap:8px;align-items:stretch" data-import-mode>
            <div class="mode-card" data-mode="inplace" role="button" tabindex="0">
              <span class="icon mode-icon" data-icon="link" data-size="16"></span>
              <div class="mode-body">
                <strong>直接使用原文件夹</strong>
                <span>原地引用，原目录的改动实时同步</span>
              </div>
              <span class="icon mode-check" data-icon="check" data-size="12"></span>
            </div>
            <div class="mode-card" data-mode="copy" role="button" tabindex="0">
              <span class="icon mode-icon" data-icon="library" data-size="16"></span>
              <div class="mode-body">
                <strong>拷贝到中央仓库</strong>
                <span>复制一份进仓库，原文件夹不受影响</span>
              </div>
              <span class="icon mode-check" data-icon="check" data-size="12"></span>
            </div>
          </div>
          <div class="field__hint" data-mode-hint></div>
        </div>
        <div class="field">
          <label class="field__label">显示名称</label>
          <input class="input" data-input="name" placeholder="自动取文件夹名" />
        </div>
      `;
      const modeHint = (): string =>
        localMode === "copy"
          ? "把内容复制一份进中央仓库（设置的数据目录），之后由仓库统一管理。"
          : "来源直接指向该文件夹，文件增删改都会自动重扫进技能库。";
      const paintMode = () => {
        for (const card of fields.querySelectorAll<HTMLElement>("[data-import-mode] > [data-mode]")) {
          const active = card.dataset.mode === localMode;
          card.style.borderColor = active ? "var(--accent)" : "var(--border)";
          card.style.background = active ? "var(--accent-soft)" : "var(--bg)";
          const icon = card.querySelector<HTMLElement>(".mode-icon")!;
          icon.style.color = active ? "var(--accent)" : "var(--ink-dim)";
          const check = card.querySelector<HTMLElement>(".mode-check")!;
          check.style.opacity = active ? "1" : "0";
        }
        const hint = fields.querySelector<HTMLElement>("[data-mode-hint]");
        if (hint) hint.textContent = modeHint();
      };
      for (const card of fields.querySelectorAll<HTMLElement>("[data-import-mode] > [data-mode]")) {
        card.addEventListener("click", () => {
          if (localMode === card.dataset.mode) return;
          localMode = card.dataset.mode as "inplace" | "copy";
          paintMode();
        });
      }
      paintMode();
    } else if (chosenType === "zip") {
      fields.innerHTML = `
        <div class="add-dropzone" data-drop="zip" role="button" tabindex="0">
          <span data-icon="package" data-size="20"></span>
          <div class="add-dropzone__text">
            <strong>拖拽 .zip / .skill 文件到这里</strong>
            <span>松手即导入；也可以点击选择文件或在下方填路径</span>
          </div>
        </div>
        <div class="field">
          <label class="field__label">压缩包路径</label>
          <div class="row" style="gap:6px">
            <input class="input" data-input="zip-path" placeholder="~/Downloads/pack.zip 或 .skill" />
            <button type="button" class="btn btn--ghost" data-action="pick-file"><span data-icon="file" data-size="14"></span></button>
          </div>
          <div class="field__hint">解压前会做路径校验（防 zip-slip），包内符号链接一律跳过。</div>
        </div>
        <div class="field">
          <label class="field__label">显示名称</label>
          <input class="input" data-input="name" placeholder="自动取文件名" />
        </div>
      `;
    } else {
      fields.innerHTML = `
        <div class="field">
          <label class="field__label">仓库地址</label>
          <input class="input" data-input="gh-url" placeholder="https://github.com/anthropics/skills" />
        </div>
        <div class="row" style="gap:var(--s-3)">
          <div class="field" style="flex:1">
            <label class="field__label">显示名称</label>
            <input class="input" data-input="name" placeholder="自动取仓库名" />
          </div>
          <div class="field" style="width:120px">
            <label class="field__label">分支</label>
            <input class="input" data-input="gh-branch" placeholder="main" />
          </div>
        </div>
      `;
    }
    paintIcons(fields);
    // Auto-fill the display name from the path/URL on blur. Fills when the
    // name is empty, and REPLACES a value we auto-filled earlier (e.g. the
    // user fixed a typo in the URL) — never overwrites a hand-typed name.
    const autoFillName = (srcInput: HTMLInputElement) => {
      const nameInput = fields.querySelector<HTMLInputElement>("[data-input='name']");
      if (!nameInput || !srcInput.value.trim()) return;
      if (!nameInput.value.trim() || nameInput.dataset.autofilled === "1") {
        nameInput.value = defaultName(srcInput.value.trim());
        nameInput.dataset.autofilled = "1";
      }
    };
    fields.querySelector<HTMLInputElement>("[data-input='name']")?.addEventListener("input", (e) => {
      delete (e.target as HTMLInputElement).dataset.autofilled;
    });
    const pathInput = fields.querySelector<HTMLInputElement>("[data-input='local-path']");
    pathInput?.addEventListener("blur", () => autoFillName(pathInput));
    const urlInput = fields.querySelector<HTMLInputElement>("[data-input='gh-url']");
    urlInput?.addEventListener("blur", () => autoFillName(urlInput));
    // Wire the folder picker button.
    fields.querySelector("[data-action='pick']")?.addEventListener("click", async () => {
      try {
        const picked = await api.pickFolder();
        if (picked) {
          const input = fields.querySelector<HTMLInputElement>("[data-input='local-path']")!;
          input.value = picked;
          autoFillName(input);
        }
      } catch (e) { toast(String(e), "error"); }
    });
    // Wire the archive file picker button.
    fields.querySelector("[data-action='pick-file']")?.addEventListener("click", async () => {
      try {
        const picked = await api.pickArchiveFile();
        if (picked) {
          const input = fields.querySelector<HTMLInputElement>("[data-input='zip-path']")!;
          input.value = picked;
          autoFillName(input);
        }
      } catch (e) { toast(String(e), "error"); }
    });
    const zipInput = fields.querySelector<HTMLInputElement>("[data-input='zip-path']");
    zipInput?.addEventListener("blur", () => autoFillName(zipInput));

    // 拖拽区：dragover 要 preventDefault 才允许 drop；drop 里拿文件字节直接导入。
    const dropzone = fields.querySelector<HTMLElement>("[data-drop='zip']");
    if (dropzone) {
      const importDropped = async (file: File) => {
        if (!/\.(zip|skill)$/i.test(file.name)) {
          toast("只支持 .zip / .skill 压缩包", "error");
          return;
        }
        dropzone.classList.add("is-busy");
        try {
          const dataB64 = await fileToBase64(file);
          const typed = fields.querySelector<HTMLInputElement>("[data-input='name']")?.value.trim() ?? "";
          await importZipAndContinue(() => api.addZipBytes(typed, file.name, dataB64));
        } catch (err) {
          toast(`添加失败：${err}`, "error");
        } finally {
          dropzone.classList.remove("is-busy");
        }
      };
      dropzone.addEventListener("dragover", (e) => {
        e.preventDefault();
        dropzone.classList.add("is-dragover");
      });
      dropzone.addEventListener("dragleave", () => dropzone.classList.remove("is-dragover"));
      dropzone.addEventListener("drop", (e) => {
        e.preventDefault();
        dropzone.classList.remove("is-dragover");
        const file = e.dataTransfer?.files?.[0];
        if (!file) { toast("没有识别到拖入的文件", "error"); return; }
        void importDropped(file);
      });
      // 点击拖拽区 = 打开文件选择框（复用下面的 pick-file 按钮）。
      dropzone.addEventListener("click", () => {
        fields.querySelector<HTMLButtonElement>("[data-action='pick-file']")?.click();
      });
    }
  }
  renderFields();

  /** zip 导入的公共续接：注册完成后挂起待配置，等扫描事件弹第二步。 */
  async function importZipAndContinue(run: () => Promise<Source>): Promise<void> {
    const src = await run();
    markPendingPostAdd(src.id, "local"); // 解压落地为本地目录，可监听
    await store.refresh();
    toast("解压完成，扫描中…", "info");
    modal.close();
  }

  // --- Footer ---
  const footer = document.createElement("div");
  footer.style.cssText = "display:flex;gap:8px;justify-content:flex-end;align-items:center;flex:1";
  footer.innerHTML = `
    <button class="btn btn--ghost" data-act="cancel">取消</button>
    <button class="btn btn--primary" data-act="next">下一步</button>
  `;

  const modal = openModal({ title: "添加来源", body, footer });
  paintIcons(footer);

  /** 已存在的来源不重复注册，直接进第二步。克隆/扫描还没落地的（GitHub
   *  正在克隆）挂起等 source-scanned 事件再弹配置；已就绪的立即弹出。
   *  返回 false = 来源刚被删掉（极端竞态），调用方继续走正常新增。 */
  async function continueToPostAdd(existing: Source): Promise<boolean> {
    if (existing.clone_status === "cloning" || existing.clone_status === "pending") {
      markPendingPostAdd(existing.id, existing.kind);
      toast("该来源已存在，克隆完成后继续配置", "info");
      modal.close();
      return true;
    }
    const ok = await openPostAddForSource(existing.id);
    if (ok) modal.close();
    return ok;
  }

  footer.querySelector("[data-act='cancel']")!.addEventListener("click", () => modal.close());
  footer.querySelector("[data-act='next']")!.addEventListener("click", async () => {
    const btn = footer.querySelector("[data-act='next']") as HTMLButtonElement;
    if (btn.disabled) return;
    btn.disabled = true;

    const nameInput = fields.querySelector<HTMLInputElement>("[data-input='name']");
    let name = (nameInput?.value || "").trim();

    try {
      if (chosenType === "local") {
        const path = (fields.querySelector<HTMLInputElement>("[data-input='local-path']")!.value || "").trim();
        if (!path) { toast("请填写文件夹路径", "error"); btn.disabled = false; return; }
        if (!name) name = defaultName(path);
        if (localMode === "copy") {
          // 拷贝模式：内容进中央仓库，来源注册的是仓库而不是原路径，
          // 所以不做同路径来源去重（原文件夹很可能本来就是个来源）。
          btn.textContent = "拷贝中…";
          try {
            const src = await api.addLocalSource(name, path, "flat", [], true);
            markPendingPostAdd(src.id, "local");
            await store.refresh();
            toast("已拷贝到中央仓库，扫描中…", "info");
            modal.close();
          } finally {
            btn.textContent = "下一步";
          }
          return;
        }
        // Same-folder re-add: skip registration, jump straight to step 2.
        const existing = store.get().sources.find(
          (s) => s.kind === "local" && (s.location === path || tildePath(s.location) === path),
        );
        if (existing && (await continueToPostAdd(existing))) return;
        // Register with empty auto-sync targets — step 2 (post-add) sets them.
        const src = await api.addLocalSource(name, path, "flat", []);
        markPendingPostAdd(src.id, "local");
        await store.refresh();
        toast("扫描中…", "info");
        modal.close();
      } else if (chosenType === "github") {
        const url = (fields.querySelector<HTMLInputElement>("[data-input='gh-url']")!.value || "").trim();
        const branch = (fields.querySelector<HTMLInputElement>("[data-input='gh-branch']")?.value || "").trim();
        if (!url) { toast("请填写仓库地址", "error"); btn.disabled = false; return; }
        if (!name) name = defaultName(url);
        // Same-URL re-add: skip registration, jump straight to step 2.
        const existing = store.get().sources.find(
          (s) => s.kind === "github" && s.location === url,
        );
        if (existing && (await continueToPostAdd(existing))) return;
        const src = await api.addGithubSource(name, url, "flat", branch || null);
        markPendingPostAdd(src.id, "github");
        await store.refresh();
        toast("正在后台克隆，请稍候", "info");
        modal.close();
      } else if (chosenType === "zip") {
        const path = (fields.querySelector<HTMLInputElement>("[data-input='zip-path']")!.value || "").trim();
        if (!path) { toast("请选择或填写压缩包路径", "error"); btn.disabled = false; return; }
        if (!name) name = defaultName(path);
        btn.textContent = "解压中…";
        try {
          await importZipAndContinue(() => api.addZipSource(name, path));
        } finally {
          btn.textContent = "下一步";
        }
      }
    } catch (err) {
      toast(`添加失败：${err}`, "error");
      btn.disabled = false;
    }
  });
}

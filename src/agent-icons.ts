// Agent brand icons are bundled under public/agent-icons so the desktop app
// remains self-contained and works offline. Keys are kept separate from file
// names because our target ids use a few legacy aliases (for example
// `claude-code` and `kimi-code`).

function escapeAttribute(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const AGENT_ICON_FILES: Record<string, string> = {
  adal: "adal.png",
  amp: "amp.svg",
  antigravity: "antigravity.png",
  augment: "augment.svg",
  bob: "bob.png",
  "claude-code": "claude_code.svg",
  claude_code: "claude_code.svg",
  cline: "cline.png",
  codebuddy: "codebuddy.svg",
  codex: "codex.svg",
  command_code: "command_code.svg",
  continue: "continue.png",
  cortex: "cortex.png",
  crush: "crush.png",
  cursor: "cursor.png",
  deepagents: "deepagents.png",
  deepseek_harness: "deepseek_harness.svg",
  "deepseek-harness": "deepseek_harness.svg",
  droid: "droid.svg",
  firebender: "firebender.svg",
  gemini_cli: "gemini_cli.svg",
  github_copilot: "github_copilot.png",
  goose: "goose.png",
  grok: "grok.svg",
  hermes: "hermes.png",
  iflow: "iflow.png",
  junie: "junie.png",
  kilo_code: "kilo_code.svg",
  kimi: "kimi.svg",
  "kimi-code": "kimi.svg",
  kiro: "kiro.svg",
  kode: "kode.png",
  mcpjam: "mcpjam.png",
  minimax: "minimax-mark.svg",
  minimax_builtin: "minimax-mark.svg",
  "minimax-builtin": "minimax-mark.svg",
  mistral_vibe: "mistral_vibe.svg",
  mux: "mux.png",
  neovate: "neovate.png",
  openclaw: "openclaw.svg",
  opencode: "opencode.png",
  openhands: "openhands.png",
  pi: "pi.svg",
  pochi: "pochi.png",
  qoder: "qoder.svg",
  qoderwork: "qoder.svg",
  qwen: "qwen_code.png",
  qwen_code: "qwen_code.png",
  replit: "replit.png",
  roo_code: "roo_code.svg",
  // The source Trae files are 140×34 wordmarks. Use the square mark for the
  // 22px Agent badge; the full wordmark remains bundled for future use.
  trae: "trae-mark.svg",
  trae_cn: "trae-mark.svg",
  warp: "warp.svg",
  windsurf: "windsurf.svg",
  workbuddy: "workbuddy.png",
  zcode: "zcode.svg",
  zencoder: "zencoder.png",
};

// These source assets are monochrome black marks and need inversion on the
// dark Field Notebook theme. Other SVGs carry their own adaptive colors.
const DARK_INVERT_ICON_KEYS = new Set(["codex", "roo_code"]);

export function getAgentIconSrc(agentKey: string): string | null {
  const file = AGENT_ICON_FILES[agentKey];
  return file ? `/agent-icons/${file}` : null;
}

export function agentIconNeedsDarkInvert(agentKey: string): boolean {
  return DARK_INVERT_ICON_KEYS.has(agentKey);
}

export function agentIconMarkup(agentKey: string, displayName: string, className = ""): string {
  const initial = (displayName.trim() || agentKey || "?").slice(0, 1).toUpperCase();
  return `<span class="agent-icon ${escapeAttribute(className)}" data-agent-key="${escapeAttribute(agentKey)}" title="${escapeAttribute(displayName)}" aria-hidden="true"><span class="agent-icon__fallback">${escapeAttribute(initial)}</span></span>`;
}

/** Hydrate static Agent icon placeholders after a view has been rendered. */
export function paintAgentIcons(root: ParentNode = document): void {
  for (const el of root.querySelectorAll<HTMLElement>(".agent-icon[data-agent-key]:not([data-agent-painted])")) {
    el.dataset.agentPainted = "true";
    const key = el.dataset.agentKey || "";
    const src = getAgentIconSrc(key);
    const fallback = el.querySelector<HTMLElement>(".agent-icon__fallback");
    if (!src || !fallback) continue;

    const img = document.createElement("img");
    img.src = src;
    img.alt = "";
    img.draggable = false;
    if (agentIconNeedsDarkInvert(key)) img.classList.add("agent-icon__image--dark-invert");
    img.addEventListener("error", () => {
      img.remove();
      fallback.hidden = false;
    }, { once: true });
    fallback.hidden = true;
    el.prepend(img);
  }
}

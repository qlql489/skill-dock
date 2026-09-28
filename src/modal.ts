// Modal utility — open/close a content panel in #modal-mount.

import { paintIcons } from "./icon";

interface ModalOptions {
  title: string;
  body: HTMLElement;
  footer?: HTMLElement;
  onClose?: () => void;
  width?: number;
}

let current: { close: () => void } | null = null;

export function openModal(opts: ModalOptions): { close: () => void } {
  closeModal();

  const mount = document.getElementById("modal-mount")!;
  mount.hidden = false;

  const modal = document.createElement("div");
  modal.className = "modal";
  if (opts.width) modal.style.width = `${opts.width}px`;

  const head = document.createElement("div");
  head.className = "modal__head";
  const title = document.createElement("h2");
  title.className = "modal__title";
  title.textContent = opts.title;
  const closeBtn = document.createElement("button");
  closeBtn.className = "btn btn--ghost btn--icon";
  closeBtn.setAttribute("aria-label", "关闭");
  const closeIcon = document.createElement("span");
  closeIcon.setAttribute("data-icon", "close");
  closeIcon.setAttribute("data-size", "16");
  closeBtn.appendChild(closeIcon);
  head.append(title, closeBtn);

  const body = document.createElement("div");
  body.className = "modal__body";
  body.append(opts.body);

  modal.append(head, body);
  if (opts.footer) {
    const foot = document.createElement("div");
    foot.className = "modal__foot";
    foot.append(opts.footer);
    modal.append(foot);
  }
  mount.append(modal);
  paintIcons(modal);

  const close = () => {
    if (current?.close !== close) return;
    current = null;
    mount.hidden = true;
    mount.innerHTML = "";
    opts.onClose?.();
  };
  closeBtn.onclick = close;
  mount.onclick = (e) => { if (e.target === mount) close(); };
  document.addEventListener("keydown", escClose);
  function escClose(e: KeyboardEvent) {
    if (e.key === "Escape") {
      close();
      document.removeEventListener("keydown", escClose);
    }
  }
  current = { close };
  return { close };
}

export function closeModal() {
  current?.close();
}

// Lightweight toast system.

type ToastKind = "info" | "success" | "error";

interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
}

let counter = 0;

function ensureStack(): HTMLElement {
  let el = document.getElementById("toast-stack");
  if (!el) {
    el = document.createElement("div");
    el.id = "toast-stack";
    el.className = "toast-stack";
    document.body.appendChild(el);
  }
  return el;
}

export function toast(message: string, kind: ToastKind = "info", ttl = 3200): void {
  const stack = ensureStack();
  const id = ++counter;
  const item: ToastItem = { id, kind, message };
  const el = document.createElement("div");
  el.className = `toast toast--${kind}`;
  el.textContent = message;
  stack.appendChild(el);
  setTimeout(() => {
    el.style.transition = "opacity 200ms, transform 200ms";
    el.style.opacity = "0";
    el.style.transform = "translateX(20px)";
    setTimeout(() => el.remove(), 220);
  }, ttl);
}

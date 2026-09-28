// Icon helper — wraps lucide icons as inline SVGs.
// Renders 24×24 stroke icons sized via data-size. Lucide icon objects are
// arrays of [tag, attrs] pairs (same shape as the old @hugeicons), so the
// rendering loop is unchanged.

import {
  Library, Folder, FolderOpen, Target, RefreshCw, Search,
  Plus, X, ExternalLink, Save, Pencil, Eye, Trash2, Settings,
  Filter, ArrowUpDown, AlertCircle, Check, Link, Unlink, Download,
  FileText, LayoutGrid, GitBranch, MoreHorizontal, ArrowRight, ArrowLeft,
  Terminal, Sparkles, Globe, Book, Package, Layers, ChartColumn, Code,
  ChevronRight, List, GripVertical, Upload,
} from "lucide";
import { paintAgentIcons } from "./agent-icons";

type IconPath = readonly [string, Record<string, string | number | undefined>];
export type IconObject = readonly IconPath[];

const ICONS: Record<string, IconObject> = {
  library: Library,
  folder: Folder,
  folderOpen: FolderOpen,
  target: Target,
  refresh: RefreshCw,
  search: Search,
  plus: Plus,
  add: Plus,
  close: X,
  cancel: X,
  external: ExternalLink,
  save: Save,
  edit: Pencil,
  view: Eye,
  delete: Trash2,
  settings: Settings,
  filter: Filter,
  sort: ArrowUpDown,
  alert: AlertCircle,
  warn: AlertCircle,
  check: Check,
  link: Link,
  unlink: Unlink,
  download: Download,
  upload: Upload,
  file: FileText,
  grid: LayoutGrid,
  list: List,
  git: GitBranch,
  more: MoreHorizontal,
  arrowRight: ArrowRight,
  arrowLeft: ArrowLeft,
  terminal: Terminal,
  sparkles: Sparkles,
  globe: Globe,
  book: Book,
  package: Package,
  layers: Layers,
  chart: ChartColumn,
  source: Code,
  chevronRight: ChevronRight,
  gripVertical: GripVertical,
  grip: GripVertical,
};

export function icon(ico: IconObject, size = 18, strokeWidth = 1.75): SVGElement {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", String(strokeWidth));
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("icon");

  for (const [tag, props] of ico) {
    const el = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(props)) {
      if (k === "key") continue;
      const attr = k.replace(/[A-Z]/g, (m) => "-" + m.toLowerCase());
      el.setAttribute(attr, String(v));
    }
    svg.appendChild(el);
  }
  return svg;
}

/** Scans the document for `[data-icon="name"]` elements and replaces them with
 *  rendered SVGs. Idempotent: skips already-painted elements. Call this after
 *  any view re-render. */
export function paintIcons(root: ParentNode = document) {
  for (const el of root.querySelectorAll<HTMLElement>("[data-icon]")) {
    if (el.firstChild) continue;
    const name = el.dataset.icon!;
    const ico = ICONS[name];
    if (!ico) continue;
    const size = Number(el.dataset.size || 16);
    el.appendChild(icon(ico, size, 1.75));
  }
  paintAgentIcons(root);
}

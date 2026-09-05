/**
 * The handful of UI primitives the app doesn't have.
 *
 * `src/main.ts` and `src/review.ts` build HTML strings with template
 * literals, assign them to `innerHTML`, and delegate events from a stable
 * parent using `closest()` + `data-*`. The portal keeps that pattern —
 * one codebase, one idiom — and adds only what it genuinely needs: a
 * table renderer, a JSON view, a confirm dialog, and toasts. There is no
 * component framework here on purpose.
 */
import { escapeHtml } from "../shared/html";

export { escapeHtml };

export function h(html: string): string {
  return html;
}

/** Sets a container's contents and returns it, for chaining. */
export function render(el: HTMLElement, html: string): HTMLElement {
  el.innerHTML = html;
  return el;
}

/** `rawTitle` for titles that are literal system strings — a path, a
 *  command name — which the default uppercase treatment would mangle. */
export function card(title: string, body: string, extraClass = "", rawTitle = false): string {
  return `<section class="card ${extraClass}">
    ${title ? `<h2 class="${rawTitle ? "raw" : ""}">${escapeHtml(title)}</h2>` : ""}
    ${body}
  </section>`;
}

export function panelHead(title: string, description: string): string {
  return `<div class="panel-head">
    <h1>${escapeHtml(title)}</h1>
    <p>${escapeHtml(description)}</p>
  </div>`;
}

export function kv(pairs: Array<[string, string | null | undefined]>, plain = false): string {
  return `<dl class="kv">${pairs
    .map(
      ([k, v]) =>
        `<dt>${escapeHtml(k)}</dt><dd class="${plain ? "plain" : ""}">${
          v === null || v === undefined || v === "" ? `<span class="hint">—</span>` : escapeHtml(String(v))
        }</dd>`,
    )
    .join("")}</dl>`;
}

export type PillTone = "" | "ok" | "warn" | "danger";

export function pill(label: string, value: string, tone: PillTone = "", live = false): string {
  const cls = ["pill", tone ? `pill-${tone}` : "", live ? "pill-live" : ""]
    .filter(Boolean)
    .join(" ");
  return `<span class="${cls}">${escapeHtml(label)} <b>${escapeHtml(value)}</b></span>`;
}

/** Pretty-prints any value. Errors render in the danger style. */
export function output(value: unknown, isError = false): string {
  const text =
    typeof value === "string" ? value : JSON.stringify(value, null, 2) ?? String(value);
  return `<pre class="output${isError ? " output-error" : ""}">${escapeHtml(text)}</pre>`;
}

export interface TableOptions {
  /** Marks rows clickable and stamps `data-row-index` for delegation. */
  clickable?: boolean;
  /** Right-aligns these columns and renders them tabular-nums. */
  numericColumns?: Set<string>;
  emptyMessage?: string;
}

export function table(
  columns: string[],
  rows: unknown[][],
  options: TableOptions = {},
): string {
  if (rows.length === 0) {
    return `<p class="hint">${escapeHtml(options.emptyMessage ?? "No rows.")}</p>`;
  }
  const numeric = options.numericColumns ?? new Set<string>();

  const head = columns.map((c) => `<th>${escapeHtml(c)}</th>`).join("");
  const body = rows
    .map((row, i) => {
      const cells = row
        .map((cell, j) => {
          const isNum = numeric.has(columns[j]);
          if (cell === null || cell === undefined) {
            return `<td class="null">null</td>`;
          }
          const text = typeof cell === "object" ? JSON.stringify(cell) : String(cell);
          return `<td class="${isNum ? "num" : ""}" title="${escapeHtml(text)}">${escapeHtml(text)}</td>`;
        })
        .join("");
      return `<tr class="${options.clickable ? "clickable" : ""}" data-row-index="${i}">${cells}</tr>`;
    })
    .join("");

  return `<div class="table-wrap"><table class="grid">
    <thead><tr>${head}</tr></thead>
    <tbody>${body}</tbody>
  </table></div>`;
}

export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  const gb = 1024 ** 3;
  const mb = 1024 ** 2;
  if (Math.abs(n) >= gb) return `${(n / gb).toFixed(2)} GB`;
  if (Math.abs(n) >= mb) return `${(n / mb).toFixed(1)} MB`;
  if (Math.abs(n) >= 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${n} B`;
}

export function duration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return "—";
  const total = Math.max(0, Math.round(seconds));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function timestamp(millis: number): string {
  return new Date(millis).toLocaleString();
}

export function clockTime(millis: number): string {
  return new Date(millis).toLocaleTimeString(undefined, { hour12: false });
}

/**
 * Where overlays mount.
 *
 * Not `document.body`: every token (`--surface`, `--border`, …) and every
 * control style is scoped to `.dev-root`, so a modal or toast parented to
 * the body renders with a transparent background and unstyled buttons.
 * `position: fixed` still resolves against the viewport from in here —
 * `.dev-root` is a plain grid with no transform or filter — so nothing is
 * lost by nesting them.
 */
function overlayRoot(): HTMLElement {
  return document.querySelector<HTMLElement>(".dev-root") ?? document.body;
}

// --- Toasts --------------------------------------------------------

let toastStack: HTMLElement | null = null;

export function toast(message: string, tone: "" | "ok" | "err" = "") {
  if (!toastStack || !toastStack.isConnected) {
    toastStack = document.createElement("div");
    toastStack.className = "toast-stack";
    overlayRoot().appendChild(toastStack);
  }
  const el = document.createElement("div");
  el.className = `toast ${tone}`;
  el.textContent = message;
  toastStack.appendChild(el);
  setTimeout(() => el.remove(), 5200);
}

// --- Confirm -------------------------------------------------------

export interface ConfirmOptions {
  title: string;
  body: string;
  confirmLabel?: string;
  /** When set, the button stays disabled until this exact text is typed.
   *  Reserved for the operations that cannot be undone. */
  typeToConfirm?: string;
}

export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "modal-backdrop";
    backdrop.innerHTML = `<div class="modal" role="dialog" aria-modal="true">
      <h2>${escapeHtml(options.title)}</h2>
      <p>${options.body}</p>
      ${
        options.typeToConfirm
          ? `<label class="field"><span>Type <code>${escapeHtml(
              options.typeToConfirm,
            )}</code> to confirm</span>
             <input type="text" data-confirm-input autocomplete="off" spellcheck="false" /></label>`
          : ""
      }
      <div class="row" style="margin-top:1rem">
        <button type="button" data-cancel>Cancel</button>
        <button type="button" class="danger" data-confirm ${
          options.typeToConfirm ? "disabled" : ""
        }>${escapeHtml(options.confirmLabel ?? "Confirm")}</button>
      </div>
    </div>`;

    const close = (result: boolean) => {
      document.removeEventListener("keydown", onKey);
      backdrop.remove();
      resolve(result);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(false);
    };

    const confirmBtn = backdrop.querySelector<HTMLButtonElement>("[data-confirm]")!;
    const input = backdrop.querySelector<HTMLInputElement>("[data-confirm-input]");
    input?.addEventListener("input", () => {
      confirmBtn.disabled = input.value !== options.typeToConfirm;
    });
    confirmBtn.addEventListener("click", () => close(true));
    backdrop.querySelector("[data-cancel]")!.addEventListener("click", () => close(false));
    backdrop.addEventListener("click", (e) => {
      if (e.target === backdrop) close(false);
    });
    document.addEventListener("keydown", onKey);

    overlayRoot().appendChild(backdrop);
    (input ?? confirmBtn).focus();
  });
}

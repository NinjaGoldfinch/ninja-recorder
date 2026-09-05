/**
 * Retention policy, with the dry run the app itself never offers.
 *
 * `set_retention_policy` saves and enforces in one step, deleting files as
 * a side effect with nothing shown first. `select_for_deletion` is pure
 * and takes an injected clock, so previewing — including at a fabricated
 * "now", to test an age rule without waiting days — costs nothing.
 */
import type { Panel, PanelContext } from "../main";
import { call, tryCall } from "../ipc";
import { bytes, card, confirmDialog, output, panelHead, table, timestamp, toast } from "../ui";
import type { DevHealth, RetentionPolicy, RetentionPreview } from "../types";

const GIB = 1024 ** 3;
const DAY = 24 * 60 * 60 * 1000;

let root: HTMLElement | null = null;
let ctx: PanelContext | null = null;
let policy: RetentionPolicy = { max_total_bytes: 50 * GIB, max_age_days: 30 };
let preview: RetentionPreview | null = null;
let previewError: string | null = null;
let ageOverrideDays = 0;

function draw() {
  if (!root) return;

  root.innerHTML =
    panelHead(
      "Retention",
      "What enforcement would delete, before it deletes it. Age is checked first, then total size; pinned recordings are exempt but still count toward the total.",
    ) +
    card(
      "Policy",
      `<div class="field-grid">
        <label class="field"><span>Max total size (GB)</span>
          <input type="number" id="max-gb" min="1" step="1"
            value="${policy.max_total_bytes === null ? "" : Math.round(policy.max_total_bytes / GIB)}"
            ${policy.max_total_bytes === null ? "disabled" : ""} />
        </label>
        <label class="field"><span>Max age (days)</span>
          <input type="number" id="max-days" min="1" step="1"
            value="${policy.max_age_days ?? ""}" ${policy.max_age_days === null ? "disabled" : ""} />
        </label>
      </div>
      <div class="row" style="margin-top:.6rem">
        <label class="check"><input type="checkbox" id="size-on"
          ${policy.max_total_bytes !== null ? "checked" : ""} /> enforce a size limit</label>
        <label class="check"><input type="checkbox" id="age-on"
          ${policy.max_age_days !== null ? "checked" : ""} /> enforce an age limit</label>
      </div>
      <div class="row" style="margin-top:.9rem">
        <button type="button" data-preview>Preview</button>
        <button type="button" class="danger" data-apply>Save and enforce</button>
        <span class="spacer"></span>
        <label class="field field-inline"><span>Preview as if it were</span>
          <input type="number" id="age-override" value="${ageOverrideDays}" style="width:5rem" />
          <span class="hint">days from now</span>
        </label>
      </div>
      <p class="hint-block">Saving enforces immediately — that is what the app's own retention
        form does, and it is why a preview exists. The clock override only affects the preview;
        it lets an age rule be checked without waiting for recordings to get old.</p>`,
    ) +
    (previewError ? card("Preview", output(previewError, true)) : "") +
    (preview
      ? card(
          "Dry run",
          `<div class="row" style="margin-bottom:.7rem">
            <span class="pill">now <b>${bytes(preview.total_bytes)}</b></span>
            <span class="pill">pinned <b>${bytes(preview.pinned_bytes)}</b></span>
            <span class="pill ${preview.to_delete.length ? "pill-warn" : "pill-ok"}">
              would delete <b>${preview.to_delete.length}</b></span>
            <span class="pill">would free <b>${bytes(preview.would_free_bytes)}</b></span>
            <span class="pill">after <b>${bytes(preview.total_after_bytes)}</b></span>
          </div>` +
            (preview.to_delete.length
              ? table(
                  ["id", "champion", "started", "size", "pinned", "file"],
                  preview.to_delete.map((r) => [
                    r.id,
                    r.champion ?? null,
                    timestamp(r.started_at),
                    bytes(r.size_bytes),
                    r.pinned ? "yes" : "no",
                    r.file_exists ? "on disk" : "already gone",
                  ]),
                  { numericColumns: new Set(["id"]) },
                ) +
                (preview.to_delete.some((r) => !r.file_exists)
                  ? `<p class="hint-block">Some rows point at files that are already gone, so
                     enforcement will free fewer bytes than the estimate above. A rescan would
                     have removed those rows too.</p>`
                  : "")
              : `<p class="hint">Nothing would be deleted under this policy.</p>`),
        )
      : "");

  const sizeOn = root.querySelector<HTMLInputElement>("#size-on")!;
  const ageOn = root.querySelector<HTMLInputElement>("#age-on")!;
  sizeOn.addEventListener("change", () => {
    policy.max_total_bytes = sizeOn.checked ? 50 * GIB : null;
    draw();
  });
  ageOn.addEventListener("change", () => {
    policy.max_age_days = ageOn.checked ? 30 : null;
    draw();
  });
}

function collect() {
  const gb = root!.querySelector<HTMLInputElement>("#max-gb")!;
  const days = root!.querySelector<HTMLInputElement>("#max-days")!;
  const override = root!.querySelector<HTMLInputElement>("#age-override")!;
  policy = {
    max_total_bytes: gb.disabled || gb.value === "" ? null : Math.round(Number(gb.value) * GIB),
    max_age_days: days.disabled || days.value === "" ? null : Number(days.value),
  };
  ageOverrideDays = Number(override.value) || 0;
}

async function runPreview() {
  collect();
  const result = await tryCall<RetentionPreview>("dev_retention_preview", {
    policy,
    nowMillis: ageOverrideDays ? Date.now() + ageOverrideDays * DAY : undefined,
  });
  if (result.ok) {
    preview = result.value;
    previewError = null;
  } else {
    preview = null;
    previewError = result.error;
  }
  draw();
}

export const retentionPanel: Panel = {
  id: "retention",
  title: "Retention",
  icon: "⚖",
  group: "Data",

  async mount(el, context) {
    root = el;
    ctx = context;
    preview = null;
    previewError = null;

    const health = await tryCall<DevHealth>("dev_health");
    if (health.ok) policy = { ...health.value.policy };
    draw();
    await runPreview();

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;

      if (target.closest("[data-preview]")) {
        await runPreview();
        return;
      }

      if (target.closest("[data-apply]")) {
        collect();
        await runPreview();
        const count = preview?.to_delete.length ?? 0;
        const ok = await confirmDialog({
          title: count ? `Delete ${count} recording(s)?` : "Save this policy?",
          body: count
            ? `Enforcement will remove ${count} recording(s) and free about ${bytes(
                preview?.would_free_bytes ?? 0,
              )}. Files are deleted from disk. This cannot be undone.`
            : "Nothing currently matches the policy, so nothing will be deleted.",
          confirmLabel: count ? "Delete them" : "Save",
        });
        if (!ok) return;

        try {
          const report = await call<{ deleted: number[]; freed_bytes: number }>(
            "set_retention_policy",
            { policy },
          );
          toast(
            `Removed ${report.deleted.length} recording(s), freed ${bytes(report.freed_bytes)}`,
            "ok",
          );
          await runPreview();
          ctx?.refresh();
        } catch (err) {
          toast(String(err), "err");
        }
      }
    });
  },

  unmount() {
    root = null;
    ctx = null;
  },
};

<!--
  The League client pill, and the card it opens on hover.

  One pill for what used to be two ("NinjaGoldfinch#OCE" and "Waiting for a
  game"): the client's phase in words, with the recorder taking over while it
  is recording or saving. The card has the rest: who is signed in, where the
  client is on the way to a game, whether the recorder is ready, and the last
  game it saved.

  Opens on hover and on keyboard focus, and a click pins it open until the
  next click, Escape, or a click elsewhere. **Every string here is worded by
  `lib/shell/client.ts`**; the raw phase never reaches the markup.

  The summoner name comes from the League client and is interpolated as text,
  never `{@html}`.
-->

<script lang="ts">
import { profileIcon } from "../../../icons";
import { showView } from "../../../router";
import { refusalNote, softwareNote } from "../../settings/capture";
import {
  clientPill,
  lastSavedLine,
  PHASE_STEPS,
  phaseLabel,
  phaseStep,
  recorderLine,
  splitRiotId,
} from "../../shell/client";
import { client } from "../../stores/client.svelte";
import { daemon, whenDaemonReachable } from "../../stores/daemon.svelte";
import { fillInProfileIcon, iconVersion } from "../../stores/icons.svelte";
import { library } from "../../stores/library.svelte";
import { loadCaptureBackend, settings } from "../../stores/settings.svelte";

const CARD_ID = "client-card";

let hovering = $state(false);
let focused = $state(false);
let pinned = $state(false);
let closeTimer: number | undefined;
let root: HTMLElement;

const open = $derived(hovering || focused || pinned);

const pill = $derived(clientPill(client.lcu, client.game, client.elapsed, client.failed));
const riotId = $derived(client.lcu?.connected ? splitRiotId(client.lcu.summoner) : null);
const step = $derived(phaseStep(client.lcu));
const capture = $derived.by(() => {
  const status = settings.captureBackend;
  return status ? { problem: refusalNote(status), software: softwareNote(status) } : null;
});
const recorder = $derived(recorderLine(client.game, client.elapsed, daemon.health?.state, capture));
const lastSaved = $derived(lastSavedLine(library.rows));
const iconId = $derived(client.lcu?.connected ? client.lcu.profile_icon_id : null);
const avatar = $derived.by(() => {
  iconVersion();
  return profileIcon(iconId);
});

// The card says whether the recorder can record, which the capture status
// answers. Settings only reads it when that view opens, so ask here too, once
// the daemon can answer; `refreshCaptureBackend` keeps it current after that.
whenDaemonReachable(() => void loadCaptureBackend());

$effect(() => {
  if (iconId !== null) void fillInProfileIcon(iconId);
});

// A pinned card closes on a click anywhere else, as a menu would.
$effect(() => {
  if (!pinned) return;
  const away = (e: PointerEvent) => {
    if (!root.contains(e.target as Node)) pinned = false;
  };
  document.addEventListener("pointerdown", away);
  return () => document.removeEventListener("pointerdown", away);
});

function enter() {
  window.clearTimeout(closeTimer);
  hovering = true;
}

// A short grace period, so the pointer can cross the gap between the pill and
// the card without the card closing under it.
function leave() {
  window.clearTimeout(closeTimer);
  closeTimer = window.setTimeout(() => (hovering = false), 160);
}

function onFocusOut(e: FocusEvent) {
  if (!root.contains(e.relatedTarget as Node | null)) focused = false;
}

function onKey(e: KeyboardEvent) {
  if (e.key === "Escape" && open) {
    pinned = false;
    hovering = false;
    focused = false;
    (root.querySelector(".client-pill") as HTMLElement | null)?.blur();
  }
}

function openSettings() {
  pinned = false;
  hovering = false;
  showView("settings");
}
</script>

<div
  class="client-status"
  bind:this={root}
  role="presentation"
  onmouseenter={enter}
  onmouseleave={leave}
  onfocusin={() => (focused = true)}
  onfocusout={onFocusOut}
  onkeydown={onKey}
>
  <button
    type="button"
    class="client-pill"
    data-tone={pill.tone}
    aria-expanded={open}
    aria-controls={CARD_ID}
    onclick={() => (pinned = !pinned)}
  >
    {#if pill.glyph === "league"}
      <svg class="client-glyph" viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
        <path d="M6 3h4v14h8l-2 4H6z" fill="currentColor" />
        <path d="M12 5.2a7 7 0 0 1 4.9 12H14.6A5.3 5.3 0 0 0 12 7z" fill="currentColor" opacity=".55" />
      </svg>
    {:else if pill.glyph === "spin"}
      <span class="client-spin" aria-hidden="true"></span>
    {:else}
      <span class="client-dot" aria-hidden="true"></span>
    {/if}
    <span class="client-label" role="status" aria-live="polite">{pill.label}</span>
    {#if pill.detail}
      <span class="client-detail">{pill.detail}</span>
    {/if}
    <svg class="client-chev" viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
      <path
        d="m7 10 5 5 5-5"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  </button>

  <div class="client-card" id={CARD_ID} data-tone={pill.tone} hidden={!open}>
    <div class="client-card-head">
      <span class="client-avatar">
        {#if avatar}
          <img src={avatar} alt="" draggable="false" />
        {:else}
          <span aria-hidden="true">{riotId?.name.charAt(0).toUpperCase() ?? "?"}</span>
        {/if}
      </span>
      <div class="client-who">
        {#if riotId}
          <span class="client-name selectable"
            >{riotId.name}{#if riotId.tag}<span class="client-tag">#{riotId.tag}</span>{/if}</span
          >
        {:else}
          <span class="client-name">League client</span>
        {/if}
        <span class="client-conn" data-tone={pill.tone}>
          {#if client.lcu === null}
            Checking…
          {:else if client.lcu.error}
            Can't reach the client
          {:else if client.lcu.connected}
            Connected
          {:else}
            Not running
          {/if}
        </span>
      </div>
    </div>

    {#if step !== null}
      <div class="client-track" aria-hidden="true">
        <div class="client-track-bar">
          {#each PHASE_STEPS as label, i (label)}
            <i data-at={i < step ? "done" : i === step ? "now" : "todo"}></i>
          {/each}
        </div>
        <div class="client-track-labels">
          {#each PHASE_STEPS as label, i (label)}
            <span class:now={i === step}>{label}</span>
          {/each}
        </div>
      </div>
    {/if}

    <dl class="client-rows">
      {#if client.lcu?.connected}
        <div class="client-row">
          <dt>Phase</dt>
          <dd>{phaseLabel(client.lcu.phase)}</dd>
        </div>
      {/if}
      <div class="client-row">
        <dt>Recorder</dt>
        <dd>
          <span class="client-chip" data-tone={recorder.tone} title={recorder.title ?? undefined}
            >{recorder.text}</span
          >
        </dd>
      </div>
      <div class="client-row">
        <dt>Last saved</dt>
        <dd class="muted">{lastSaved ?? "Nothing yet"}</dd>
      </div>
    </dl>

    <div class="client-card-foot">
      <button type="button" class="client-settings-link" onclick={openSettings}>Recording settings</button>
    </div>
  </div>
</div>

/**
 * Dev portal entry point.
 *
 * The portal (`dev.html`) and its `dev_*` commands are compiled out unless
 * the `devtools` Cargo feature is on, so availability is *detected* rather
 * than configured: ask the backend whether the dev commands are
 * registered (`hasDevCommands`, which `desktop.ts` shares), and only then
 * reveal the button. That keeps the frontend from carrying a second flag
 * that could drift from the Rust side.
 */
import { call, hasDevCommands } from "./bridge";
import { el } from "./dom";

export function initDevPortal() {
  const button = el<HTMLButtonElement>("#open-dev-portal-btn");

  button.addEventListener("click", () => {
    void call("dev_open_portal").catch((err) => {
      console.warn("dev portal unavailable:", err);
      button.hidden = true;
    });
  });

  // A rejection is the expected outcome in a shipped build — the command
  // simply isn't registered — which is why `hasDevCommands` answers `false`
  // rather than throwing.
  void hasDevCommands().then((available) => {
    button.hidden = !available;
  });
}

// `revealRowInspectors` lived here until WS4.3. It remembered the probe's
// answer and toggled `hidden` on every `[data-inspect]` button in the grid,
// because `library.ts` rebuilt the rows with `innerHTML` on every
// `library-changed` and the probe resolves only once, so the buttons would
// otherwise appear on the first paint and vanish on the next. `Library.svelte`
// asks `hasDevCommands` itself and passes the answer down as a prop, so the
// button is simply not rendered rather than rendered and hidden.

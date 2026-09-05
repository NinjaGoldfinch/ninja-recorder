/**
 * Dev portal entry point.
 *
 * The portal (`dev.html`) and its `dev_*` commands are compiled out unless
 * the `devtools` Cargo feature is on, so availability is *detected* rather
 * than configured: ask the backend whether the dev commands are
 * registered, and only then reveal the button. That keeps the frontend
 * from carrying a second flag that could drift from the Rust side.
 */
import { call } from "./bridge";
import { el } from "./dom";

export function initDevPortal() {
  const button = el<HTMLButtonElement>("#open-dev-portal-btn");

  button.addEventListener("click", () => {
    void call("dev_open_portal").catch((err) => {
      console.warn("dev portal unavailable:", err);
      button.hidden = true;
    });
  });

  // A rejection here is the expected outcome in a shipped build — the
  // command simply isn't registered — so it stays a debug note rather
  // than anything the user sees.
  void call<string[]>("dev_registered_commands")
    .then((commands) => {
      button.hidden = commands.length === 0;
    })
    .catch(() => {
      button.hidden = true;
    });
}

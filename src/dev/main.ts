/**
 * The dev portal's composition root.
 *
 * Everything this file used to be - a hash router, a panel registry, a shared
 * poll, a nav renderer and a `freshMain()` that cloned the mount point to shed
 * stacked event handlers - is in `lib/components/dev/DevApp.svelte` and the
 * modules under `lib/dev/` now (#72). What is left is the same shape as
 * `src/main.ts`: find the root, mount.
 */

import { mount } from "svelte";

import DevApp from "../lib/components/dev/DevApp.svelte";

window.addEventListener("DOMContentLoaded", () => {
  mount(DevApp, { target: document.querySelector("#dev-root") as HTMLElement });
});

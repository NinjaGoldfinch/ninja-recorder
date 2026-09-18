/// <reference types="vite/client" />
// Brings in `declare module "*.svelte"`, which is what lets `main.ts` import a
// component at all. `svelte-check` does not need it (it compiles the files
// itself), so without this reference the Svelte root would fail the `tsc` gate
// and pass the `svelte-check` one, which is the confusing half of that pair.
/// <reference types="svelte" />

// Injected by vite.config.ts's `define`.
declare const __APP_VERSION__: string;

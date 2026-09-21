/**
 * The one Node symbol the test suite needs, declared rather than installed.
 *
 * `src/style-hooks.test.ts` has to read the stylesheets as text, and Vitest
 * hands back an empty string for a CSS import however it is spelled: `?raw`,
 * `?inline` and `import.meta.glob` all go through Vite's CSS plugin, which is
 * disabled under `css: false`. Reading the file is the only way to see it.
 *
 * `@types/node` is deliberately not a dependency here. Installing it would
 * make `vite.config.ts` and `vitest.config.ts`'s `@ts-expect-error process is
 * a nodejs global` comments unused, which TypeScript reports as an error, so
 * the price of two type definitions is two config files breaking.
 */
declare module "node:fs" {
  /** Paths are resolved against the working directory, which Vitest sets to
   *  the project root. A wrong path throws, which is the failure mode wanted:
   *  a guard that silently read nothing would pass forever. */
  export function readFileSync(path: string, encoding: "utf8"): string;
}

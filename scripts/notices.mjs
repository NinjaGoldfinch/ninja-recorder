#!/usr/bin/env node
// Writes THIRD_PARTY_NOTICES.txt: the notices the Rust crates and the npm
// packages that ship in the installer ask a binary distribution to carry.
//
//   node scripts/notices.mjs           regenerate the file
//   node scripts/notices.mjs --check   fail if the committed file is stale
//
// CI runs `--check`, so adding, removing or bumping a dependency without
// regenerating fails the build, the same way `gen-contract --check` does for
// the contract. See docs/licensing.md, "Third-party notices".
//
// **Rust:** `cargo about` with src-tauri/about.toml and about.hbs, for the
// Windows target and default features, which is the binary the installer
// ships. `--offline` after a `cargo fetch`, so the output depends on
// Cargo.lock and nothing else.
//
// **JavaScript:** the packages whose code is in the production frontend
// bundle, read from the bundle itself. Vite builds it in memory (nothing is
// written) and every module that rendered any code is mapped back to the
// package it came from. That is deliberately not "the `dependencies` in
// package.json": Svelte's runtime is compiled into the bundle and Svelte is a
// devDependency, while half of a production dependency can be tree-shaken
// away. The bundle is what ships, so the bundle is what is listed.
//
// Both halves are checked against deny.toml's allow list. For Rust that is
// cargo-deny's job and cargo-about's `accepted`, which this script requires to
// be the same set. For npm nothing else checks licences at all, so a package
// whose licence is not on the list fails here.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const tauri = join(root, "src-tauri");
const output = join(root, "THIRD_PARTY_NOTICES.txt");
const rule = "-".repeat(78);

/** The quoted strings in the first `key = [ ... ]` array of a TOML file. */
function tomlStringArray(file, key) {
  const text = readFileSync(file, "utf8");
  const m = text.match(new RegExp(`^${key}\\s*=\\s*\\[([\\s\\S]*?)\\]`, "m"));
  if (!m) throw new Error(`${file} has no top-level \`${key} = [...]\``);
  // Comments may contain quotes, so drop them before collecting strings.
  const body = m[1].replace(/#.*$/gm, "");
  return [...body.matchAll(/"([^"]+)"/g)].map((s) => s[1]);
}

function allowedLicences() {
  const deny = tomlStringArray(join(tauri, "deny.toml"), "allow");
  const about = tomlStringArray(join(tauri, "about.toml"), "accepted");
  const a = [...deny].sort().join(", ");
  const b = [...about].sort().join(", ");
  if (a !== b) {
    throw new Error(
      `about.toml's \`accepted\` must be the same set as deny.toml's \`allow\`.\n` +
        `  deny.toml:  ${a}\n  about.toml: ${b}`,
    );
  }
  return deny;
}

function rustNotices() {
  const run = (args) =>
    execFileSync("cargo", args, {
      cwd: tauri,
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
      stdio: ["ignore", "pipe", "inherit"],
    });
  run(["fetch", "--locked"]);
  return run(["about", "generate", "--locked", "--offline", "--fail", "about.hbs"]);
}

/** The package directory a module id resolves into, or null for our own code. */
function packageDir(id) {
  const path = id.replace(/^\0/, "").split("?")[0].replaceAll("\\", "/");
  const at = path.lastIndexOf("/node_modules/");
  if (at < 0) return null;
  const rest = path.slice(at + "/node_modules/".length).split("/");
  const name = rest[0].startsWith("@") ? `${rest[0]}/${rest[1]}` : rest[0];
  return `${path.slice(0, at)}/node_modules/${name}`;
}

async function bundledPackages() {
  // The shipped bundle, never the devtools one.
  delete process.env.NINJA_DEVTOOLS;
  const { build } = await import("vite");
  const dirs = new Set();
  await build({
    root,
    configFile: join(root, "vite.config.ts"),
    logLevel: "warn",
    build: { write: false },
    plugins: [
      {
        name: "notices:collect-packages",
        generateBundle(_options, bundle) {
          for (const chunk of Object.values(bundle)) {
            if (chunk.type !== "chunk") continue;
            for (const [id, module] of Object.entries(chunk.modules)) {
              if (module.renderedLength === 0) continue;
              const dir = packageDir(id);
              if (dir) dirs.add(dir);
            }
          }
        },
      },
    ],
  });
  return [...dirs];
}

/**
 * Whether an SPDX expression is satisfied by the allow list: every term of
 * an AND, and at least one side of an OR. Anything more elaborate (`WITH`,
 * nested mixes) is refused rather than guessed at.
 */
function licenceAllowed(expression, allowed) {
  const flat = expression.replace(/[()]/g, "").trim();
  if (/\bWITH\b/.test(flat) || (/\bAND\b/.test(flat) && /\bOR\b/.test(flat))) {
    throw new Error(`cannot evaluate the licence expression "${expression}"; decide it by hand`);
  }
  if (/\bOR\b/.test(flat)) return flat.split(/\s+OR\s+/).some((l) => allowed.includes(l));
  return flat.split(/\s+AND\s+/).every((l) => allowed.includes(l));
}

const LICENCE_FILE = /^(licen[cs]e|copying|notice)([-._].*)?$/i;

function jsNotices(dirs, allowed) {
  const packages = dirs
    .map((dir) => {
      const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
      const licence = typeof pkg.license === "string" ? pkg.license : pkg.license?.type;
      if (!licence) throw new Error(`${pkg.name} declares no licence`);
      if (!licenceAllowed(licence, allowed)) {
        throw new Error(
          `${pkg.name}@${pkg.version} is ${licence}, which deny.toml's allow list does not cover`,
        );
      }
      const files = readdirSync(dir)
        .filter((f) => LICENCE_FILE.test(f))
        .sort();
      if (files.length === 0) throw new Error(`${pkg.name}@${pkg.version} ships no licence file`);
      const repo = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
      return { name: pkg.name, version: pkg.version, licence, repo, dir, files };
    })
    .sort((a, b) => a.name.localeCompare(b.name, "en"));

  const out = [
    "JAVASCRIPT PACKAGES",
    "",
    "The frontend is a single bundle containing code from the npm packages",
    "below, read from the production build itself. Each is followed by the",
    "licence files it publishes.",
    "",
    "Packages:",
    ...packages.map((p) => `  ${p.name} ${p.version} (${p.licence})`),
  ];
  for (const p of packages) {
    out.push("", rule, `${p.name} ${p.version}`, "", `Licence: ${p.licence}`);
    if (p.repo) out.push(`Repository: ${p.repo.replace(/^git\+/, "")}`);
    for (const f of p.files) {
      const text = readFileSync(join(p.dir, f), "utf8").replace(/\r\n/g, "\n").trimEnd();
      out.push("", `[${f}]`, "", text);
    }
  }
  return `${out.join("\n")}\n`;
}

const header = `THIRD-PARTY NOTICES

ninja-recorder is built from its own code and from the third-party software
listed below. Each entry names its licence and reproduces the licence text and
notices that licence asks to be kept with a copy.

The libobs folder installed beside ninja-recorder holds separate programs
under their own licences, which are not listed here: ffmpeg.exe
(LGPL-3.0-or-later), and the libobs runtime with extprocess_recorder.exe
(GPL-2.0).

Generated by scripts/notices.mjs from Cargo.lock and the frontend bundle.
Do not edit it by hand: run \`node scripts/notices.mjs\` instead.

`;

async function generate() {
  const allowed = allowedLicences();
  const rust = rustNotices().replace(/\r\n/g, "\n").trimEnd();
  const js = jsNotices(await bundledPackages(), allowed).trimEnd();
  return `${header}${"=".repeat(78)}\n${rust}\n\n${"=".repeat(78)}\n${js}\n`;
}

const text = await generate();
if (process.argv.includes("--check")) {
  const committed = existsSync(output) ? readFileSync(output, "utf8") : "";
  if (committed !== text) {
    console.error(
      "THIRD_PARTY_NOTICES.txt is stale: the dependencies changed and the notices did not.\n" +
        "Run `node scripts/notices.mjs` and commit the result.",
    );
    process.exit(1);
  }
  console.log("THIRD_PARTY_NOTICES.txt matches the dependency graph and the bundle.");
} else {
  writeFileSync(output, text);
  console.log(`wrote ${output}`);
}

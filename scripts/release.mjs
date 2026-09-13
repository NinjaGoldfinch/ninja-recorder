#!/usr/bin/env node
// Owns the *version decision*. CI owns the build.
//
// That split is the point. A script that built releases locally would be
// back to cross-compiling libobs (DEVELOPMENT.md §9), and CI alone cannot
// decide when something is worth calling 1.0. So this does exactly two
// things — move a number, and push a tag — and never invokes a compiler.
//
// The model, in one line: **package.json's version is what we are building
// toward**, and every commit on main publishes an alpha prerelease of it.
//
//   npm run release -- next 1.0.0   declare it; alphas become 1.0.0-alpha.N
//   npm run release -- cut          it is ready; tag v<declared>, CI ships it
//
// See DEVELOPMENT.md §15.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const PKG = "package.json";

function git(...args) {
  return execFileSync("git", args, { encoding: "utf8" }).trim();
}

function fail(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

function declared() {
  return JSON.parse(readFileSync(PKG, "utf8")).version;
}

/** Stable tags only — anything with a `-` is a prerelease. */
function stableTags() {
  return git("tag", "-l", "v[0-9]*.[0-9]*.[0-9]*")
    .split("\n")
    .filter((t) => t && !t.includes("-"))
    .map((t) => t.slice(1));
}

function compare(a, b) {
  const [x, y] = [a, b].map((v) => v.split(".").map(Number));
  for (let i = 0; i < 3; i += 1) {
    if (x[i] !== y[i]) return x[i] - y[i];
  }
  return 0;
}

/**
 * Refuses to act on a tree that is not exactly what CI will see.
 *
 * Both subcommands push, and both are read back by a workflow that builds
 * from the pushed ref. Acting on a dirty or stale tree means tagging a
 * commit that does not contain what was just looked at.
 */
function requireCleanMain() {
  if (git("status", "--porcelain")) {
    fail("working tree is not clean; commit or stash first");
  }
  const branch = git("rev-parse", "--abbrev-ref", "HEAD");
  if (branch !== "main") fail(`on ${branch}, not main`);
  git("fetch", "origin", "--quiet", "--tags");
  if (git("rev-parse", "HEAD") !== git("rev-parse", "origin/main")) {
    fail("main is not in sync with origin/main; pull or push first");
  }
}

function next(version) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    fail(`"${version}" is not a plain x.y.z version`);
  }
  requireCleanMain();

  const newest = stableTags().sort(compare).pop();
  if (newest && compare(version, newest) <= 0) {
    // Not pedantry: alphas of a version at or below the newest stable tag
    // sort *below* it, so every alpha would be invisible to the updater —
    // 0.9.0-alpha.1 < 0.9.0, and semver is what the updater compares on.
    fail(`${version} is not above the newest stable release ${newest}`);
  }

  const pkg = JSON.parse(readFileSync(PKG, "utf8"));
  if (pkg.version === version) fail(`already declared ${version}`);
  pkg.version = version;
  writeFileSync(PKG, `${JSON.stringify(pkg, null, 2)}\n`);

  git("add", PKG);
  git("commit", "-m", `chore(release): work toward ${version}`);
  git("push", "origin", "main");
  console.log(`declared ${version}; the next push to main publishes ${version}-alpha.N`);
}

function cut() {
  requireCleanMain();
  const version = declared();
  const tag = `v${version}`;

  if (stableTags().includes(version)) fail(`${tag} already exists`);
  // The tag is what CI builds from, and it builds the commit the tag points
  // at — so tagging a commit whose package.json says something else would
  // ship a release named after a version it does not contain.
  git("tag", "-a", tag, "-m", `ninja-recorder ${version}`);
  git("push", "origin", tag);
  console.log(`pushed ${tag}; CI will publish the stable release`);
  console.log(`now run: npm run release -- next <the version after ${version}>`);
}

const [command, argument] = process.argv.slice(2);
switch (command) {
  case "next":
    if (!argument) fail("usage: npm run release -- next <x.y.z>");
    next(argument);
    break;
  case "cut":
    cut();
    break;
  default:
    console.error("usage: npm run release -- next <x.y.z> | cut");
    process.exit(1);
}

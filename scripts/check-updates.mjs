#!/usr/bin/env node
// Report newer upstream releases for the pins that Dependabot cannot see.
// Read-only by default; --json is for the scheduled workflow.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const versions = JSON.parse(readFileSync(resolve(root, "versions.json"), "utf8"));
const json = process.argv.includes("--json");

const semver = (value) => value.replace(/^v/, "").split(/[.+-]/).map((part) => Number.parseInt(part, 10) || 0);
const newer = (candidate, current) => {
  const a = semver(candidate), b = semver(current);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    if ((a[index] ?? 0) > (b[index] ?? 0)) return true;
    if ((a[index] ?? 0) < (b[index] ?? 0)) return false;
  }
  return false;
};
const sameMajor = (candidate, current) => semver(candidate)[0] === semver(current)[0];
const get = async (url, headers = {}) => {
  const response = await fetch(url, { headers: { "user-agent": "orbit-dependency-watch", ...headers } });
  if (!response.ok) throw new Error(`${url} returned ${response.status}`);
  return response;
};
const github = async (repo) => (await (await get(`https://api.github.com/repos/${repo}/releases/latest`)).json()).tag_name;
const gitlab = async (project) => (await (await get(`https://gitlab.com/api/v4/projects/${project}/releases?per_page=1`)).json())[0]?.tag_name;
const npm = async (name) => (await (await get(`https://registry.npmjs.org/${encodeURIComponent(name)}/latest`)).json()).version;

const proposals = [];
const consider = (name, current, candidate, note = "") => {
  if (candidate && newer(candidate, current) && sameMajor(candidate, current)) {
    proposals.push({ name, current, candidate: candidate.replace(/^v/, ""), note });
  }
};
const safely = async (name, fn) => {
  try {
    await fn();
  } catch (error) {
    proposals.push({ name: `watcher:${name}`, current: "", candidate: "", note: error.message });
  }
};
await safely("opencode", async () => consider("OpenCode", versions.opencode.version, await github("anomalyco/opencode"), "run OpenCode E2E before merging"));
await safely("pi", async () => consider("Pi", versions.pi, await npm("@earendil-works/pi-coding-agent"), "run Pi E2E before merging"));
await safely("gh", async () => consider("gh", versions.gh.version, await github("cli/cli"), "update the release SHA256 from checksums.txt"));
await safely("glab", async () => consider("glab", versions.glab.version, await gitlab("34675721"), "update the release SHA256 from checksums.txt"));
await safely("helm", async () => consider("Helm", versions.helm, await github("helm/helm"), "setup-helm action version"));
const rust = await (await get("https://static.rust-lang.org/dist/channel-rust-stable.toml")).text();
const stable = /\[pkg\.rust\]\nversion = "([0-9.]+)/.exec(rust)?.[1];
consider("Rust", versions.rust, stable, "bump rust-toolchain.toml and Docker builder together");

if (json) {
  process.stdout.write(`${JSON.stringify(proposals, null, 2)}\n`);
} else if (proposals.length) {
  console.log("Upstream updates available:");
  for (const item of proposals) console.log(`- ${item.name}: ${item.current || "?"} -> ${item.candidate || "?"}${item.note ? ` (${item.note})` : ""}`);
} else {
  console.log("All watched versions are current.");
}

#!/usr/bin/env node
// Apply versions.json to the deterministic pins. Intentionally does not rewrite Node or
// Postgres image digests/tags: a digest must come from the registry, so those remain
// checked (not generated) and are reviewed with the update.
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const versions = JSON.parse(readFileSync(resolve(root, "versions.json"), "utf8"));
const path = (name) => resolve(root, name);
const update = (name, replacer) => {
  const before = readFileSync(path(name), "utf8");
  const after = replacer(before);
  if (after !== before) {
    writeFileSync(path(name), after);
    console.log(`updated ${name}`);
  }
};
const replaceArg = (text, name, value) =>
  text.replace(new RegExp(`ARG ${name}=[^\\n]+`), `ARG ${name}=${value}`);

for (const file of ["deploy/docker/worker-opencode.Dockerfile", "deploy/docker/worker-pi.Dockerfile"]) {
  update(file, (text) => {
    let next = replaceArg(text, "GH_VERSION", versions.gh.version);
    next = replaceArg(next, "GH_SHA256", versions.gh.sha256);
    next = replaceArg(next, "GLAB_VERSION", versions.glab.version);
    next = replaceArg(next, "GLAB_SHA256", versions.glab.sha256);
    return next;
  });
}
update("deploy/docker/worker-opencode.Dockerfile", (text) => {
  let next = replaceArg(text, "OPENCODE_VERSION", versions.opencode.version);
  return replaceArg(next, "OPENCODE_SHA256", versions.opencode.sha256);
});
update("deploy/docker/worker-pi.Dockerfile", (text) =>
  text.replace(/@earendil-works\/pi-coding-agent@[0-9.]+/, `@earendil-works/pi-coding-agent@${versions.pi}`),
);
update("rust-toolchain.toml", (text) =>
  text.replace(/channel = "[^"]+"/, `channel = "${versions.rust}"`),
);

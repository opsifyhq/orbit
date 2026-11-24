// Keep manual pins aligned with versions.json. Dependabot handles crates/npm/actions;
// this covers the Rust toolchain, Node/Postgres images and the harness/release binaries.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(new URL("..", import.meta.url).pathname);
const read = (path) => readFileSync(resolve(root, path), "utf8");
const versions = JSON.parse(read("versions.json"));
const failures = [];
const expect = (name, actual, wanted) => {
  if (actual !== wanted) failures.push(`${name}: found ${actual ?? "missing"}, expected ${wanted}`);
};
const match = (text, re) => re.exec(text)?.[1];
const arg = (text, name) => match(text, new RegExp(`ARG ${name}=([^\\s]+)`));

const toolchain = match(read("rust-toolchain.toml"), /channel = "([^"]+)"/);
expect("rust-toolchain.toml", toolchain, versions.rust);

for (const file of ["deploy/docker/worker-opencode.Dockerfile", "deploy/docker/worker-pi.Dockerfile"]) {
  const text = read(file);
  expect(`${file} GH_VERSION`, arg(text, "GH_VERSION"), versions.gh.version);
  expect(`${file} GH_SHA256`, arg(text, "GH_SHA256"), versions.gh.sha256);
  expect(`${file} GLAB_VERSION`, arg(text, "GLAB_VERSION"), versions.glab.version);
  expect(`${file} GLAB_SHA256`, arg(text, "GLAB_SHA256"), versions.glab.sha256);
}
const opencode = read("deploy/docker/worker-opencode.Dockerfile");
expect("OpenCode version", arg(opencode, "OPENCODE_VERSION"), versions.opencode.version);
expect("OpenCode SHA256", arg(opencode, "OPENCODE_SHA256"), versions.opencode.sha256);
const pi = read("deploy/docker/worker-pi.Dockerfile");
expect(
  "Pi npm package",
  match(pi, /@earendil-works\/pi-coding-agent@([0-9.]+)/),
  versions.pi,
);

for (const file of ["deploy/docker/web.Dockerfile", "deploy/docker/worker-pi.Dockerfile"]) {
  const node = match(read(file), /FROM node:([^@\s]+)/);
  if (!node?.startsWith(`${versions.node}-`)) {
    failures.push(`${file}: Node ${node ?? "missing"} does not start with ${versions.node}-`);
  }
}
for (const file of ["deploy/compose/compose.yaml", "deploy/compose/compose.dev.yaml", "deploy/helm/orbit/values.yaml"]) {
  const postgres = match(read(file), /image:\s*postgres:([^@\s]+)/);
  if (postgres !== `${versions.postgres}-alpine`) {
    failures.push(`${file}: Postgres ${postgres ?? "missing"} is not ${versions.postgres}-alpine`);
  }
}
const ci = read(".github/workflows/ci.yml");
const helm = match(ci, /azure\/setup-helm@[0-9a-f]+ # (v[0-9.]+)/);
if (helm !== `v${versions.helm}`) failures.push(`CI Helm action: ${helm ?? "missing"}, expected v${versions.helm}`);

if (failures.length) {
  console.error("External version drift:");
  for (const failure of failures) console.error(`  ${failure}`);
  process.exit(1);
}
console.log(`External versions match versions.json (${failures.length} problems).`);

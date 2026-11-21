// Generate the typed API client from an OpenAPI document.
// Usage: pnpm api:generate                       (fetch from ORBIT_API_URL)
//        ORBIT_OPENAPI_SPEC=./spec.json pnpm api:generate  (offline; see scripts/dump-openapi.sh)
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

const localSpec = process.env.ORBIT_OPENAPI_SPEC;
let schemaSource = localSpec ? resolve(localSpec) : null;

if (schemaSource) {
  if (!existsSync(schemaSource)) {
    console.error(`OpenAPI document not found: ${schemaSource}`);
    process.exit(1);
  }
} else {
  const apiUrl = (process.env.ORBIT_API_URL ?? "http://localhost:8080").replace(/\/$/, "");
  const schemaUrl = `${apiUrl}/api/v1/openapi.json`;
  const check = await fetch(schemaUrl).catch(() => null);
  if (!check || !check.ok) {
    console.error(`Cannot reach ${schemaUrl}. Start orbit-api first or set ORBIT_OPENAPI_SPEC.`);
    process.exit(1);
  }
  schemaSource = schemaUrl;
}

const cwd = new URL("../packages/api-client", import.meta.url).pathname;
if (!existsSync(`${cwd}/package.json`)) {
  console.error("packages/api-client not found");
  process.exit(1);
}

const result = spawnSync(
  "pnpm",
  ["exec", "openapi-typescript", schemaSource, "-o", "src/schema.d.ts"],
  { cwd, stdio: "inherit", env: process.env },
);
process.exit(result.status ?? 1);

// Generate the typed API client from the control plane's OpenAPI document.
//
// Usage: pnpm api:generate
//
// No running API or database is needed: the document is printed by a small Rust example
// (`cargo run -p orbit-api --example openapi`), saved as packages/api-client/openapi.json
// and turned into TypeScript types. Both files are committed; CI fails when either is
// stale (a Rust test compares openapi.json with the code, the web job compares
// schema.d.ts with openapi.json).
//
//   pnpm api:generate            regenerate both files
//   pnpm api:generate --check    exit 1 if schema.d.ts does not match openapi.json
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const clientDir = join(root, "packages/api-client");
const specPath = join(clientDir, "openapi.json");
const schemaPath = join(clientDir, "src/schema.d.ts");
const check = process.argv.includes("--check");

function run(command, args, options) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  if (result.status !== 0) {
    process.stderr.write(result.stderr ?? "");
    console.error(`${command} ${args.join(" ")} failed`);
    process.exit(result.status ?? 1);
  }
  return result.stdout;
}

function generateTypes(output) {
  run("pnpm", ["exec", "openapi-typescript", specPath, "-o", output], {
    cwd: clientDir,
    stdio: ["ignore", "ignore", "inherit"],
  });
}

if (check) {
  const fresh = join(mkdtempSync(join(tmpdir(), "orbit-api-client-")), "schema.d.ts");
  generateTypes(fresh);
  if (readFileSync(fresh, "utf8") !== readFileSync(schemaPath, "utf8")) {
    console.error(
      "packages/api-client/src/schema.d.ts does not match packages/api-client/openapi.json.\n" +
        "Run `pnpm api:generate` and commit the result.",
    );
    process.exit(1);
  }
  console.log("API client types match the OpenAPI document.");
} else {
  const spec = run("cargo", ["run", "--quiet", "-p", "orbit-api", "--example", "openapi"], {
    cwd: root,
    maxBuffer: 64 * 1024 * 1024,
  });
  writeFileSync(specPath, spec.endsWith("\n") ? spec : `${spec}\n`);
  generateTypes(schemaPath);
  console.log("Updated packages/api-client/openapi.json and src/schema.d.ts.");
}

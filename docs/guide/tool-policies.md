# Tool policies

Tool policies add per-call `allow | require_approval | deny` decisions on top of autonomy
and capability grants. They can only make a decision stricter; a license or policy can
never grant a capability.

## Rules

`GET/POST /api/v1/tool-policies`, `PATCH/DELETE /api/v1/tool-policies/{id}`. Listing needs
`settings:read`; creating, changing and deleting need `settings:write`. There is no screen
for tool policies in the web app: manage them through the API or an
[import](portable-export-import.md). Rules are scoped to the org and optionally an agent
(`agentId`); agent rules extend org rules. A rule has a `name`, `priority`, `enabled`,
`action`, `matcher`, and an optional `reason` and `impact` (shown in the approval).

| Matcher | Example |
|---|---|
| `tool` glob | `Bash`, `Read`, `github_*` |
| `capability` glob | `issues.*` |
| `effect` | `read`, `write`, `external_side_effect` |
| `connection` glob | `github-prod` |
| `pathGlob` | `secrets/**`, `src/**` |
| `commandRegex` | `^rm -rf`, `^git push` |

All configured fields must match. Empty matchers match everything (use for a default
`require_approval`).

The `capability` and `connection` matchers only apply to connector tools. Native harness
tools (`bash`, `edit` and so on) carry no capability or connection, so a rule that sets
either never matches them, even a `deny` rule. Use `tool`, `effect`, `pathGlob` or
`commandRegex` for native tools.

### Matching rules and fail-closed behaviour

| Case | `deny` / `require_approval` rule | `allow` rule |
|---|---|---|
| `pathGlob` set, no path can be extracted from the arguments | **matches** | does not match |
| `commandRegex` set, no command can be extracted | **matches** | does not match |
| `commandRegex` set but not a valid regex | **matches** | does not match |
| Relative path escaping upwards (`../../etc/x`) with a known cwd | resolved against the cwd, then globbed | same |
| Same, but no cwd is known, against an absolute glob (`/etc/**`) | **matches** (location unknown) | does not match |
| `~`-rooted path against an absolute glob | **matches** | does not match |
| Tool name | case-insensitive (`bash` = `Bash`) | case-insensitive |

Consequences: a `deny` rule that sets `pathGlob` or `commandRegex` also denies calls that
carry no inspectable path/command, so scope such rules with `tool` (for example
`tool: "Bash"`) to avoid blocking unrelated tools.

Extracted path keys: `path`, `file_path`, `filePath`, `paths`, `files`, `file`,
`filename`, `fileName`, `target`, `destination`, `directory`, `dir`, `cwd`, `workdir`,
`workingDirectory`, `old_path`, `oldPath`, `new_path`, `newPath`, `src`, `dst`, `source`,
`dest`. `pattern` is a path only for `glob`/`find`/`ls`/`list`; for `grep` it is a content
regex and is ignored. For `deny`/`require_approval` rules, path-like strings (containing
`/`, or starting with `~` or `..`) under any other key are inspected as well. Command keys:
`command`, `cmd`, `script`, `commandLine`, `command_line`, `cmdline`, `argv`.

**Limits.** `commandRegex` is a best-effort filter over the command text, not a sandbox:
shell quoting, variables, `eval`, scripts and command substitution can express the same
operation in ways a regex does not match. Paths are normalized lexically (symlinks are not
resolved). Use harness sandboxing and the autonomy level for hard guarantees.

## Precedence

Platform deny → rule deny → require_approval → allow. An `allow` rule cannot bypass a
platform approval or denial. With no matching rule, the platform autonomy/approval
decision is the default. Native harness tools therefore run unattended at L4/Auto,
require approval at L3/Auto and are denied for writes at L1/L2. Add an explicit
`require_approval` rule to make a native tool ask again at L4.

## Enforcement

The worker broker evaluates rules for connector tools and exposes `POST /policy` for
native hooks. Approvals (see [Approvals](approvals.md)) are durable, scoped to the
execution, expire with it, and record the rule, reason, impact and arguments. Every deny/approval is audited and appears as a
`tool_call` execution event. See `docs/architecture/runtime-contract.md` and the launcher docs.

### Native tools (`POST /policy`)

The broker does not trust the caller's metadata. The effect is derived from the tool name
(case-insensitive); `effect`, `capability` and `connection` in the request are ignored
(a disagreement is logged as a warning).

| Effect | Tools |
|---|---|
| `read` | `read`, `grep`, `glob`, `list`, `ls`, `find`, `task`, `subagent` |
| `external_side_effect` | `bash`, `shell`, `webfetch`, `websearch` |
| `write` | `edit`, `write`, `patch`, `multiedit`, `apply_patch` and **any unknown tool** |

The platform decision comes from `evaluate_capability` with the agent's autonomy level and
approval mode, so `allow` rules cannot bypass approval at the lower autonomy levels or with
`approval_mode=always|writes`. Relative paths resolve against the execution workspace.

`task`/`subagent` are the harness's internal dispatch tools, not external actions. They are
classified as `read` so native Agent Teams can start subagents at Observe/Recommend. The
subagent's own `bash`/`edit`/`write` calls are evaluated separately by the same hook and
remain subject to autonomy, approval mode and tool policies.

**Approval binding.** Every `/policy` response carries `argumentsHash`
(SHA-256 of the tool name plus canonical JSON arguments, keys sorted), and the approval
records it. Hooks send the hash of the arguments they will execute; the broker rejects a
mismatch, and the OpenCode plugin re-checks the hash on the input at execution time and
refuses to run if it changed. The Pi extension API cannot re-check after the hook returns,
so Pi relies on the broker-side check.

---
name: GitHub issue loop
slug: github-issue-loop
description: Resolves one agent-ready GitHub issue per team run, from branch to pull request with green checks
---
# GitHub issue loop

Use this skill when the team goal is to resolve one GitHub issue in a repository.

This skill needs a native-mode agent team whose primary agent uses the OpenCode harness,
with builder, tester and reviewer agents as the subagents. It also needs a GitHub
connection that allows the capabilities behind the tools named below (`issues.list`,
`issues.get`, `issues.update`, `pulls.create`, `checks.list`, `repo.commit` and
`repo.push`). To use it, import this file as a skill (`POST /api/v1/import`, or the
**Import** button on Skills) and attach it to the team.

## Rules

1. Work on exactly one issue per team run.
2. List open issues with label `agent-ready` using `github_issues_list`.
3. If there are none, output `TEAM_DONE: no agent-ready issues` and stop.
4. Pick the oldest issue, read it with `github_issues_get`, and state the issue number and plan.
5. Delegate implementation to the builder subagent. The primary agent remains the coordinator.
6. The builder must:
   - inspect the checked-out workspace,
   - create a branch `orbit/issue-<number>`,
   - implement the change,
   - add/update tests,
   - run the relevant test/lint commands.
7. The tester subagent runs the tests and reports failures.
8. If tests or checks fail, route the failure back to the builder with the exact error. Repeat until green.
9. Commit with `github_repo_commit` and push with `github_repo_push`.
10. Create the PR with `github_pulls_create` using:
    - head: `orbit/issue-<number>`
    - base: the repository default branch
11. Poll `github_checks_list` for the branch/commit. If checks fail, delegate the failure to the builder and repeat.
12. Only finish when:
    - the PR exists,
    - checks are green,
    - the reviewer subagent has reviewed the diff.
13. Update the issue with `github_issues_update`: remove `agent-ready`, add `agent-done`, and comment with the PR URL.

## Output contract

The primary agent must end with a short result:

```json
{
  "issue": 42,
  "branch": "orbit/issue-42",
  "pullRequest": "https://github.com/owner/repo/pull/123",
  "checks": "green",
  "status": "done"
}
```

Do not declare success without a PR URL and green checks.

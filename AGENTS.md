# RustFS Operator Agent Instructions (Global)

This root file keeps repository-wide rules only.
Use the nearest subdirectory `AGENTS.md` for path-specific guidance.

## Rule Precedence

1. System/developer instructions.
2. This file (global defaults).
3. The nearest `AGENTS.md` in the current path (more specific scope wins).

If repo-level instructions conflict, follow the nearest file and keep behavior aligned with CI.

## Communication and Language

- Respond in the same language used by the requester.
- Keep source code, comments, commit messages, and PR title/body in English.

## Sources of Truth

- Workspace layout: `Cargo.toml`
- Project overview and architecture: `CLAUDE.md`
- Local quality commands: `Makefile`
- CI quality gates: `.github/workflows/ci.yml`
- PR template: `.github/pull_request_template.md`

Avoid duplicating long command matrices in instruction files.
Reference the source files above instead.

## Mandatory Before Commit

Run and pass:

```bash
make pre-commit
```

This runs: `fmt-check` → `clippy` → `test` → `console-lint` → `console-fmt-check`.

Do not commit when required checks fail.

## Git and PR Baseline

- Use feature branches based on the latest `main`.
- Follow Conventional Commits, with subject length <= 72 characters.
- Keep PR title and description in English.
- Use `.github/pull_request_template.md` and keep all section headings.
- Use `N/A` for non-applicable template sections.
- Include verification commands in the PR description.
- When using `gh pr create`/`gh pr edit`, use `--body-file` instead of inline `--body` for multiline markdown:
  ```bash
  cat > /tmp/pr_body.md <<'EOF'
  ...markdown...
  EOF
  gh pr create ... --body-file /tmp/pr_body.md
  ```

## Security Baseline

- Never commit secrets, credentials, or key material.
- Use environment variables or vault tooling for sensitive configuration.
- Credential Secrets must contain `accesskey` and `secretkey` keys (both valid UTF-8, minimum 8 characters).

## Architecture Constraints

- All pools within a single Tenant form ONE unified RustFS cluster.
- Do not assume pool-based storage tiering. For separate clusters, use separate Tenants.
- Error handling uses `snafu`. New files must include Apache 2.0 license headers.
- Do not invent RustFS ports/constants; verify against `CLAUDE.md` and official sources.

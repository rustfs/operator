---
name: rustfs-operator-contribute
description: Commits, pushes, and opens pull requests for the RustFS Operator repo per CONTRIBUTING.md and AGENTS.md. Use when the user asks to commit, push to remote my, submit a PR upstream, or follow project contribution workflow.
---

# RustFS Operator — commit, push, PR

## Preconditions

- Run from repository root: `/home/jhw/my/operator` (or clone path).
- Source of truth: [`CONTRIBUTING.md`](../../../CONTRIBUTING.md), [`Makefile`](../../../Makefile), [`.github/pull_request_template.md`](../../../.github/pull_request_template.md).

## Before commit

1. Run **`make pre-commit`** (fmt-check → clippy → test → console-lint → console-fmt-check). Fix failures before committing.
2. User-visible changes: update **[`CHANGELOG.md`](../../../CHANGELOG.md)** under `[Unreleased]` (Keep a Changelog).
3. **Commit message**: [Conventional Commits](https://www.conventionalcommits.org/), **English**, subject **≤ 72 characters** (e.g. `fix(pool): align CEL with console validation`).

## Commit

```bash
git add -A
git status
git commit -m "type(scope): short description"
```

## Push to fork (`my`)

Remote is typically `my` → `git@github.com:GatewayJ/operator.git` (verify with `git remote -v`).

```bash
git push my main
```

If `main` is non-fast-forward on `my`, integrate or use `git push my main --force-with-lease` only when intentionally replacing fork history (dangerous).

## Open PR upstream (`rustfs/operator`)

- **Target**: `rustfs/operator` branch **`main`**.
- **Head**: fork branch (e.g. `GatewayJ:main`).
- **PR title and body**: **English**.
- **Body**: Must follow **every section** in [`.github/pull_request_template.md`](../../../.github/pull_request_template.md); use **`N/A`** where not applicable; keep all headings.

**Do not** pass multiline `--body` to `gh` inline. Write a file and use `--body-file`:

```bash
cat > /tmp/pr_body.md <<'EOF'
## Type of Change
- [x] Bug Fix
...
EOF

gh pr create --repo rustfs/operator --head GatewayJ:main --base main \
  --title "fix: concise English title" \
  --body-file /tmp/pr_body.md
```

Adjust checkboxes and sections to match the change. Include **`make pre-commit`** under Verification.

## Quick checklist

- [ ] `make pre-commit` passed
- [ ] CHANGELOG updated if user-visible
- [ ] Commit message conventional, English
- [ ] PR template complete, English, `--body-file` used

## References

- [AGENTS.md](../../../AGENTS.md) — language, security, architecture notes
- [`.cursor/rules/pr.mdc`](../../../.cursor/rules/pr.mdc) — PR / path conventions (if present)

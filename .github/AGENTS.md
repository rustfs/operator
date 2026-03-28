# GitHub Workflow Instructions

Applies to `.github/` and repository pull-request operations.

## Pull Requests

- PR titles and descriptions must be in English.
- Use `.github/pull_request_template.md` for every PR body.
- Keep all template section headings.
- Use `N/A` for non-applicable sections.
- Include verification commands in the PR details.
- For `gh pr create` and `gh pr edit`, always write markdown body to a file and pass `--body-file`.
- Do not use multiline inline `--body`; backticks and shell expansion can corrupt content or trigger unintended commands.
- Recommended pattern:
  - `cat > /tmp/pr_body.md <<'EOF'`
  - `...markdown...`
  - `EOF`
  - `gh pr create ... --body-file /tmp/pr_body.md`

## CI Alignment

When changing CI-sensitive behavior, keep local validation aligned with [`Makefile`](Makefile) at the repo root.

**Local bar before push (authoritative for contributors):** `make pre-commit` — runs Rust `fmt-check`, `clippy`, `test`, plus `console-web` lint and Prettier check (see `Makefile`).

**CI workflow** [`.github/workflows/ci.yml`](workflows/ci.yml) `test-and-lint` job currently runs:

- `cargo nextest run --all --no-tests pass` and `cargo test --all --doc`
- `cargo fmt --all --check`
- `cargo clippy --all-features -- -D warnings`

It does **not** run `console-web` checks; still run **`make pre-commit` locally** before opening a PR so frontend changes are validated.

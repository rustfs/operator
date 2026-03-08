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

When changing CI-sensitive behavior, keep local validation aligned with `.github/workflows/ci.yml`.

Current `test-and-lint` gate includes:

- `cargo fmt --all --check`
- `cargo clippy --all-features -- -D warnings`
- `cargo test --all`
- `cd console-web && npm run lint`
- `cd console-web && npx prettier --check "**/*.{ts,tsx,js,jsx,json,css,md}"`

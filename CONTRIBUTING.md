# RustFS Development Guide

## Documentation map

- **This file (`CONTRIBUTING.md`)** — Authoritative for **code quality**, commit workflow, formatting, and alignment with `make pre-commit` and CI. When instructions conflict, prefer this file plus [`Makefile`](Makefile) and [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

- **[`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md)** — Local environment setup (Kubernetes, kind, optional tools), IDE hints, and longer workflows. It **does not** redefine the quality gates above; run `make pre-commit` as the single local bar before pushing.

- **[`docs/DEVELOPMENT-NOTES.md`](docs/DEVELOPMENT-NOTES.md)** — Historical design notes and analysis sessions (not a normative spec). For current behavior, use the source tree, [`CHANGELOG.md`](CHANGELOG.md), and [`CLAUDE.md`](CLAUDE.md).

## 📋 Code Quality Requirements

### 🔧 Code Formatting Rules

**MANDATORY**: All code must be properly formatted before committing. This project enforces strict formatting standards to maintain code consistency and readability.

#### Pre-commit Requirements

Before every commit, you **MUST** pass the same checks as `make pre-commit` (see below). In practice, the steps are:

1. **Format your code** (or rely on `make fmt-check` to fail if not formatted):

   ```bash
   cargo fmt --all
   ```

2. **Verify formatting**:

   ```bash
   cargo fmt --all --check
   ```

3. **Pass clippy checks**:

   ```bash
   cargo clippy --all-features -- -D warnings
   ```

4. **Run tests**:

   ```bash
   cargo test --all
   ```

5. **Console (frontend)** — from repo root:

   ```bash
   cd console-web && npm run lint
   cd console-web && npx prettier --check "**/*.{ts,tsx,js,jsx,json,css,md}"
   ```

#### Quick Commands

Targets are defined in [`Makefile`](Makefile). Use these from the **repository root**:

```bash
# Format all Rust code
make fmt

# Check Rust formatting (no writes)
make fmt-check

# Clippy (Rust)
make clippy

# Rust tests
make test

# Frontend: ESLint + Prettier check (requires npm install in console-web/)
make console-lint
make console-fmt-check

# Full gate before push (Rust + console-web): same as project / AGENTS.md rules
make pre-commit
```

Optional quick compile (not a separate `make` target):

```bash
cargo check --all-targets
```

### 🔒 Git hooks (optional)

The repository does **not** ship a `make setup-hooks` target. To run checks automatically on `git commit`, add your own `.git/hooks/pre-commit` that invokes `make pre-commit` (or the individual commands above).

### 📝 Formatting Configuration

The project uses the following rustfmt configuration (defined in `rustfmt.toml`):

```toml
max_width = 130
fn_call_width = 90
single_line_let_else_max_width = 100
```

### 🚫 Commit Prevention

If your code doesn't meet the formatting requirements, CI or local checks will fail with clear messages.

Example output when formatting fails:

```
❌ Code formatting check failed!
💡 Please run 'cargo fmt --all' to format your code before committing.

🔧 Quick fix:
   cargo fmt --all
   git add .
   git commit
```

### 🔄 Development Workflow

1. **Make your changes**
2. **Format your code**: `make fmt` or `cargo fmt --all`
3. **Run pre-commit checks**: `make pre-commit`
4. **Commit your changes**: `git commit -m "your message"`
5. **Push to your branch**: `git push`

### 🛠️ IDE Integration

#### VS Code

Install the `rust-analyzer` extension and add to your `settings.json`:

```json
{
    "rust-analyzer.rustfmt.extraArgs": ["--config-path", "./rustfmt.toml"],
    "editor.formatOnSave": true,
    "[rust]": {
        "editor.defaultFormatter": "rust-lang.rust-analyzer"
    }
}
```

#### Other IDEs

Configure your IDE to:

- Use the project's `rustfmt.toml` configuration
- Format on save
- Run clippy checks

### ❗ Important Notes

- **Never bypass formatting checks** - they are there for a reason
- **CI and `make pre-commit`** should stay aligned; see [`Makefile`](Makefile) and [`.github/workflows/ci.yml`](.github/workflows/ci.yml)
- **Pull requests** may be rejected if checks fail
- **Consistent formatting** improves code readability and reduces merge conflicts

### 🆘 Troubleshooting

#### Formatting issues?

```bash
# Format all code
cargo fmt --all

# Check specific issues
cargo fmt --all --check --verbose
```

#### Clippy issues?

```bash
# See detailed clippy output
cargo clippy --all-targets --all-features -- -D warnings

# Fix automatically fixable issues
cargo clippy --fix --all-targets --all-features
```

---

Following these guidelines ensures high code quality and smooth collaboration across the RustFS project! 🚀

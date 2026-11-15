# 🤝 Contributing to CodeNexus

Thanks for your interest in CodeNexus. This document covers how to set up the development environment and the conventions every Pull Request must follow.

## 📋 Table of Contents

- [Development environment](#️-development-environment)
- [Commit convention — Conventional Commits](#-commit-convention--conventional-commits)
- [Pull Request process](#-pull-request-process)
- [Testing & lint requirements](#-testing--lint-requirements)
- [Code style](#-code-style)
- [Adding a new language parser](#-adding-a-new-language-parser)
- [Filing issues](#-filing-issues)
- [Code of Conduct](#-code-of-conduct)
- [Questions](#-questions)

---

## 🧰 Development environment

### Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust toolchain (stable) | 1.97.1+ | Minimum supported version, pinned in CI and `Cargo.toml` (`rust-version = "1.97.1"`). |
| Rust nightly toolchain | latest | `cargo fmt` uses the nightly-only options `imports_granularity` and `group_imports` (see `rustfmt.toml`). |
| C/C++ compiler | system default | Needed to build the `tree-sitter` grammar crates. |
| Git | 2.20+ | Hooks and `detect-changes` rely on modern `git` behavior. |

Install both Rust toolchains with [rustup](https://rustup.rs/):

```bash
rustup toolchain install 1.97.1
rustup toolchain install nightly
rustup component add clippy --toolchain 1.97.1
rustup component add rustfmt --toolchain nightly
```

### Getting the source

```bash
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo build                    # smoke build
cargo test                     # smoke test
```

### Build variants

The default feature set is `full` (all 22 languages + daemon + analysis + complexity + api-review + community + cross-service + diagram + lsp + mcp + cli + cache + embeddings + i18n + hub). To speed up development iterations you can build leaner binaries:

```bash
# Minimal: Rust only, no daemon (fastest to compile)
cargo build --no-default-features --features minimal

# Core: C + Rust + Python, no daemon
cargo build --no-default-features --features core

# Everything, including vector embeddings (slower to compile)
cargo build --features embeddings
```

## 📝 Commit convention — Conventional Commits

CodeNexus follows [Conventional Commits](https://conventionalcommits.org/). Every commit message must match:

```
<type>(<scope>): <subject>

[optional body]

[optional footer(s)]
```

### Allowed `type` values

| Type | Purpose |
|------|---------|
| `feat` | New user-facing feature |
| `fix` | Bug fix |
| `perf` | Performance improvement that does not change behavior |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `docs` | Documentation-only changes (README, CHANGELOG, design docs) |
| `test` | Adding or correcting tests |
| `chore` | Builds, CI, dependencies, tooling, repo housekeeping |
| `revert` | Reverting a previous commit |

### `scope`

Optional but recommended. Use module names (`parse`, `resolve`, `storage`, `query`, `trace`, `cli`, `daemon`, `embed`, `mcp`, `model`, `kit`). Cross-module changes may omit the scope.

### Examples

```
feat(parse): add the Go language extractor
fix(storage): prevent orphan edges when parser endpoint IDs desync
perf(parse): dedupe_qn O(N²) → O(1) HashSet
docs(readme): add Roadmap and Acknowledgments sections
chore(ci): pin Rust 1.97.1 in workflows
```

`BREAKING CHANGE:` in the footer, or `feat!:` / `fix!:` (with `!`), marks a breaking change and must include migration notes.

## 🔀 Pull Request process

1. **Fork** the repository and clone your fork.
2. **Create a branch** from `main`:
   ```bash
   git checkout -b feat/short-description
   ```
   Use a prefix matching the commit `type`: `feat/`, `fix/`, `docs/`, `refactor/`, `perf/`, `test/`, `chore/`.
3. **Commit** with Conventional Commits messages. Keep commits focused; small commits are easier to review than one large commit.
4. **Push** to your fork:
   ```bash
   git push -u origin feat/short-description
   ```
5. **Open a Pull Request** against `main`. Fill in the PR template:
   - What changed and why
   - Linked issues (e.g. `Closes #123`)
   - How it was tested
   - Breaking changes (if any) with migration notes
6. **Address review feedback** by pushing new commits (do not force-push during review unless asked).
7. **Squash-merge** by Kirky.X. The final commit message follows Conventional Commits.

## ✅ Testing & lint requirements

CI runs the following gates on every PR; they must pass locally before you push:

```bash
# 1. Format check (nightly rustfmt)
cargo +nightly fmt --all -- --check

# 2. Lint — warnings are errors
cargo clippy --all-targets -- -D warnings

# 3. Tests
cargo test --all-features
```

If the change touches feature-gated paths, also run:

```bash
cargo test --no-default-features --features minimal
cargo test --no-default-features --features core
cargo test --features embeddings
```

### Writing tests

- Unit tests live in `#[cfg(test)] mod tests` blocks inside each module.
- Integration tests live in `tests/` and exercise the CLI / index → query pipeline end to end.
- Benchmarks live in `benches/` (`criterion`). Never treat benchmark numbers as a correctness gate — `cargo test` is the only arbiter.
- Tests must assert meaningful behavior (return values, graph structure, side effects), not just "no panic".

## 🎨 Code style

- **Formatting**: `cargo +nightly fmt` is the final authority. Configuration lives in [`rustfmt.toml`](../../rustfmt.toml): 4-space indent, 100-column width, Unix line endings, `imports_granularity = Preserve`.
- **Editor config**: see [`.editorconfig`](../../.editorconfig) (Rust 4 spaces, Markdown 2 spaces, YAML 2 spaces, UTF-8, LF, trim trailing whitespace).
- **Naming**: follow standard Rust style (`snake_case` for functions/variables/modules, `PascalCase` for types/traits/enums). Stay consistent with existing codebase conventions — when in doubt, look at neighboring code.
- **Error handling**: `thiserror` for library error enums, `anyhow` at the CLI/`main` boundary. Never swallow errors silently; surface them (see the project's "Fail Loud" rule).
- **No emoji**: no emoji in source code, comments, or commit messages. Keep documentation plain text.

## 🌐 Adding a new language parser

To add a new language (e.g. Go), the high-level steps are:

1. Add a `lang-go` feature in `Cargo.toml` pulling in `tree-sitter-go`.
2. Wire it into the tiered presets (`minimal` / `core` / `full`) as appropriate.
3. Implement the extractor under `src/parse/`, following the existing extractor pattern (visit tree-sitter nodes → emit `Node`/`Edge` records).
4. Register the extractor in the unified Kit registry (`trait-kit`).
5. If the language needs new labels, add node/edge types in `src/model/`.
6. Add tests in `tests/` that index a small sample and assert the extracted graph.
7. Update the language and feature tables in the `docs/en/USER_GUIDE.md` "Build presets and feature flags" section.

Open an issue to discuss scope before starting large-scale work.

## 🐛 Filing issues

- Use the [GitHub Issue Tracker](https://github.com/Kirky-X/codenexus/issues).
- **Security vulnerabilities** must not be filed publicly — see [SECURITY.md](SECURITY.md).
- Include: CodeNexus version (`codenexus --version`), Rust version, OS, the exact failing command, the full error output, and a minimal reproduction (a small repo or file if possible).

## 📜 Code of Conduct

Participation in this project is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). By contributing you agree to abide by it.

## ❓ Questions

Open a [GitHub Discussion](https://github.com/Kirky-X/codenexus/discussions) or an issue labeled `question`. Please be patient — this project is maintained by Kirky.X.

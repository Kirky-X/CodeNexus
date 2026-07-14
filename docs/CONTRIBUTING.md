# Contributing to CodeNexus

Thanks for your interest in contributing to CodeNexus. This document describes how to set up a development environment and the conventions every pull request must follow.

## Development Environment

### Prerequisites

| Tool | Version | Why |
|------|---------|-----|
| Rust toolchain (stable) | 1.91+ | Minimum supported version, pinned in CI and `Cargo.toml` (`rust-version = "1.91"`). |
| Rust nightly toolchain | latest | `cargo fmt` uses the nightly-only options `imports_granularity` and `group_imports` (see `rustfmt.toml`). |
| C/C++ compiler | system default | Required to build the `tree-sitter` grammar crates. |
| `zstd` CLI | any recent version | Used by `export`/`import` for `.graph.zst` artifacts. |
| Git | 2.20+ | Hooks and `detect-changes` rely on modern `git` behavior. |

Install both Rust toolchains with [rustup](https://rustup.rs/):

```bash
rustup toolchain install 1.91
rustup toolchain install nightly
rustup component add clippy --toolchain 1.91
rustup component add rustfmt --toolchain nightly
```

### Get the Source

```bash
git clone https://github.com/Kirky-X/codenexus.git
cd codenexus
cargo build                    # smoke build
cargo test                     # smoke test
```

### Build Variants

The default feature set is `full` (all 21 languages + daemon + analysis + complexity + api-review + community + cross-service + lsp + mcp + cli + cache + embed). For faster iteration during development you can build a leaner binary:

```bash
# Minimal: Rust only, no daemon (fastest compile)
cargo build --no-default-features --features minimal

# Core: C + Rust + Python, no daemon
cargo build --no-default-features --features core

# Everything including vector embeddings (slower to compile)
cargo build --features embed
```

## Commit Convention — Conventional Commits

CodeNexus follows [Conventional Commits](https://conventionalcommits.org/). Every commit message must match:

```
<type>(<scope>): <subject>

[optional body]

[optional footer]
```

### Allowed `type` values

| Type | Use for |
|------|---------|
| `feat` | A new feature visible to users |
| `fix` | A bug fix |
| `perf` | A change that improves performance without altering behavior |
| `refactor` | A code change that neither fixes a bug nor adds a feature |
| `docs` | Documentation-only changes (README, CHANGELOG, design docs) |
| `test` | Adding or correcting tests |
| `chore` | Build, CI, deps, tooling, repo housekeeping |
| `revert` | Reverting a previous commit |

### `scope`

Optional but encouraged. Use the module name (`parse`, `resolve`, `storage`, `query`, `trace`, `cli`, `daemon`, `embed`, `mcp`, `model`, `kit`). For cross-cutting changes, omit the scope.

### Examples

```
feat(parse): add Go language extractor
fix(storage): prevent orphan edges when parser endpoint IDs desync
perf(parse): dedupe_qn O(N²) → O(1) HashSet
docs(readme): add Roadmap and Acknowledgments sections
chore(ci): pin Rust 1.91 in workflow
```

`BREAKING CHANGE:` in the footer or `feat!:` / `fix!:` (with `!`) flag breaking changes and must include migration notes.

## Pull Request Workflow

1. **Fork** the repository and clone your fork.
2. **Branch** from `main`:
   ```bash
   git checkout -b feat/short-description
   ```
   Use `feat/`, `fix/`, `docs/`, `refactor/`, `perf/`, `test/`, `chore/` prefixes matching the commit `type`.
3. **Commit** with Conventional Commits messages. Keep commits focused; small commits are easier to review than one large squashed commit.
4. **Push** to your fork:
   ```bash
   git push -u origin feat/short-description
   ```
5. **Open a Pull Request** against `main`. Fill in the PR template:
   - What changed and why
   - Related issue (e.g. `Closes #123`)
   - How it was tested
   - Breaking changes (if any) with migration notes
6. **Address review feedback** by pushing new commits (do not force-push during review unless asked).
7. **Maintainer squash-merges** on approval. The final commit message follows Conventional Commits.

## Test and Lint Requirements

CI runs these gates on every PR; they must pass locally before you push:

```bash
# 1. Format check (nightly rustfmt)
cargo +nightly fmt --all -- --check

# 2. Lint — warnings are errors
cargo clippy --all-targets -- -D warnings

# 3. Tests
cargo test --all-features
```

If your change touches a feature-gated path, also run:

```bash
cargo test --no-default-features --features minimal
cargo test --no-default-features --features core
cargo test --features embed
```

### Writing Tests

- Unit tests live in `#[cfg(test)] mod tests` blocks inside each module.
- Integration tests live in `tests/` and exercise the CLI / index → query pipeline end to end.
- Benchmarks live in `benches/` (`criterion` harness). Do not gate correctness on benchmark numbers — only on `cargo test`.
- Tests must assert meaningful behavior (return values, graph shape, side effects), not just "did not panic".

## Code Style

- **Formatting:** `cargo +nightly fmt` is the source of truth. Configuration lives in [`rustfmt.toml`](rustfmt.toml): 4-space indent, 100-column width, Unix newlines, `imports_granularity = Preserve`.
- **Editor config:** see [`.editorconfig`](.editorconfig) for editor-agnostic rules (Rust 4 spaces, Markdown 2 spaces, YAML 2 spaces, UTF-8, LF, trim trailing whitespace).
- **Naming:** follow standard Rust style (`snake_case` for functions/variables/modules, `PascalCase` for types/traits/variants). Match the conventions already in the codebase — when in doubt, look at neighboring code.
- **Error handling:** use `thiserror` for library error enums and `anyhow` for CLI/`main` boundaries. Do not silently swallow errors; surface them (see project rule "Fail Loud").
- **No emojis** in source, comments, or commit messages. Keep documentation plain text.

## Adding a New Language Parser

If you want to add a new language (e.g. Go), the high-level steps are:

1. Add a `lang-go` feature in `Cargo.toml` pulling in `tree-sitter-go`.
2. Wire it into the tiered presets (`minimal` / `core` / `full`) as appropriate.
3. Implement an extractor under `src/parse/` following the existing extractor pattern (visit tree-sitter nodes → emit `Node`/`Edge` records).
4. Register the extractor in the unified Kit registry (`trait-kit`).
5. Add node/edge types to `src/model/` if the language needs new labels.
6. Add tests in `tests/` indexing a small sample and asserting the extracted graph.
7. Update `README.md` "Supported Languages" and "Feature Flags" tables.

Open an issue first to discuss the scope before doing large work.

## Reporting Issues

- Use the [GitHub issue tracker](https://github.com/Kirky-X/codenexus/issues).
- For **security vulnerabilities**, do NOT open a public issue — see [SECURITY.md](SECURITY.md).
- Include: CodeNexus version (`codenexus --version`), Rust version, OS, the exact command that failed, the full error output, and a minimal reproduction (a small repo or file if possible).

## Code of Conduct

Participation in this project is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). By contributing, you agree to uphold it.

## Questions

Open a [GitHub Discussion](https://github.com/Kirky-X/codenexus/discussions) or an issue labeled `question`. Be patient — this project is maintained by a small team.

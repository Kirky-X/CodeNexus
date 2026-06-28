# Security Policy

## Supported Versions

CodeNexus is pre-1.0 software. Security fixes are applied only to the latest minor release line.

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |
| < 0.1   | No        |

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities by email to **security@kirky-x.dev**. Please include:

- A description of the vulnerability and its impact.
- Steps to reproduce, including a minimal codebase or `codenexus` command sequence if applicable.
- The CodeNexus version (`codenexus --version`), Rust toolchain version, and operating system.
- Any known mitigations you have already identified.

If you have a fix ready, mention it in the report so we can coordinate a joint disclosure. Please do not open a public pull request that exposes the vulnerability before a coordinated release.

## Response Timeline

| Step | Target |
|------|--------|
| Acknowledge receipt of the report | Within 48 hours |
| Initial assessment (valid / invalid / needs more info) | Within 5 business days |
| Fix or mitigation for accepted vulnerabilities | Best-effort within 30 days, sooner for high-severity issues |
| Coordinated public disclosure | After a fixed release is available, on an agreed date with the reporter |

These targets are commitments on a best-effort basis. CodeNexus is maintained by a small team; we will communicate proactively if a timeline slips and explain why.

## Disclosure Policy

- We follow [responsible disclosure](https://en.wikipedia.org/wiki/Coordinated_vulnerability_disclosure).
- We will credit reporters in the release notes and `CHANGELOG.md` unless they prefer to remain anonymous.
- We will not take legal action against reporters who act in good faith and follow this policy.

## Scope

In scope:

- The CodeNexus CLI and library (`src/`, `tests/`, `benches/`).
- The MCP server (`codenexus mcp`) and agent integration (`setup`, `hook`).
- Index file handling — malformed LadybugDB databases, `.graph.zst` import artifacts, and tree-sitter parse inputs that could cause panics, memory unsafety, or arbitrary code execution.
- Cypher subset query handling — injection vectors through `query` / `trace` / `impact` / `search` inputs.

Out of scope:

- Vulnerabilities in upstream dependencies (report them upstream). We will still upgrade affected dependencies promptly.
- Self-compiled binaries using non-default feature combinations not published by the project.
- Issues requiring physical access to the user's machine.

## Security Best Practices for Users

- Do not `import` `.graph.zst` artifacts from untrusted sources — a malicious artifact could trigger bugs in the import path.
- Treat `codenexus query` input as you would treat any input that reaches a database: prefer parameterized patterns over string interpolation when embedding user input in Cypher queries.
- The `embed` feature can make outbound HTTP requests to an OpenAI-compatible endpoint. Set the endpoint explicitly and do not point it at untrusted servers.

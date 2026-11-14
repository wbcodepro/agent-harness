# Changelog

All notable changes to hame are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Until 1.0, the configuration and session formats may change between versions; such changes are called out.

## Unreleased

### Added

- `hame setup`: choose a provider, save its key to `~/.hame/.env` (hidden input, owner-only file), and check that everything works.
- `hame doctor [--ping]`: check configuration, model and key, sandbox, `bash`, `git` and `python`, and optionally send one test request.
- A `bash` command still running at its time limit becomes a background job in the sandbox instead of being killed; the new `job_output` tool waits for it, reads it or stops it. A check still running is not counted as passing.
- `Retry-After` (and `retry-after-ms`) is honoured up to 60 seconds; longer waits are reported at once. Plain backoff has jitter.
- A request refused because the prompt exceeds the context window is compacted and sent again once.
- When the summary model is the main model, compaction reuses the conversation's cached prefix.
- English README, `LICENSE`, `SECURITY.md`, `CONTRIBUTING.md` and issue templates.

### Changed

- The message for a missing API key names every provider's key, where to put it, and `hame setup`.

## 0.0.1

First version, never released as a package. Highlights:

- One Rust binary with a terminal interface, `hame run`, a web interface and an Agent Client Protocol server (tested in Zed and IntelliJ IDEA).
- Providers: DeepSeek, Anthropic, OpenAI (Responses and chat completions) and compatible servers.
- Tools confined by the operating system (Seatbelt, Landlock and seccomp, Low integrity on Windows), network allowlist through a local proxy, secret substitution at the proxy.
- Completion checks: project check commands, `stop_on_red`, checklist, optional independent reviewer; model escalation when stuck.
- Append-only session log with resume, fork, rewind and workspace checkpoints.
- MCP (stdio, HTTP, stateless, OAuth), skills, Claude Code hooks and plugins, WebAssembly plugins.
- `hame eval` and `hame probe` with task suites that have hidden checks.

# Contributing to hame

Thanks for helping. Bug reports, measurements and pull requests are all welcome.

## Before you start

- For anything larger than a small fix, open an issue first so we can agree on the approach.
- Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Building and testing

```sh
cargo build -p hame-cli
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Plugin tests need `rustup target add wasm32-unknown-unknown`; some tests need `python3` and `git`. Tests never call a real model: they use the scripted model in `hame-eval` (set `HAME_SCRIPT=<file.jsonl>` to replay a recording through the real binary). A few opt-in tests call a real provider and say so at the top of the file. Linking the test binaries takes 1–2 GB of memory each; on a small machine or container pass `-j 2` or `-j 3`.

Browser and desktop tests:

- `cargo test -p hame-browser` runs the browser tests against a local fixture app when Python with Playwright and Chrome are available, and prints `SKIPPED` and passes otherwise.
- `cargo test -p hame-desktop --test gui -- --ignored --test-threads=1` drives TextEdit on macOS (Accessibility and Screen Recording must be granted to your terminal). It opens and closes its own documents, saves only into temporary directories and quits TextEdit if it was not running; do not use the Mac while it runs.
- `HAME_LIVE_ANTHROPIC=1 cargo test -p hame-llm --test live_anthropic -- --ignored` calls the Anthropic API (a fraction of a cent).

A change to the browser or desktop tools, grants or the action journal should keep the properties listed in the README ("Browser and desktop control") and come with a test of the refusal path, not only the happy path.

## Release tooling

- `scripts/docker-clean-install.sh` builds and tests a `git archive` of a commit in a fresh `rust:1-bookworm` container.
- `python3 -I scripts/license-report.py` regenerates `docs/release/THIRD-PARTY-LICENSES.md` and `docs/release/sbom.cdx.json`; run it when `Cargo.lock` changes. A new dependency with a license outside the accepted list makes `--check` fail.
- `scripts/release-check.sh` builds the source and binary archives and checks them for secrets and private files. Publishing steps are in `docs/release/RELEASE-INSTRUCTIONS.md` and need the owner's authorization.

## Changes that affect what the model does

Prompts, tool descriptions, edit formats and verification hooks change how well the agent works, and the effect is easy to misjudge. For such a change, please run the relevant eval suite before and after (`hame eval evals/harder --runs 3`, or `evals/expert` / `evals/frontier` for harder work) and put the numbers in the pull request: pass rate, tokens and time. Say which model you used.

## Style

- Code, comments and commit messages in English.
- Comments explain why, not what.
- Keep the sandbox and secret-handling guarantees: a change must not let tools write, read or connect where the README says they cannot.
- Add a line to [CHANGELOG.md](CHANGELOG.md) under "Unreleased" for user-visible changes.

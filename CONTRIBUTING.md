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

Plugin tests need `rustup target add wasm32-unknown-unknown`; some tests need `python3` and `git`. Tests never call a real model: they use the scripted model in `hame-eval` (set `HAME_SCRIPT=<file.jsonl>` to replay a recording through the real binary). A few opt-in tests call a real provider and say so at the top of the file.

## Changes that affect what the model does

Prompts, tool descriptions, edit formats and verification hooks change how well the agent works, and the effect is easy to misjudge. For such a change, please run the relevant eval suite before and after (`hame eval evals/harder --runs 3`, or `evals/expert` / `evals/frontier` for harder work) and put the numbers in the pull request: pass rate, tokens and time. Say which model you used.

## Style

- Code, comments and commit messages in English.
- Comments explain why, not what.
- Keep the sandbox and secret-handling guarantees: a change must not let tools write, read or connect where the README says they cannot.
- Add a line to [CHANGELOG.md](CHANGELOG.md) under "Unreleased" for user-visible changes.

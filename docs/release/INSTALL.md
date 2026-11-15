# Installing hame 0.1.0-rc.1

hame is distributed as source for this release candidate. It builds into one binary, `hame`, that contains every interface (terminal, web, ACP), the sandbox helper and the browser and desktop helper sources.

## Prerequisites

| Needed for | What | Check |
|---|---|---|
| building | Rust 1.89 or later (`rustup`), a C toolchain (Xcode command line tools on macOS, `build-essential` on Debian/Ubuntu, MSVC on Windows) | `rustc --version` |
| running | `git` (checkpoints, rewind), `bash` (the shell tool; Git for Windows' bash on Windows) | `hame doctor` |
| a model | an API key: `ANTHROPIC_API_KEY` or `CLAUDE_API_KEY`, `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, or `HAME_API_KEY` for a compatible server | `hame doctor --ping` |
| shell sandbox | macOS (Seatbelt, built in); Linux 6.7+ (Landlock and seccomp); Windows: writes only, accept with `allow_partial = true` | `hame doctor` (`sandbox` line) |
| optional | `python3` (syntax checks of Python edits, the `script` tool) | `hame doctor` |
| browser tool | Python 3 with Playwright for Python importable in isolated mode, and Google Chrome (or `browser_channel = "chromium"` after `python3 -m playwright install chromium`) | `python3 -I -c "import playwright.async_api"`; `hame doctor` (`browser` line) |
| desktop tool | macOS 14 or later, `swiftc` (Xcode command line tools: `xcode-select --install`), Accessibility (and for screenshots Screen Recording) granted to the terminal that runs hame | `hame doctor` (`desktop` line) |
| running the test suite | `rustup target add wasm32-unknown-unknown`, `python3`, `git`; about 1–2 GB of memory per parallel link job | `cargo test --workspace` |

## Build from source

```sh
git clone https://github.com/wbcodepro/agent-harness hame && cd hame
git checkout v0.1.0-rc.1            # once the tag exists

# Either install into ~/.cargo/bin:
cargo install --locked --path crates/hame-cli
# or build target/release/hame:
cargo build --release --locked -p hame-cli
```

`--locked` builds exactly the dependency versions in `Cargo.lock`, the ones listed in [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) and [sbom.cdx.json](sbom.cdx.json). Building from the repository URL also works: `cargo install --locked --git https://github.com/wbcodepro/agent-harness hame-cli`.

### Browser tool (optional)

```sh
# Any Python 3 whose `python3 -I` sees Playwright. With Homebrew or a system Python that
# refuses `pip install` (PEP 668), use a virtual environment first on PATH:
python3 -m venv ~/.hame/venv
~/.hame/venv/bin/pip install playwright
export PATH="$HOME/.hame/venv/bin:$PATH"   # in the shell that starts hame
# Google Chrome installed normally is used by default; otherwise:
python3 -m playwright install chromium       # and set browser_channel = "chromium"
```

`pip install --user playwright` does not work: the helper runs Python in isolated mode (`-I`), which ignores the per-user site directory and `PYTHONPATH`.

### Desktop tool (optional, macOS)

```sh
xcode-select --install       # provides swiftc
```

Then grant **Accessibility** (and **Screen & System Audio Recording** for screenshots) to the app you start hame from (Terminal, iTerm, your editor) in System Settings > Privacy & Security, and restart that app. The helper is compiled on first use (about 4 seconds) into `~/.hame/helpers/`; `hame doctor` builds it, starts it and reports both permissions as the helper sees them.

## First run

```sh
hame setup          # choose a provider, paste the key (hidden; saved to ~/.hame/.env, mode 600), checks
hame doctor         # the same checks at any time; --ping sends one tiny request
cd your/project
hame                # terminal interface
hame run "explain what this repository does"
```

To try computer control, add to `~/.hame/config.toml`:

```toml
[computer]
browser = true

[[computer.grant]]
channel = "browser"
origin = "http://127.0.0.1:*"
capabilities = ["observe", "interact"]
```

and see the README sections "Browser and desktop control" and "Tasks with a verified outcome".

`hame doctor` prints one line per check: `ok`, `FAIL` (hame will not work as configured; the line says what to do) or `--` (optional and missing). Browser and desktop requirements count as failures only when the configuration turns those tools on.

## Uninstall

`cargo uninstall hame-cli` (or delete the binary). hame's data is in `~/.hame` (sessions, artifacts, browser profiles, helpers, keys in `.env`); delete it to remove everything. Workspace checkpoints live in that directory too, not in your repositories.

## Clean-install verification for 0.1.0-rc.1

Recorded on 2026-11-15 from branch `worktree-agent-a21de12c1ce6c5e33` (commits `862eebf`…`34cf61c`, version `0.1.0-rc.1`).

### macOS arm64

Machine: macOS 26.6.2, Apple M5, rustc 1.99.1, Swift 6.4.2, Python 3.14.5 with Playwright, Google Chrome installed; Accessibility and Screen Recording granted to the terminal.

```sh
S=<scratch directory>
cargo install --locked --path crates/hame-cli --root $S/install --target-dir $S/install-target
# Finished `release` profile [optimized] target(s) in 1m 43s; installed $S/install/bin/hame (44 MB)
cd $S/emptyproj
$S/install/bin/hame --version          # hame 0.1.0-rc.1
$S/install/bin/hame --help             # lists run, task, setup, doctor, ..., workflow, sessions
env -u ANTHROPIC_API_KEY -u CLAUDE_API_KEY -u OPENAI_API_KEY -u DEEPSEEK_API_KEY \
  HAME_HOME=$S/mac-home $S/install/bin/hame doctor
```

```text
hame 0.1.0-rc.1
ok   home      …/mac-home (not created yet)
ok   config    none; using defaults
FAIL model     no API key found. Put one of DEEPSEEK_API_KEY, ANTHROPIC_API_KEY (or CLAUDE_API_KEY), OPENAI_API_KEY in the environment or in ~/.hame/.env (or $HAME_HOME/.env), or run `hame setup`, which asks for one and saves it there
ok   sandbox   tools run confined by Seatbelt; a write outside the workspace was refused
ok   bash      GNU bash, version 3.2.57(1)-release (arm64-apple-darwin25)
ok   git       git version 2.56.0
ok   python    Python 3.14.5
ok   artifacts …/mac-home/artifacts is writable (created on first use)
ok   browser   Playwright for Python and the chrome browser found; not enabled ([computer] browser = true or a browser grant turns it on)
ok   desktop   helper ready, Accessibility granted, Screen Recording granted; not enabled ([computer] desktop = true or a desktop grant turns it on)

1 check(s) failed; the lines above say what to do.
```

Exit status 1, as expected without a key. With a placeholder `DEEPSEEK_API_KEY` set (no request sent), every line is `ok` and the exit status is 0. The first doctor run built the desktop helper into `$S/mac-home/helpers/`.

The README's task-file, workflow and `[computer]` examples were extracted and parsed by this build: `hame task task.toml` got past parsing to the missing-key error; `hame workflow list|show|trust --yes|untrust` worked on the workflow example; the `[computer]` block loaded as `config.toml`.

`cargo test --workspace --no-fail-fast` in the worktree on this machine (browser tests ran against the fixture app with Chrome): **340 passed, 0 failed, 16 ignored** (opt-in live-provider and desktop GUI tests).

### Linux arm64 (Docker, clean)

```sh
open -a Docker                                   # Docker Desktop 29.4.3, 10 CPUs, 7.75 GiB
scripts/docker-clean-install.sh HEAD             # git archive HEAD | docker run -i rust:1-bookworm …
```

The script exports the committed tree with `git archive` (no `target/`, `.env`, `.claude/` or `.solutions/`; the inner script fails if any is present), extracts it into a fresh `rust:1-bookworm` container with no cargo cache, adds the `wasm32-unknown-unknown` target as root, and builds and tests as an unprivileged user (`scripts/clean-install-inner.sh`).

Results (commit `e0e44c2`; an earlier run at `34cf61c` passed 339):

| Step | Result |
|---|---|
| Environment | Linux 6.12.76-linuxkit aarch64, Debian GNU/Linux 12, rustc 1.100.0, git 2.39.5, Python 3.11.2 (no Playwright, no Chrome) |
| `cargo build --release -p hame-cli` | ok, 65 s |
| `hame --help`, `hame --version` | ok, `hame 0.1.0-rc.1` |
| `hame doctor` (empty `HAME_HOME`, no key) | exit 1 for the missing key only; `sandbox` ok ("tools run confined by Landlock and seccomp; a write outside the workspace was refused"); `artifacts` ok; `browser` `--` (Playwright not importable; not enabled); `desktop` `--` (macOS only; not enabled) |
| `cargo test --workspace --no-fail-fast -j 3` | **376 passed, 0 failed, 5 ignored**; browser tests print `SKIPPED (browser tests need Python Playwright and Chrome)` and return; desktop GUI and live-provider tests are `#[ignore]` |

The first attempt with the default job count failed to link (`ld terminated with signal 9`: out of memory with 10 parallel links in a 7.75 GiB VM); the script now passes `-j 3` (`HAME_TEST_JOBS` overrides it). That is a resource limit of the container, not a code issue. The first run at `2af4f82` failed to compile `hame-desktop`'s tests: one macOS-only test lacked its `cfg` (fixed in `e0e44c2`).

Not verified: Linux x86_64, macOS x86_64, and Windows for this release candidate (the `ci` workflow covers Ubuntu, macOS and Windows but was not run, to save GitHub Actions minutes).

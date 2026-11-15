# Pilot task set

The diagnostic tasks of the unified-harness roadmap (§5), run with
`hame eval evals/pilot`. Defined: the file tasks F01–F05, the browser tasks
B01–B05, the desktop tasks D01, D03 and D04 (TextEdit, macOS), the
cross-application demonstration X01 and the recovery tasks R01–R05. The
prompt-injection suite I01–I04 is a separate manifest,
`hame eval evals/pilot/injection.toml`, and is never counted as task
success. Results and reproduction commands: `docs/release/EVIDENCE.md`.

Not defined yet: D02 (application preference), D05 (document appearance),
O01–O05 (office documents) and X02–X05. They need fixtures and checkers
that this pass did not build; they are left out rather than listed as
failing.

## Layout

```text
manifest.toml        task ids, prompts, channels, checks, limits, version
injection.toml       the prompt-injection suite (same format)
fixture/webapp.py    the fixture web application (fresh state per run)
fixture/pilot.py     shared setup, fault injection, checker and teardown helpers
fixture/axtool.swift accessibility tool for checkers and teardown (independent of hame's helper)
F01/files/           the fixture: copied into a fresh workspace for every run
F01/check/           the hidden checker (check.py); never copied, exported as $HAME_CHECK_DIR
F01/solution/        a reference solution (solve.py, run in a workspace)
```

The whole directory is added to the sandbox's `unreadable` list for every
run, so the agent's tools can read neither checkers nor solutions. (Where
the sandbox confines only writes, and for agents run with `--command`, that
protection does not apply.) `cargo test -p hame-cli --test pilot` checks
that every checker fails on the untouched fixture and passes on the
reference solution, starting each task's fixture server as the runner does.
Desktop tasks have no scripted solution; they are exercised by real runs.

## Manifest fields

| Field | Meaning |
|---|---|
| `task_set`, `version` | Name and version recorded as `task_set_version`; bump the version when anything a task depends on changes |
| `[defaults]` | `channels`, `requires`, `timeout_secs`, `check`, `server`, `teardown` used by tasks that do not set them |
| `id` | Unique task id; also the task's directory unless `dir` is set |
| `title`, `category` | `file`, `browser`, `desktop`, `office`, `cross_app`, `recovery` (others are accepted, e.g. `injection`) |
| `prompt`, `prompts` | English prompt; translations by language code, chosen with `--lang`. `{fixture_url}`, `{workspace}` and `{outbox}` are replaced |
| `channels` | How the agent may act (`files`, `shell`, `browser`, `desktop`, ...); recorded with each run |
| `requires` | Capabilities the runner must provide: `files` and `shell` always, `browser` when Python Playwright and Chrome are installed, `desktop` on macOS with a Swift compiler. A task needing one it lacks is reported `unsupported` and left out of every denominator |
| `config` | Merged into the agent's config.toml for this task, e.g. `{ computer = { browser = true, grant = [...] } }`. The arm's own settings win on conflicts; arrays (grants) are joined |
| `server` | A live fixture: started before setup, must print `PORT <n>` (or `URL <url>`) as its first line, serves until its input closes. Fresh for every run, stopped after the check and teardown |
| `setup` | Shell command run in the workspace after the fixture is copied and the server started (timed as setup, not task time) |
| `background` | Started beside the agent and stopped when it finishes: fault injection and interference |
| `resume` | When the background process killed the agent (it touched `$HAME_RUN_DIR/agent.killed`), the session is continued once with this prompt; the record counts one human intervention |
| `reset` | `fresh_copy`: new workspace, new agent data directory, cold start, for every run |
| `check` | Shell command run in the workspace afterwards; exit 0 is a pass; its last lines go into the record's note |
| `teardown` | Run after the check whatever happened, to put the machine back; not timed |
| `forbidden` | Side effects the task forbids, in words; the checker enforces them |
| `unchanged` | Paths (a file, or a directory ending in `/`) the runner verifies by hash; a change is a forbidden effect and fails the run |
| `timeout_secs` | The agent's time limit; `--timeout` overrides it |

Every command a task runs (server, setup, background, check, teardown) gets:

| Variable | Value |
|---|---|
| `HAME_FIXTURE_URL` | The server's address |
| `HAME_FIXTURE_TOKEN` | A fresh secret per run; the fixture's `/__state` answers only to it. The agent never sees it |
| `HAME_RUN_DIR` | A private directory per run, unreadable to the agent's tools; `agent.pid` holds the agent's pid while it runs |
| `HAME_OUTBOX` | A folder outside the workspace that the agent's shell and file tools can neither read nor write, so a file there was saved by an application |
| `HAME_WORKSPACE`, `HAME_AGENT_HOME` | The agent's working directory and data directory (sessions, browser profiles) |
| `HAME_SUITE_DIR`, `HAME_TASK_DIR`, `HAME_CHECK_DIR`, `HAME_TASK_ID` | Where the task set, the task and its checker live |

## Outcomes

`pass` (checker passed, nothing forbidden happened), `fail` (checker failed
and the agent said it was blocked), `false_success` (the agent claimed
success and the checker failed), `timeout`, `error` (setup failed, the
agent could not start, or exited with an error), `unsupported`. A claim of
success is a normal exit, a last turn that ended `completed`, and a final
answer whose lead (up to the first blank line) has no blocker phrase
("could not", "unable to", "I did not", "không thể", ...; see
`hame_eval::report::BLOCKER_MARKERS`).

## Live tasks and the machine

Browser tasks run headless Chrome with profiles under the run's agent data
directory. Desktop tasks drive TextEdit: setup launches it if needed and
remembers which windows were open; teardown closes only the windows the run
opened (discarding changes) and quits TextEdit if the run started it. A
save panel left at its default folder writes into the user's iCloud
TextEdit folder or Documents; the desktop checkers fail a run that put a
file there, and teardown moves such files (from TextEdit's iCloud folder,
or named after the run) into the run directory. Run desktop tasks with
nobody using the machine: they take the keyboard focus.

## Adding tasks

Checkers must read the fixture's state (`pilot.state()`, saved files,
`pilot.textedit_windows()`), never the agent's own report. Until the runner
provides a capability, list it in `requires` so the task shows up as
unsupported instead of failing.

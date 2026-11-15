# Pilot task set

The diagnostic tasks of the unified-harness roadmap (§5), run with
`hame eval evals/pilot`. Only the five file tasks F01–F05 exist so far.

## Layout

```text
manifest.toml        task ids, prompts, channels, checks, limits, version
F01/files/           the fixture: copied into a fresh workspace for every run
F01/check/           the hidden checker (check.py); never copied, exported as $HAME_CHECK_DIR
F01/solution/        a reference solution (solve.py, run in a workspace)
```

The whole directory is added to the sandbox's `unreadable` list for every
run, so the agent's tools can read neither checkers nor solutions. (Where
the sandbox confines only writes, and for agents run with `--command`, that
protection does not apply.) `cargo test -p hame-cli --test pilot` checks
that every checker fails on the untouched fixture and passes on the
reference solution.

## Manifest fields

| Field | Meaning |
|---|---|
| `task_set`, `version` | Name and version recorded as `task_set_version`; bump the version when anything a task depends on changes |
| `[defaults]` | `channels`, `requires`, `timeout_secs`, `check` used by tasks that do not set them |
| `id` | Unique task id; also the task's directory unless `dir` is set |
| `title`, `category` | `file`, `browser`, `desktop`, `office`, `cross_app`, `recovery` |
| `prompt`, `prompts` | English prompt; translations by language code, chosen with `--lang` |
| `channels` | How the agent may act (`files`, `shell`, `browser`, `desktop`, ...); recorded with each run |
| `requires` | Capabilities the runner must provide; a task needing one it lacks is reported `unsupported` and left out of every denominator |
| `setup` | Shell command run in the workspace after the fixture is copied (timed as setup, not task time) |
| `reset` | `fresh_copy`: new workspace, new agent data directory, cold start, for every run |
| `check` | Shell command run in the workspace afterwards; exit 0 is a pass |
| `forbidden` | Side effects the task forbids, in words; the checker enforces them |
| `unchanged` | Paths (a file, or a directory ending in `/`) the runner verifies by hash; a change is a forbidden effect and fails the run |
| `timeout_secs` | The agent's time limit; `--timeout` overrides it |

## Outcomes

`pass` (checker passed, nothing forbidden happened), `fail` (checker failed
and the agent said it was blocked), `false_success` (the agent claimed
success and the checker failed), `timeout`, `error` (setup failed, the
agent could not start, or exited with an error), `unsupported`. A claim of
success is a normal exit, a last turn that ended `completed`, and a final
answer without a blocker phrase ("could not", "unable to", "không thể",
...; see `hame_eval::report::BLOCKER_MARKERS`).

## Adding tasks

Browser, desktop, office, cross-app and recovery tasks use the same fields.
Their checkers must read the fixture's state (database rows, saved files,
reopened application settings), never the agent's own report. Until the
runner provides a capability, list it in `requires` so the task shows up as
unsupported instead of failing.

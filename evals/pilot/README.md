# Pilot task set

The 30 diagnostic tasks of the unified-harness roadmap (§5), run with
`hame eval evals/pilot`: the file tasks F01–F05, the browser tasks B01–B05,
the desktop tasks D01–D05 (TextEdit), the office tasks O01–O05 (Numbers and
Pages), the cross-application tasks X01–X05 and the recovery tasks R01–R05.
The prompt-injection suite I01–I04 is a separate manifest,
`hame eval evals/pilot/injection.toml`, and is never counted as task
success. Results and reproduction commands: `docs/release/EVIDENCE.md`.

| Task | What it asks | How the hidden checker decides |
|---|---|---|
| D02 | TextEdit setting: new documents as plain text | The preference read back, no other setting changed, not written with `defaults`; TextEdit quit and relaunched shows Plain text in Settings. Teardown restores the preference |
| D05 | Title bold 20 pt centred, one line italic, save in place (TextEdit, RTF) | Each paragraph's text and formatting through textutil; the Quick Look rendering shows a centred title taller than the body |
| O01 | Six receipts into a Numbers table, export to Excel | Cell values (amounts as numbers) and row completeness in the export; the open document shows the receipts |
| O02 | Line-total formulas, a SUM total, change a quantity, export | Formula references per row and over the total range; cached values after the change |
| O03 | Three text edits in a Pages document without changing formatting, export to Word | Text, every character's bold/italic/size and paragraph alignment against Pages' own export of the untouched document (`check/reference.docx`); line layout of both rendered by Quick Look |
| O04 | Column chart with title and value-axis name with unit, export | Chart XML in the export (kind, one series, categories Jan–Jun, values, titles); the chart on screen captured and its bars counted and compared with the data |
| O05 | Export a Pages document to PDF and reopen it in Preview | One A4 page, the text, title bold and larger, a bold phrase; the PDF open in Preview |
| X02 | Site items (all pages) into Numbers, export to the workspace, upload | Record coverage of the workbook; the uploaded bytes' hash equals the workbook's, once, on R-105; the open document shows the items |
| X03 | Edit an RTF file, preview it in TextEdit, export a PDF | New date and room in the file and the PDF (old ones gone), formatting kept, title bold and larger in the PDF, output only in the requested folder |
| X04 | Join the site's items with a Numbers price list, write the discrepancies | The exact discrepancy set (both sides, blanks for one-sided ids, sorted, two decimals); the price list unchanged |
| X05 | Write a report; a user correction arrives mid-task | The report follows the correction (EUR, region left out); the correction is in the session as a user message |

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
| `interject` | `{ when = "<shell>", message = "..." }`: a user message sent while the agent works. `when` runs beside the agent (in the workspace) and the message is sent when it exits 0, or as soon as the agent starts without it. The runner gives `hame run --inbox <dir>`; hame queues each new `*.msg` there for the model's next request (or before it would stop). If the agent finished first, the message is sent as the next turn of the same session. The record counts one human intervention and its note says which way it went (`user correction delivered during the turn` or `... as a follow-up turn`) |
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

## Office documents

Numbers and Pages documents are native files kept in each task's `assets/`
(made once by opening generated xlsx/docx files in Numbers/Pages 14.4 and
saving them, with header rows set so charts get labels). Setup copies the
file into the outbox and opens it with `pilot.py app-start <bundle> <file>`;
never open the copies in `assets/` themselves, since the apps rewrite a
file they open. Checkers read what the application exported (xlsx through
openpyxl, docx through its XML, PDF through PyMuPDF, RTF through textutil),
renderings through Quick Look, and the live document through
`axtool cells` / `axtool frame`. Shared readers are in `fixture/office.py`.
Each new checker has `check/test_check.py`, run without the API or GUI by
`cargo test -p hame-cli --test pilot` (it needs openpyxl, PyMuPDF and
Pillow). Numbers and Pages keep imported and new documents in their iCloud
folders; those folders count as default save locations for the stray-file
checks, and teardown moves the run's drafts out of them.

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
`pilot.textedit_windows()`, `pilot.app_cells()`), never the agent's own
report. Until the runner
provides a capability, list it in `requires` so the task shows up as
unsupported instead of failing.

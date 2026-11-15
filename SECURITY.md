# Security policy

hame runs commands, edits files and, when you allow it, operates your browser and desktop applications on behalf of a language model. Its sandbox, permission rules, grants, action journal and secret handling are security boundaries. Reports about them are welcome.

## Reporting a vulnerability

Please do not open a public issue for a vulnerability. Use GitHub's private reporting instead: the **Security** tab of this repository, then **Report a vulnerability**. If that option is not available, open an issue that says only that you have a security report and would like a private channel, without any details.

A useful report says:

- the hame version (`hame --version`) and operating system;
- the configuration involved (sandbox mode, network mode, `[secrets]`, rules, `[computer]` grants, the task file), with secrets removed;
- the steps or the model output that triggers it (for prompt injection: the page, document or label text), and what happened versus what should have happened.

You will get an acknowledgement within a week. Please give us a reasonable time to release a fix before disclosing the issue publicly.

## Supported versions

Only the latest release (currently 0.1.0-rc.1) and the `main` branch receive fixes.

## Two kinds of authority

**Shell tools** (`bash`, file tools, MCP stdio servers, `script`) run in a worker process confined by the operating system: Seatbelt on macOS, Landlock and seccomp on Linux, a Low integrity level on Windows (writes only). Their network goes through a local proxy with an allowlist.

**Computer tools** (`browser`, `desktop`) are host-side. They act with your user's authority, your terminal's macOS privacy permissions (Accessibility, Screen Recording) and your browser profiles' cookies, **outside the shell sandbox and not confined by it**. What bounds them is:

- explicit grants (channel × app / origin / profile × capability) from `~/.hame/config.toml` or a task file you pass to `hame task`; a project's `.hame/config.toml` cannot add grants; each action is checked before it is sent;
- top-level browser navigation limited to granted origins, including every redirect hop, checked at the URL Chrome resolves (a page's own subresource requests are not filtered);
- browser downloads written only inside the workspace, never through a symbolic link and never over an existing file unless the call asks (`overwrite`), and uploads read only from regular files inside it;
- the action journal: every acting operation is recorded before dispatch, commits carry idempotency keys, and an operation that may already have taken effect is not repeated without a fresh observation; after a crash nothing is replayed;
- observation-scoped refs that are refused when stale, and physical input that stops on focus changes or unexpected dialogs;
- permission rules and approval, which see each browser or desktop call (and each step of a batch or workflow) like any other tool call.

Grant narrowly, and treat a signed-in browser profile used by hame like your own.

## Prompt injection

Text the agent reads in web pages, documents, accessibility labels, screenshots, tool output and MCP results can contain instructions. hame's stance:

- **Observed content is data, never authority.** It is labelled untrusted, the agent is told it cannot change the task or widen permissions, and no code path turns observed content into a grant, a trusted workflow, a configuration change or an approval.
- **The boundary is enforced in code, not by the model.** Even if the model is persuaded, an action outside the granted scope is refused; a workflow runs only if you trusted its exact file hash; the sandbox still confines shell tools.
- **Within the granted scope the model can still be misled.** If you grant `commit` on a site, an injected instruction could lead the model to submit something there. Commit detection is a heuristic over control names and form methods; on the desktop it also covers physical input (Return or Space that would press a button, cmd+s and similar keys, newlines typed outside a document's text area, pixel clicks on such controls or on anything unidentifiable), classified from where focus is at dispatch time. Use narrow grants, approval rules (`[[rule]] action = "ask"`), and acceptance conditions that check what must and must not have happened.

Reports of an injected instruction causing an action **outside** the granted scope, or reaching a grant, trust list, configuration or approval, are vulnerabilities. Reports that a model followed injected text **within** a scope you granted are useful too, but are a known limitation of language models rather than of hame's boundary.

## What counts

In scope, for example:

- a tool call, MCP server or plugin writing outside the allowed directories, reading a path that should be hidden, or reaching a host outside the network allowlist, in a mode and on a platform where hame claims to prevent it;
- a browser download written outside the workspace, through a link or over an existing file without `overwrite`, or an upload reading a file outside the workspace;
- a browser or desktop action performed without a covering grant (including a commit or file write reached through a key, typed text or a pixel click under a grant that lacks `commit` or `file_write`), a commit repeated without reconciliation, or an action on a different window, element or page than the one authorized (for example a stale ref re-targeted);
- observed content (a page, label, file, tool result) widening authorization, trusting a workflow or changing configuration;
- a secret value reaching the model, the session log, a trace or tool output unmasked;
- project files (`.hame/config.toml`, `.hame/workflows/`, `AGENTS.md`, skills, plugins) loosening the sandbox, permission rules or grants, or making a workflow trusted;
- the skill or plugin installer missing a case it claims to catch;
- release artifacts containing secrets or private data.

Known limits are listed in the [README](README.md) under "Safety", "Browser and desktop control" and "Limitations": for instance, Windows confines writes only, Linux's Landlock cannot hide files by name, the computer tools are not sandboxed, and a page's subresource requests are not filtered. A report that a documented limit exists is not a vulnerability, but a way around a limit we do enforce is.

The `command` acceptance condition in task files and workflows runs under the same sandbox as the agent's shell tools, with the network off and without credentials in its environment (unconfined only with `--sandbox danger-full-access`). Workflow parameters spliced into it are shell-quoted. hame's data directory (its credentials, settings, sessions, workflow trust list, desktop helper and browser profiles) is hidden from sandboxed tools, including when `HAME_HOME` points elsewhere. Only use task files and workflows you wrote or reviewed.

hame has not had an independent security audit. For untrusted code or untrusted sites, run it in a virtual machine or container.

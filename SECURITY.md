# Security policy

hame runs commands and edits files on your machine on behalf of a language model, so its sandbox, permission rules and secret handling are security boundaries. Reports about them are welcome.

## Reporting a vulnerability

Please do not open a public issue for a vulnerability. Use GitHub's private reporting instead: the **Security** tab of this repository, then **Report a vulnerability**. If that option is not available, open an issue that says only that you have a security report and would like a private channel, without any details.

A useful report says:

- the hame version (`hame --version`) and operating system;
- the configuration involved (sandbox mode, network mode, `[secrets]`, rules), with secrets removed;
- the steps or the model output that triggers it, and what happened versus what should have happened.

You will get an acknowledgement within a week. Please give us a reasonable time to release a fix before disclosing the issue publicly.

## What counts

In scope, for example:

- a tool call, MCP server or plugin writing outside the allowed directories, reading a path that should be hidden, or reaching a host outside the network allowlist, in a mode and on a platform where hame claims to prevent it;
- a secret value reaching the model, the session log or tool output unmasked;
- project files (`.hame/config.toml`, `AGENTS.md`, skills, plugins) loosening the sandbox or permission rules;
- the skill or plugin installer missing a case it claims to catch.

Known limits are listed under "Known limits" in the [README](README.md#safety): for instance, Windows confines writes only, and Linux's Landlock cannot hide files by name. A report that a documented limit exists is not a vulnerability, but a way around a limit we do enforce is.

hame has not had an independent security audit. For untrusted code, run it in a virtual machine or container.

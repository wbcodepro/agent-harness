# Acceptance evidence

Reviewed source: `497fe24f3d10383466fb8e0f1ae545c4d6d6625a`, macOS arm64, 2026-11-15.

Uncommitted implementation edits appeared after the recorded check runs, while this report was being assembled. The results certify neither those later edits nor a future working tree; see the report's concurrent-edit boundary. The original `verifier-probe.log` is retained from before those edits.

The decision and findings are in [the acceptance report](../../ACCEPTANCE-2026-11-15.md). `results.json` records the check results and historical benchmark revisions. The log files retain test output; absolute repository and local audit-scratch paths are replaced with placeholders.

## Reproduce the verifier findings

On macOS, after building the workspace and fetching its locked dependencies:

```sh
python3 docs/release/evidence/acceptance-2026-11-15/reproduce.py
```

The launcher makes an isolated Cargo package using local workspace crates and copies the workspace lockfile as the dependency-resolution starting point. It runs Cargo offline, uses the repository's target cache, and never changes workspace source or its lockfile. The probe creates synthetic files in a temporary directory and uses the real macOS sandbox wrapper. It does not read the repository's `.env` or call any model API.

`probe.rs` calls the public verifier and `OutcomeGate` APIs. The first case checks that a confined process cannot read a fake `.env`, can create a symlink to it, and that the verifier then includes its fake marker in model-bound feedback. The remaining cases demonstrate false absence and incorrect CSV record counts. Temporary fixtures are removed after a successful run.

**This is a reproduction of defects, not a passing acceptance suite.** Its assertions deliberately expect the vulnerable behavior of the reviewed revision. After a fix, invert the expectations and move appropriate cases into the maintained regression suite; do not interpret a failed reproduction assertion as a product regression.

## Logs

- `workspace-tests.log`: ordinary workspace tests, including ignored-test declarations.
- `clippy.log`, `msrv.log`, `release-build.log`: compilation/check results.
- `live-api.log`: four enabled tests, five real Anthropic API requests; synthetic image and cache data only.
- `gui-first.log`: serialized GUI suite, 13 passed and one TextEdit startup failure.
- `gui-rerun.log`: the failed startup case passed when rerun separately.
- `verifier-probe.log`: successful reproduction of the reported defects.

Formatting passed with exit code 0 and no output. The one live MCP discovery test was not enabled. These files contain no API credentials; the real API key remained in the authorized test subprocess's environment.

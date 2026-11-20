# Test Snapshot Workflow

Soroban's `Env` test harness writes JSON artifacts for snapshot tests. The files record serialized contract observations such as emitted events and storage state, making changes to those externally meaningful details reviewable.

## Layout

A snapshot path mirrors the Rust test module path. Tests in `tests` use `test_snapshots/tests/<test_name>.json`; a top-level test module uses `test_snapshots/<module>/<test_name>.json`; nested modules add another directory segment. Keep the path aligned when moving or adding tests.

## Review and regeneration

Review snapshot diffs whenever a test or contract change can affect emitted events, event payloads, or storage layout. Run:

```bash
make check_snapshots
# or:
bash scripts/check_test_snapshots.sh
```

The script runs `cargo test --workspace`, which regenerates snapshots in place, then compares `*/test_snapshots/` with the committed working tree. If the behavior change is intentional, inspect the JSON diff, explain the event or storage change in the pull request, and commit the regenerated snapshot files. Do not commit snapshots that changed only because of an accidental behavior change.

## CI behavior

The contributor gate runs `make all`, which includes `make check_snapshots`. The drift script fails when snapshot files are modified, added, or removed after the test run. Consequently, an intentional snapshot change must be committed with the code change; accidental drift fails the pull request and must be investigated.

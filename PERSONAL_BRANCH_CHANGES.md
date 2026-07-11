# Personal Branch Changes

This file tracks local `personal` branch behavior that should survive future
upstream merges.

## Usage Report Cache

- Keep the Rust usage cache helper at `rust/crates/ccusage/src/adapter/cache.rs`.
- Keep report-summary caching wired for root Claude `daily`/`session` commands
  and `ccusage opencode` reports.
- Keep Codex per-file event caching and grouped-report caching wired into
  `ccusage codex` and all-agent Codex loading.
- Keep Codex `--speed auto` pricing based on recorded per-thread service-tier
  events for Codex CLI v0.144+ logs. Recorded `priority` and legacy `fast` are
  fast; recorded `default`, `flex`, missing, or unknown tiers are standard.
  Apply a recorded tier from the next `turn_context` only.
- Keep the personal historical fast-mode cutoff windows only for older Codex
  logs that do not have v0.144+ recorded thread-tier metadata.
- When Codex speed-pricing semantics change, bump the Codex event/group cache
  discriminators or cost-policy key so cached costs are not reused across
  incompatible rules.
- Cache files live under `$XDG_CACHE_HOME/ccusage/usage-rust`, or
  `~/.cache/ccusage/usage-rust` when `XDG_CACHE_HOME` is unset.
- Cache invalidation must include source file metadata plus cost-affecting
  options such as mode, offline, `--no-cost`, the effective timezone, date
  bounds, pricing overrides, and `CCUSAGE_MODEL_ALIASES`.
- Treat Codex files as append-only only when their byte length strictly grows;
  equal-size rewrites must rebuild cached groups.
- Keep fetched pricing JSON cached under `$XDG_CACHE_HOME/ccusage/pricing`, with
  short network timeouts, so non-`--offline` reports do not block repeatedly on
  LiteLLM or models.dev refreshes.

## OpenCode Personal Fast Paths

- Keep direct embedded pricing for `glm-5.2`, currently matching `glm-5.1`,
  including the 200k context limit fallback.
- Include `opencode.db-wal` in cache signatures so SQLite WAL writes invalidate
  cached reports even when `opencode.db` itself is unchanged.
- Keep OpenCode `Display`/`--no-cost` behavior from recalculating derived costs
  when OpenCode stored zero or missing cost values.

## Merge Check

After merging upstream, rebuild the Rust CLI and time warmed runs for:

```sh
LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage opencode daily --offline --json >/dev/null
LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage opencode session --offline --json >/dev/null
LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage daily --offline --json >/dev/null
LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage session --offline --json >/dev/null
```

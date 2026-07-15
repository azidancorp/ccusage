# Personal Branch Divergence Ledger

This is the authoritative inventory of intentional behavior on `personal` that
differs from `main`. Update it whenever `main` is merged, a personal policy is
changed, or an upstream change absorbs a divergence. Generated schemas,
snapshots, and documentation that merely expose one of these behaviors do not
need separate entries.

## Audit Baseline

- Last audited: 2026-07-15.
- Compared `personal` through `06d84397`, including the GLM context-limit and
  responsive-table fixes, against `main` at `ba99c0d09b6d`.
- `personal` is currently missing `main` commits `dad1b5d8` and `ba99c0d0`.
  Those benchmark-fixture build changes are branch lag, not personal policy.
- The `backup/personal-*` refs are ancestors of `personal`; they contain no
  additional divergence that needs to be recovered.

## Codex Cost and Token Accounting

Origins: `4dfd76cc`, `cd324ec6`, and `24224a53`.

- For Codex CLI v0.144.0 and newer session logs, keep `--speed auto` pricing
  based on each thread's persisted `thread_settings_applied` event. Recorded
  `priority` and legacy `fast` tiers are fast; `default`, `flex`, missing, and
  unknown tiers are standard. A new setting applies only after the following
  `turn_context`.
- Do not infer the speed of v0.144.0+ logs from the user's current Codex
  `config.toml`. The setting is thread-specific and may change during a session.
- Only pre-v0.144.0 logs without durable thread-tier metadata may use these
  personal fast-mode fallback windows, expressed as half-open UTC ranges:
  - `[2026-04-07T00:00:00.000Z, 2026-05-09T00:00:00.000Z)`
  - `[2026-05-14T19:33:00.000Z, 2026-07-10T09:06:20.097Z)`
- Keep explicit `--speed fast` and `--speed standard` as overrides of all
  automatic detection.
- Keep personal totals counting every Codex `token_count` row in
  `thread_spawn` subagent and `forked_from_id` session files, including copied
  parent history. This intentionally makes personal token and cost totals
  higher than `main` for fork-heavy workloads. Preserve the cumulative baseline
  so later real child usage is still calculated correctly. This is a personal
  accounting policy, not a claim that copied history was billed again.
- When any of these rules change, bump the Codex event/group cache
  discriminators or cost-policy key. Never reuse cached costs across incompatible
  tier or replay-accounting semantics.

Primary owners: `rust/crates/ccusage/src/adapter/codex/{parser,speed,loader,aggregate,report}.rs`.

## Persistent Usage and Pricing Caches

Origins: `bdd51021` and `0b90dd79`.

- Keep the shared Rust usage-cache helper in
  `rust/crates/ccusage/src/adapter/cache.rs`.
- Keep root Claude `daily`/`session` summary caching and OpenCode summary
  caching.
- Keep all-agent row caching for Claude, Codex, and OpenCode.
- Keep Codex per-file event caching and append-aware grouped-report caching.
  Treat a file as append-only only when its byte length strictly grows;
  equal-size rewrites must rebuild the affected groups.
- Cache files live under `$XDG_CACHE_HOME/ccusage/usage-rust`, or
  `~/.cache/ccusage/usage-rust` when `XDG_CACHE_HOME` is unset.
- Cache invalidation must cover source metadata and every cost-affecting option,
  including report kind, cost mode, offline/`--no-cost`, effective timezone,
  date bounds, pricing overrides, Codex speed policy, and
  `CCUSAGE_MODEL_ALIASES`.
- Keep fetched LiteLLM and models.dev JSON under
  `$XDG_CACHE_HOME/ccusage/pricing`, use short network timeouts, and retain stale
  valid JSON as a network-failure fallback.

## OpenCode and GLM Pricing

Origins: `2a37bfce` and GLM-5.2 context correction `103f5ad6`.

- Keep explicit embedded Z.ai pricing for `glm-5.2`: $1.40 per million input
  tokens, $0.26 per million cached-input tokens, $4.40 per million output
  tokens, and no cache-create/storage charge.
- Keep the GLM-5.2 context limit at 1,000,000 tokens. GLM-5.1 currently shares
  the same token rates but retains its 200,000-token context limit.
- Include `opencode.db-wal` in source signatures so SQLite WAL writes invalidate
  cached reports even when `opencode.db` itself is unchanged.
- Keep OpenCode `Display`/`--no-cost` behavior from calculating a derived cost
  when OpenCode stored a zero or omitted cost. Explicit calculate mode may use
  embedded pricing.

Primary owners: `rust/crates/ccusage/src/pricing.rs` and
`rust/crates/ccusage/src/adapter/opencode/{loader,parser,report}.rs`.

## Kimi Source Fidelity

Origins: `ab66e270`, `0607acdb`, and `1336a2b6`.

- Keep discovery of nested `subagents/<id>/wire.jsonl` and newer
  `agents/<id>/wire.jsonl` streams, including nested `SubagentEvent` /
  `StatusUpdate` token usage.
- Keep stream-scoped deduplication: identical message IDs in parent and child
  streams are separate usage, while duplicates within one stream are not.
- Resolve the configured display model in this order: `KIMI_MODEL_NAME`,
  `config.toml`, legacy `config.json`, then the `kimi-for-coding` fallback.
- Preserve configured display slugs while mapping `kimi-for-coding` variants to
  the timestamp-appropriate K2.5 or K2.6 pricing candidate.

Primary owners: `rust/crates/ccusage/src/adapter/kimi/{paths,parser,loader}.rs`.

## Unified Report Presentation

Origins: `87597235` and full-date restoration `06d84397`.

- Hide the Models column in all-agent tables by default. Keep
  `--with-models` as an all-agent-only opt-in; `--breakdown` also shows it.
- Keep complete `YYYY-MM-DD` values visible in date-grouped tables whenever the
  table's minimum full-date layout fits. If it cannot fit, compact or split the
  date instead of replacing the day with an ellipsis.

Primary owners: `rust/crates/ccusage/src/adapter/all/report.rs`,
`rust/crates/ccusage-cli`, and `rust/crates/ccusage-terminal/src/table.rs`.

## Antigravity Heuristic Adapter

Origins: `1c1cbfd0`, `925413c9`, `6a0db490`, and `51c15779`.

- Keep `ccusage antigravity` and Antigravity rows in unified all-agent reports.
- Read `${ANTIGRAVITY_DATA_DIR:-~/.gemini/antigravity-cli}` conversation
  databases without modifying them.
- Preserve the documented heuristic behavior: estimate tokens at 3.8
  characters per token, reset cached-context assumptions on user messages, and
  attribute estimates to Gemini 3.5 Flash pricing. These estimates are not
  official Antigravity token or billing records.
- Keep the CLI, configuration schema, help, tests, and user-facing documentation
  for the adapter aligned.
- Treat `antigravity-analysis/` as historical provenance only. Runtime code and
  current docs are authoritative; the older integration blueprint is not.

Primary owner: `rust/crates/ccusage/src/adapter/antigravity`.

## Retired or Rejected Divergences

- Do not restore the obsolete GLM-5.1/5.2 $0.98-per-million input and
  $3.08-per-million output override. The current official rates are recorded
  above.
- Do not replace thread-recorded Codex v0.144.0+ service tiers with a single
  current global config value or extend a historical cutoff beyond the first
  durable tier records.

## Merge Audit Checklist

After every merge from `main`:

1. Compare both commit policy and resulting trees:

   ```sh
   git log --left-right --cherry-pick --oneline main...personal
   git diff --name-status main...personal
   git diff --name-status main personal
   ```

2. Classify each difference as active, retired/upstreamed, branch lag, generated
   output, or accidental merge residue. Update the baseline and this ledger in
   the same change.

3. Run focused Codex tier/replay, GLM pricing, Kimi, all-agent layout, cache, and
   Antigravity tests, followed by the Rust workspace test suite.

4. Rebuild the personal release binary. Verify that `ccusage codex daily --help`
   describes recorded thread tiers with historical fallback; a binary built
   from `main` can otherwise make correct personal source look broken.

5. Smoke-test warmed cached reports:

   ```sh
   LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage opencode daily --offline --json >/dev/null
   LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage opencode session --offline --json >/dev/null
   LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage daily --offline --json >/dev/null
   LOG_LEVEL=0 NO_COLOR=1 COLUMNS=200 TZ=UTC ./rust/target/release/ccusage session --offline --json >/dev/null
   ```

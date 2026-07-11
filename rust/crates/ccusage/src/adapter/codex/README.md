# Codex Source

Data source:

```text
${CODEX_HOME:-~/.codex}/sessions/
${CODEX_HOME:-~/.codex}/archived_sessions/
```

When both directories contain the same relative JSONL path for one Codex home,
the active `sessions/` copy wins.

Relevant JSONL event:

- `type === "event_msg"`
- `payload.type === "token_count"`
- `payload.info.total_token_usage` is cumulative.
- `payload.info.last_token_usage` is the current turn delta.
- If only cumulative totals exist, subtract prior totals to recover deltas.

Token mapping:

- `input_tokens` - total input tokens.
- `cached_input_tokens` - cached prompt tokens.
- `output_tokens` - completion tokens, including reasoning cost.
- `reasoning_output_tokens` - informational breakdown; already included in output billing.
- `total_tokens` - provided directly or recomputed as input plus output for legacy entries.

Pricing uses model metadata from `turn_context`. Early sessions without metadata fall back to `gpt-5`, mark `isFallbackModel === true`, and expose fallback rows as approximate in aggregate JSON.

Speed pricing uses recorded thread service tiers when Codex logs include them.
Codex CLI v0.144+ persists `event_msg` records with
`payload.type === "thread_settings_applied"` and
`payload.thread_settings.service_tier`. `priority` and legacy `fast` map to
fast pricing. Other recorded tiers, including `default` and `flex`, map to
standard pricing. A recorded setting is applied from the next `turn_context` so
mid-turn changes do not reprice earlier usage.

Older logs without recorded thread-tier metadata fall back to the personal
historical fast-mode cutoff windows when `--speed auto` is used. Explicit
`--speed fast` and `--speed standard` override both recorded tiers and fallback
windows.

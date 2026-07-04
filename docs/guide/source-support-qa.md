# Source Support Q&A

ccusage only supports a coding agent when it can read local usage records with enough information to produce accurate reports. At minimum, a source needs local timestamps, session identity, model identity, and token counts or recorded costs that can be mapped to token usage.

If a tool stores only prompts, transcripts, quota percentages, or opaque cloud state, ccusage does not estimate token usage from text length in upstream-grade adapters. That would make daily, monthly, session, and cost reports look precise while being based on guesses.

## What Makes a Source Supportable?

A source is a good fit when its local files include most of the following:

- Per-message or per-turn token counts
- Input and output token counts, with cache and reasoning tokens when available
- Model identifiers for pricing
- Timestamps for date filtering and grouping
- Session or conversation identifiers
- Stable local file formats such as JSONL, SQLite tables, or structured telemetry exports

Local transcript text alone is not enough. A transcript can be useful for debugging, but it does not reveal tokenizer behavior, hidden system context, cached input, tool-call overhead, or provider-side accounting.

## Heuristic Sources

::: details Why is Antigravity CLI heuristic-only on this branch?
Antigravity CLI is separate from Gemini CLI. The Antigravity CLI binary is exposed as `agy`, and it stores state under `~/.gemini/antigravity-cli/`.

The local conversation database does not expose official input, output, cache, or reasoning token counts. This branch therefore supports `ccusage antigravity` with a personal heuristic instead of upstream-grade token accounting.

The heuristic reads strings from Antigravity conversation steps, converts characters to tokens with a `3.8 chars/token` estimate, and prices all rows as Gemini 3.5 Flash Standard. Every user message is treated as a cache reset: the next model request counts the full accumulated prompt as uncached, while autonomous continuation steps count previously sent context as cache-read and only newly added context as uncached.

Use these reports as personal estimates, not as exact provider billing records.
:::

## Unsupported Sources Investigated

::: details Why is Grok CLI not supported?
Grok CLI was investigated, but its local SQLite data did not contain usable token accounting. Without token counts, model usage, or recorded costs in the local database, ccusage has nothing reliable to aggregate.

Estimating tokens from message text would ignore provider-side context, hidden prompts, tool-call payloads, cached input, and tokenizer differences, so ccusage does not do that.
:::

::: details Why is Devin CLI not supported?
Devin CLI usage information appears to live in Devin's cloud service rather than in a local usage log that ccusage can read. The locally available data did not provide direct access to historical token usage or costs.

ccusage is a local, read-only analyzer. It does not scrape private cloud services or depend on undocumented authenticated APIs for user usage history. If Devin adds a local export with timestamps, sessions, models, and token counts, support can be revisited.
:::

## Can These Be Added Later?

Yes. Open an issue if a tool starts writing local usage data with token counts or exposes an official export. Useful examples include:

- A sample redacted log file
- The default data directory
- A description of which fields represent input, output, cache, reasoning, model, timestamp, and session ID
- Notes about whether costs are recorded or should be calculated from model pricing

Please do not share secrets, API keys, OAuth tokens, raw private prompts, or full conversation transcripts.

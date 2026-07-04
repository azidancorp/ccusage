use crate::cli::CodexSpeed;

const CODEX_FAST_COST_MULTIPLIER_START_MILLIS: i64 = 1_775_520_000_000; // 2026-04-07T00:00:00.000Z
const CODEX_FAST_COST_MULTIPLIER_END_MILLIS: i64 = 1_778_284_800_000; // 2026-05-09T00:00:00.000Z
const CODEX_FAST_COST_MULTIPLIER_RESTART_MILLIS: i64 = 1_778_787_180_000; // 2026-05-14T19:33:00.000Z

pub(crate) fn resolve_codex_speed_for_timestamp(
    requested: CodexSpeed,
    timestamp: crate::TimestampMs,
) -> CodexSpeed {
    match requested {
        CodexSpeed::Auto if is_codex_fast_window(timestamp) => CodexSpeed::Fast,
        CodexSpeed::Auto => CodexSpeed::Standard,
        speed => speed,
    }
}

fn is_codex_fast_window(timestamp: crate::TimestampMs) -> bool {
    let millis = timestamp.as_millis();
    (CODEX_FAST_COST_MULTIPLIER_START_MILLIS..CODEX_FAST_COST_MULTIPLIER_END_MILLIS)
        .contains(&millis)
        || millis >= CODEX_FAST_COST_MULTIPLIER_RESTART_MILLIS
}

#[cfg(test)]
mod tests {
    use super::resolve_codex_speed_for_timestamp;
    use crate::cli::CodexSpeed;

    #[test]
    fn resolves_auto_speed_by_personal_timestamp_windows() {
        let before_gap = crate::parse_ts_timestamp("2026-05-08T23:59:59.999Z").unwrap();
        let gap_start = crate::parse_ts_timestamp("2026-05-09T00:00:00.000Z").unwrap();
        let restart = crate::parse_ts_timestamp("2026-05-14T19:33:00.000Z").unwrap();

        assert_eq!(
            resolve_codex_speed_for_timestamp(CodexSpeed::Auto, before_gap),
            CodexSpeed::Fast
        );
        assert_eq!(
            resolve_codex_speed_for_timestamp(CodexSpeed::Auto, gap_start),
            CodexSpeed::Standard
        );
        assert_eq!(
            resolve_codex_speed_for_timestamp(CodexSpeed::Auto, restart),
            CodexSpeed::Fast
        );
        assert_eq!(
            resolve_codex_speed_for_timestamp(CodexSpeed::Standard, restart),
            CodexSpeed::Standard
        );
    }
}

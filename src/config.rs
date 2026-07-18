//! Configuration file support.
//!
//! `tsdb` reads an optional `.tsdb.toml` configuration file. Two locations are
//! searched, in precedence order:
//!
//! 1. **Project level** — `./.tsdb.toml` in the current working directory
//! 2. **User level** — `~/.tsdb.toml` in the user's home directory
//!
//! The first file found wins outright; settings are not merged across levels.
//! When neither file exists the built-in defaults apply, which reproduce the
//! behaviour of releases before configuration support existed.
//!
//! # Supported keys
//!
//! | Key          | Type   | Default | Meaning                                    |
//! |--------------|--------|---------|--------------------------------------------|
//! | `utc_offset` | string | `"Z"`   | Offset applied to written timestamps        |
//!
//! `utc_offset` accepts `Z`/`z`/`UTC`/`utc` (zero), or a signed offset in
//! `+HH:MM`, `-HH:MM`, `+HHMM`, `-HHMM`, `+HH` or `-HH` form. The offset is
//! applied to the `# YYYYMMDDhhmmss` trailer written at the end of every
//! `.dov` file, so the trailer reads in the operator's own wall-clock frame
//! instead of UTC.
//!
//! # Grammar
//!
//! A deliberately small, strict subset of TOML — enough for a flat table of
//! scalar settings and no more:
//!
//! ```text
//! # comment                     ; full-line comment
//! key = "value"                 ; double-quoted string
//! key = value                   ; bare value (no quotes, no spaces)
//! ```
//!
//! Blank lines and full-line comments are ignored. Trailing comments after a
//! value are **not** stripped — a `#` inside a value is part of that value.
//! Unknown keys, malformed lines and unparsable values are hard errors rather
//! than silent fallbacks: a configuration file that does not mean what its
//! author intended should say so loudly.

use crate::error::{Result, TsdbError};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Configuration file name, identical at project and user level.
pub const CONFIG_FILE_NAME: &str = ".tsdb.toml";

/// Resolved configuration.
///
/// The derived default is UTC (`utc_offset_secs == 0`), which reproduces the
/// behaviour of every release before configuration support existed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    /// Seconds to add to UTC when formatting written timestamps.
    pub utc_offset_secs: i64,
}

/// Process-wide cache. The configuration cannot change during a run, and
/// `current_timestamp` is called on every write, so the file is read at most
/// once per process.
static CACHE: OnceLock<Config> = OnceLock::new();

/// Return the active configuration, loading it on first use.
///
/// A malformed configuration file is reported on stderr and the defaults are
/// used, because timestamp formatting sits on the write path and must not be
/// able to fail a database write. Callers that need to surface the error
/// itself should use [`load_from`] directly.
pub fn active() -> &'static Config {
    CACHE.get_or_init(|| match discover_and_load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("tsdb: {}; using defaults", e);
            Config::default()
        }
    })
}

/// Search both levels and load the first configuration file found.
fn discover_and_load() -> Result<Config> {
    match discover() {
        Some(path) => load_from(&path),
        None => Ok(Config::default()),
    }
}

/// Return the path of the configuration file that applies, if any.
///
/// Project level takes precedence over user level.
pub fn discover() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    discover_in(cwd.as_deref(), home_dir().as_deref())
}

/// The precedence rule, isolated from the environment so it can be tested.
///
/// Kept separate from [`discover`] deliberately: reading `current_dir` and
/// `$HOME` inline would make the rule testable only by mutating process-global
/// state under a parallel test harness. The ordering of `[project, home]` is
/// the entire precedence guarantee, so it is the thing most worth pinning.
///
/// A candidate that exists but is not a regular file (a directory named
/// `.tsdb.toml`, say) is skipped rather than treated as a match.
fn discover_in(project_dir: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    for base in [project_dir, home].into_iter().flatten() {
        let candidate = base.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// The user's home directory, from `$HOME`.
fn home_dir() -> Option<PathBuf> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Some(PathBuf::from(h)),
        _ => None,
    }
}

/// Read and parse a configuration file.
pub fn load_from(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| TsdbError::Other(format!("cannot read config {}: {}", path.display(), e)))?;
    parse_config(&text).map_err(|e| match e {
        TsdbError::ParseError { line, message } => TsdbError::Other(format!(
            "config {} line {}: {}",
            path.display(),
            line,
            message
        )),
        other => other,
    })
}

/// Parse the configuration grammar described in the module documentation.
pub fn parse_config(text: &str) -> Result<Config> {
    let mut cfg = Config::default();

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = split_key_value(line, line_no)?;

        match key {
            "utc_offset" => {
                cfg.utc_offset_secs =
                    parse_utc_offset(value).ok_or_else(|| TsdbError::ParseError {
                        line: line_no,
                        message: format!(
                            "invalid utc_offset {:?}; expected Z, UTC, or +HH:MM / -HH:MM",
                            value
                        ),
                    })?;
            }
            other => {
                return Err(TsdbError::ParseError {
                    line: line_no,
                    message: format!("unknown key {:?}", other),
                });
            }
        }
    }

    Ok(cfg)
}

/// Split a `key = value` line, stripping one layer of double quotes.
fn split_key_value(line: &str, line_no: usize) -> Result<(&str, &str)> {
    let eq = line.find('=').ok_or_else(|| TsdbError::ParseError {
        line: line_no,
        message: format!("expected `key = value`, found {:?}", line),
    })?;

    let key = line[..eq].trim();
    let value = line[eq + 1..].trim();

    if key.is_empty() {
        return Err(TsdbError::ParseError {
            line: line_no,
            message: "empty key".to_string(),
        });
    }

    let value = match value.strip_prefix('"') {
        Some(rest) => rest
            .strip_suffix('"')
            .ok_or_else(|| TsdbError::ParseError {
                line: line_no,
                message: format!("unterminated string {:?}", value),
            })?,
        None => value,
    };

    Ok((key, value))
}

/// Parse a UTC offset into seconds east of UTC.
///
/// Accepts `Z`, `z`, `UTC`, `utc`, and signed `+HH:MM` / `-HH:MM` /
/// `+HHMM` / `-HHMM` / `+HH` / `-HH`. Returns `None` on anything else,
/// including out-of-range hour or minute components.
pub fn parse_utc_offset(s: &str) -> Option<i64> {
    let s = s.trim();

    if s.eq_ignore_ascii_case("z") || s.eq_ignore_ascii_case("utc") {
        return Some(0);
    }

    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1i64, &s[1..]),
        b'-' => (-1i64, &s[1..]),
        _ => return None,
    };

    let (hh, mm) = match rest.find(':') {
        // A colon commits the caller to writing minutes. `+08:` is far more
        // likely a truncated value than a deliberate "+08:00", so reject it
        // rather than quietly assuming zero minutes.
        Some(i) if i + 1 < rest.len() => (&rest[..i], &rest[i + 1..]),
        Some(_) => return None,
        None => match rest.len() {
            2 => (rest, ""),
            4 => (&rest[..2], &rest[2..]),
            _ => return None,
        },
    };

    if hh.len() != 2 || !hh.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = hh.parse().ok()?;

    let minutes: i64 = if mm.is_empty() {
        0
    } else {
        if mm.len() != 2 || !mm.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        mm.parse().ok()?
    };

    // Real-world offsets span UTC-12:00 to UTC+14:00; reject anything outside
    // a day either way, and reject minute values that are not minutes.
    if hours > 24 || minutes > 59 {
        return None;
    }
    let total = hours * 3600 + minutes * 60;
    if total > 24 * 3600 {
        return None;
    }

    Some(sign * total)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod tmp {
        use std::path::{Path, PathBuf};
        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("tsdb_config_test_{:016x}", rand::random::<u64>()));
                std::fs::create_dir_all(&path).unwrap();
                TempDir { path }
            }
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    fn write_config(dir: &Path, body: &str) {
        std::fs::write(dir.join(CONFIG_FILE_NAME), body).unwrap();
    }

    // -- discover_in: the precedence rule -----------------------------------

    #[test]
    fn discover_prefers_project_over_user() {
        let proj = tmp::TempDir::new();
        let home = tmp::TempDir::new();
        write_config(proj.path(), "utc_offset = \"+08:00\"");
        write_config(home.path(), "utc_offset = \"-05:30\"");
        assert_eq!(
            discover_in(Some(proj.path()), Some(home.path())),
            Some(proj.path().join(CONFIG_FILE_NAME))
        );
    }

    #[test]
    fn discover_falls_back_to_user_when_no_project_file() {
        let proj = tmp::TempDir::new();
        let home = tmp::TempDir::new();
        write_config(home.path(), "utc_offset = \"-05:30\"");
        assert_eq!(
            discover_in(Some(proj.path()), Some(home.path())),
            Some(home.path().join(CONFIG_FILE_NAME))
        );
    }

    #[test]
    fn discover_returns_none_when_neither_exists() {
        let proj = tmp::TempDir::new();
        let home = tmp::TempDir::new();
        assert_eq!(discover_in(Some(proj.path()), Some(home.path())), None);
    }

    #[test]
    fn discover_tolerates_absent_cwd_or_home() {
        let home = tmp::TempDir::new();
        write_config(home.path(), "utc_offset = \"Z\"");
        // No cwd (current_dir failed): user level still applies.
        assert_eq!(
            discover_in(None, Some(home.path())),
            Some(home.path().join(CONFIG_FILE_NAME))
        );
        // No HOME set: absence is not an error.
        assert_eq!(discover_in(None, None), None);
    }

    #[test]
    fn discover_ignores_a_config_that_is_a_directory() {
        // `.tsdb.toml` existing as a directory must not count as a match; the
        // search falls through to the next level rather than erroring.
        let proj = tmp::TempDir::new();
        let home = tmp::TempDir::new();
        std::fs::create_dir_all(proj.path().join(CONFIG_FILE_NAME)).unwrap();
        write_config(home.path(), "utc_offset = \"+08:00\"");
        assert_eq!(
            discover_in(Some(proj.path()), Some(home.path())),
            Some(home.path().join(CONFIG_FILE_NAME))
        );
    }

    #[test]
    fn discovered_project_config_round_trips_through_load_from() {
        // Ties the precedence rule to the value actually applied, so a future
        // change cannot satisfy discovery while loading the wrong file.
        let proj = tmp::TempDir::new();
        let home = tmp::TempDir::new();
        write_config(proj.path(), "utc_offset = \"+08:00\"");
        write_config(home.path(), "utc_offset = \"-05:30\"");
        let found = discover_in(Some(proj.path()), Some(home.path())).unwrap();
        assert_eq!(load_from(&found).unwrap().utc_offset_secs, 8 * 3600);
    }

    // -- parse_utc_offset ---------------------------------------------------

    #[test]
    fn offset_zulu_and_utc_are_zero() {
        assert_eq!(parse_utc_offset("Z"), Some(0));
        assert_eq!(parse_utc_offset("z"), Some(0));
        assert_eq!(parse_utc_offset("UTC"), Some(0));
        assert_eq!(parse_utc_offset("utc"), Some(0));
    }

    #[test]
    fn offset_colon_form() {
        assert_eq!(parse_utc_offset("+08:00"), Some(8 * 3600));
        assert_eq!(parse_utc_offset("-05:30"), Some(-(5 * 3600 + 30 * 60)));
    }

    #[test]
    fn offset_compact_and_hour_only_forms() {
        assert_eq!(parse_utc_offset("+0800"), Some(8 * 3600));
        assert_eq!(parse_utc_offset("-0530"), Some(-(5 * 3600 + 30 * 60)));
        assert_eq!(parse_utc_offset("+08"), Some(8 * 3600));
        assert_eq!(parse_utc_offset("-05"), Some(-5 * 3600));
    }

    #[test]
    fn offset_surrounding_whitespace_ignored() {
        assert_eq!(parse_utc_offset("  +08:00  "), Some(8 * 3600));
    }

    #[test]
    fn offset_rejects_unsigned_and_junk() {
        assert_eq!(parse_utc_offset("08:00"), None);
        assert_eq!(parse_utc_offset(""), None);
        assert_eq!(parse_utc_offset("+8:00"), None); // hour must be 2 digits
        assert_eq!(parse_utc_offset("+08:0"), None); // minute must be 2 digits
        assert_eq!(parse_utc_offset("+ab:cd"), None);
        assert_eq!(parse_utc_offset("+08:00:00"), None);
    }

    #[test]
    fn offset_rejects_dangling_colon() {
        // A trailing colon reads as a truncated value; do not treat it as
        // "+08:00" by defaulting the minutes.
        assert_eq!(parse_utc_offset("+08:"), None);
        assert_eq!(parse_utc_offset("-05:"), None);
    }

    #[test]
    fn offset_rejects_out_of_range() {
        assert_eq!(parse_utc_offset("+25:00"), None);
        assert_eq!(parse_utc_offset("+08:60"), None);
        assert_eq!(parse_utc_offset("+24:01"), None);
    }

    #[test]
    fn offset_accepts_boundaries() {
        assert_eq!(parse_utc_offset("+24:00"), Some(24 * 3600));
        assert_eq!(parse_utc_offset("-24:00"), Some(-24 * 3600));
        assert_eq!(parse_utc_offset("+00:00"), Some(0));
    }

    // -- parse_config -------------------------------------------------------

    #[test]
    fn empty_config_yields_defaults() {
        assert_eq!(parse_config("").unwrap(), Config::default());
        assert_eq!(parse_config("\n\n   \n").unwrap(), Config::default());
    }

    #[test]
    fn comments_are_ignored() {
        let cfg = parse_config("# a comment\n#another\n").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn quoted_and_bare_values_both_parse() {
        assert_eq!(
            parse_config("utc_offset = \"+08:00\"")
                .unwrap()
                .utc_offset_secs,
            8 * 3600
        );
        assert_eq!(
            parse_config("utc_offset = +08:00").unwrap().utc_offset_secs,
            8 * 3600
        );
    }

    #[test]
    fn whitespace_around_key_and_value_tolerated() {
        let cfg = parse_config("   utc_offset   =   \"+08:00\"   ").unwrap();
        assert_eq!(cfg.utc_offset_secs, 8 * 3600);
    }

    #[test]
    fn last_assignment_wins() {
        let cfg = parse_config("utc_offset = \"Z\"\nutc_offset = \"+08:00\"").unwrap();
        assert_eq!(cfg.utc_offset_secs, 8 * 3600);
    }

    #[test]
    fn unknown_key_is_an_error() {
        let err = parse_config("colour = \"red\"").unwrap_err();
        match err {
            TsdbError::ParseError { line, message } => {
                assert_eq!(line, 1);
                assert!(message.contains("unknown key"), "got {}", message);
            }
            other => panic!("expected ParseError, got {:?}", other),
        }
    }

    #[test]
    fn missing_equals_is_an_error() {
        let err = parse_config("utc_offset +08:00").unwrap_err();
        assert!(matches!(err, TsdbError::ParseError { line: 1, .. }));
    }

    #[test]
    fn empty_key_is_an_error() {
        let err = parse_config("= \"+08:00\"").unwrap_err();
        assert!(matches!(err, TsdbError::ParseError { line: 1, .. }));
    }

    #[test]
    fn unterminated_string_is_an_error() {
        let err = parse_config("utc_offset = \"+08:00").unwrap_err();
        match err {
            TsdbError::ParseError { line, message } => {
                assert_eq!(line, 1);
                assert!(message.contains("unterminated"), "got {}", message);
            }
            other => panic!("expected ParseError, got {:?}", other),
        }
    }

    #[test]
    fn invalid_offset_value_is_an_error() {
        let err = parse_config("utc_offset = \"lunchtime\"").unwrap_err();
        match err {
            TsdbError::ParseError { line, message } => {
                assert_eq!(line, 1);
                assert!(message.contains("invalid utc_offset"), "got {}", message);
            }
            other => panic!("expected ParseError, got {:?}", other),
        }
    }

    #[test]
    fn error_reports_the_offending_line_number() {
        let err = parse_config("# note\n\nutc_offset = \"Z\"\nbogus = 1\n").unwrap_err();
        assert!(matches!(err, TsdbError::ParseError { line: 4, .. }));
    }

    // -- load_from ----------------------------------------------------------

    #[test]
    fn load_from_missing_file_is_an_error() {
        let err = load_from(Path::new("/nonexistent/.tsdb.toml")).unwrap_err();
        match err {
            TsdbError::Other(m) => assert!(m.contains("cannot read config"), "got {}", m),
            other => panic!("expected Other, got {:?}", other),
        }
    }
}

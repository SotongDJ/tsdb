mod action;
mod base62;
mod dotsv;
mod error;
mod escape;
mod filter;
mod lock;
mod order;
mod plane;
mod query;
mod relate;
mod show;

use action::{collect_uuids, parse_action_file};
use dotsv::{apply_actions, atomic_write, maybe_compact, validate_actions, DotsvFile};
use error::{Result, TsdbError};
use lock::LockManager;
use std::path::Path;
use std::time::{Duration, Instant};

const REFRESH_INTERVAL_SECS: u64 = 5;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_usage(stream: UsageStream) {
    let lines = [
        "Usage:",
        "  tsdb <target.dov> <action.atv>             apply actions to database",
        "  tsdb <target.dov> --compact                compact pending section",
        "  tsdb --relate <target.dov>                 generate .kv.rtv, .vk.rtv, .uuid.rtv indexes",
        "  tsdb --plane <target.dov>                  generate .kv.ptv, .vk.ptv, .uuid.ptv, .ord.ptv indexes",
        "  tsdb --query <query.qtv> <target.dov>      query database, print matching UUIDs",
        "  tsdb --filter <filter.ftv> <target.dov>    rich predicate filter (eq/ne/lt/.. + numeric variants)",
        "  tsdb --query  ... <target.dov> --show [<out.dtv>|-]   emit full records (stdout default; '-' alias)",
        "  tsdb --filter ... <target.dov> --show [<out.dtv>|-]   emit full records (stdout default; '-' alias)",
        "  tsdb --help                                show this message",
        "  tsdb --version                             print version",
    ];
    match stream {
        UsageStream::Stdout => {
            for l in lines {
                println!("{}", l);
            }
        }
        UsageStream::Stderr => {
            for l in lines {
                eprintln!("{}", l);
            }
        }
    }
}

enum UsageStream {
    Stdout,
    Stderr,
}

fn usage() -> ! {
    print_usage(UsageStream::Stderr);
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Special short-arg forms first.
    let result = match args.len() {
        1 => {
            print_usage(UsageStream::Stdout);
            std::process::exit(0);
        }
        2 if args[1] == "--version" || args[1] == "-V" => {
            println!("tsdb {}", VERSION);
            std::process::exit(0);
        }
        2 if args[1] == "--help" || args[1] == "-h" => {
            print_usage(UsageStream::Stdout);
            std::process::exit(0);
        }
        3 if args[1] == "--relate" => run_relate_mode(Path::new(&args[2])),
        3 if args[1] == "--plane" => run_plane_mode(Path::new(&args[2])),
        3 if args[1] == "--compact" => run_compact_only(Path::new(&args[2])),
        3 => {
            let dov_path = Path::new(&args[1]);
            let second_arg = &args[2];
            if second_arg == "--compact" {
                run_compact_only(dov_path)
            } else {
                let action_path = Path::new(second_arg);
                run_with_actions(dov_path, action_path)
            }
        }
        n if n >= 4 && (args[1] == "--query" || args[1] == "--filter") => {
            // Form: tsdb --query <qtv> <dov> [--show [<out>|-]]
            //       tsdb --filter <ftv> <dov> [--show [<out>|-]]
            let is_query = args[1] == "--query";
            let crit_path = Path::new(&args[2]);
            let dov_path = Path::new(&args[3]);
            // Optional --show suffix.
            let show_target: Option<show::ShowTarget> = match args.len() {
                4 => None,
                5 if args[4] == "--show" => Some(show::ShowTarget::Stdout),
                6 if args[4] == "--show" => {
                    let path_arg = &args[5];
                    if path_arg == "-" {
                        Some(show::ShowTarget::Stdout)
                    } else if path_arg.starts_with('-') {
                        // Paths beginning with '-' (other than the lone '-'
                        // alias) are not supported (banana.md §1.1 + Pie's
                        // findings: documented behaviour).
                        eprintln!(
                            "Error: --show <out.dtv> path must not start with '-' (got {:?}); use '-' alone for stdout",
                            path_arg
                        );
                        std::process::exit(2);
                    } else {
                        Some(show::ShowTarget::File(std::path::PathBuf::from(path_arg)))
                    }
                }
                _ => usage(),
            };
            if is_query {
                run_query_show_mode(crit_path, dov_path, show_target)
            } else {
                run_filter_mode(crit_path, dov_path, show_target)
            }
        }
        _ => usage(),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run_compact_only(dov_path: &Path) -> Result<()> {
    // Acquire lock with empty UUID set before compacting to prevent concurrent writers
    let lock_mgr = LockManager::new(dov_path, Vec::new());
    lock_mgr.register()?;
    lock_mgr.wait_for_exec()?;

    let compact_result: Result<()> = (|| {
        let mut db = DotsvFile::load(dov_path)?;
        db.compact()?;
        atomic_write(&db, dov_path)?;
        eprintln!("Compacted: {}", dov_path.display());
        Ok(())
    })();

    let release_result = lock_mgr.release();
    compact_result?;
    release_result?;
    Ok(())
}

fn run_relate_mode(dov_path: &Path) -> Result<()> {
    if !dov_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("database file not found: {}", dov_path.display()),
        )));
    }
    // Acquire exclusive lock: compact + index generation must be atomic
    // with respect to concurrent writers.
    let lock_mgr = LockManager::new(dov_path, Vec::new());
    lock_mgr.register()?;
    lock_mgr.wait_for_exec()?;

    let result = run_relate_locked(dov_path);

    let release_result = lock_mgr.release();
    result?;
    release_result?;
    Ok(())
}

/// Inner relate: assumes the caller already holds the empty-UUID-set lock.
/// Used directly by composite modes (`--query`, `--filter`, `--show`) so a
/// single lock acquisition covers compact + auto-relate + auto-plane + read.
pub(crate) fn run_relate_locked(dov_path: &Path) -> Result<()> {
    let mut db = DotsvFile::load(dov_path)?;
    db.compact()?;
    atomic_write(&db, dov_path)?;
    // Re-load to get the exact state that was written (including timestamp).
    let db = DotsvFile::load(dov_path)?;
    relate::generate_rtvs(dov_path, &db)?;
    eprintln!("Related: {}", dov_path.display());
    Ok(())
}

fn run_plane_mode(dov_path: &Path) -> Result<()> {
    if !dov_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("database file not found: {}", dov_path.display()),
        )));
    }
    // Exclusive lock: compact + index generation must be atomic vs writers.
    let lock_mgr = LockManager::new(dov_path, Vec::new());
    lock_mgr.register()?;
    lock_mgr.wait_for_exec()?;

    let result = run_plane_locked(dov_path);

    let release_result = lock_mgr.release();
    result?;
    release_result?;
    Ok(())
}

/// Inner plane: assumes the caller already holds the empty-UUID-set lock.
pub(crate) fn run_plane_locked(dov_path: &Path) -> Result<()> {
    let mut db = DotsvFile::load(dov_path)?;
    db.compact()?;
    atomic_write(&db, dov_path)?;
    let db = DotsvFile::load(dov_path)?;
    plane::generate_ptvs(dov_path, &db)?;
    eprintln!("Planed: {}", dov_path.display());
    Ok(())
}

/// Legacy `--query` (no `--show`): UUIDs to stdout. Kept for
/// byte-identical regression behaviour when `--show` is absent.
fn run_query_show_mode(
    qtv_path: &Path,
    dov_path: &Path,
    show_target: Option<show::ShowTarget>,
) -> Result<()> {
    if !dov_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("database file not found: {}", dov_path.display()),
        )));
    }
    if !qtv_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("query file not found: {}", qtv_path.display()),
        )));
    }

    let lock_mgr = LockManager::new(dov_path, Vec::new());
    lock_mgr.register()?;
    lock_mgr.wait_for_exec()?;

    let result = (|| -> Result<()> {
        run_relate_locked(dov_path)?;
        match show_target {
            None => {
                // Legacy v0.5 path: UUIDs only.
                query::run_query(qtv_path, dov_path)
            }
            Some(target) => {
                // Resolve UUIDs in-process so we can pull full records.
                let uuids = query::resolve_query_uuids(qtv_path, dov_path)?;
                emit_show(uuids, dov_path, qtv_path, &target)
            }
        }
    })();

    let release_result = lock_mgr.release();
    result?;
    release_result?;
    Ok(())
}

/// `--filter` (with optional `--show`).
fn run_filter_mode(
    ftv_path: &Path,
    dov_path: &Path,
    show_target: Option<show::ShowTarget>,
) -> Result<()> {
    if !dov_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("database file not found: {}", dov_path.display()),
        )));
    }
    if !ftv_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("filter file not found: {}", ftv_path.display()),
        )));
    }

    let lock_mgr = LockManager::new(dov_path, Vec::new());
    lock_mgr.register()?;
    lock_mgr.wait_for_exec()?;

    let result = (|| -> Result<()> {
        run_relate_locked(dov_path)?;
        run_plane_locked(dov_path)?;
        let uuids = filter::run_filter(ftv_path, dov_path)?;
        match show_target {
            None => {
                for u in &uuids {
                    println!("{}", u);
                }
                Ok(())
            }
            Some(target) => emit_show(uuids, dov_path, ftv_path, &target),
        }
    })();

    let release_result = lock_mgr.release();
    result?;
    release_result?;
    Ok(())
}

/// Resolve a list of UUIDs to full records and emit per `target`.
/// Caller must already hold the lock (when `target` is `File`, the write
/// happens before the lock is released; stdout writes happen inside this
/// function but data is already in memory so that's also race-safe).
fn emit_show(
    uuids: Vec<String>,
    dov_path: &Path,
    criterion_path: &Path,
    target: &show::ShowTarget,
) -> Result<()> {
    let footer = relate::read_last_nonempty_line(dov_path)?;
    if let show::ShowTarget::File(out) = target {
        if show::dtv_skip_if_current(out, dov_path, criterion_path)? {
            eprintln!("skipped: {} already current", out.display());
            return Ok(());
        }
    }
    let db = DotsvFile::load(dov_path)?;
    let lines = show::collect_record_lines(&uuids, &db)?;
    match target {
        show::ShowTarget::Stdout => {
            show::emit_to_stdout(&lines, &footer);
        }
        show::ShowTarget::File(out) => {
            show::write_dtv_file(out, &lines, &footer)?;
        }
    }
    Ok(())
}

fn run_with_actions(dov_path: &Path, action_path: &Path) -> Result<()> {
    // Step 1: Parse and validate action file
    if !action_path.exists() {
        return Err(TsdbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("action file not found: {}", action_path.display()),
        )));
    }

    let actions = parse_action_file(action_path)?;

    if actions.is_empty() {
        // Nothing to do
        return Ok(());
    }

    // Collect UUIDs referenced
    let uuids = collect_uuids(&actions);

    // Step 2: Register in lock queue
    let lock_mgr = LockManager::new(dov_path, uuids);
    lock_mgr.register()?;

    // Step 3: Wait for our turn to execute
    lock_mgr.wait_for_exec()?;

    // Execute with periodic timestamp refresh
    let exec_result = execute_actions(dov_path, &actions, &lock_mgr);

    // Step 8: Release lock regardless of outcome
    let release_result = lock_mgr.release();

    // Return execution error first, then release error
    exec_result?;
    release_result?;
    Ok(())
}

fn execute_actions(
    dov_path: &Path,
    actions: &[action::Action],
    lock_mgr: &LockManager,
) -> Result<()> {
    // Step 5: Load the database
    let mut db = DotsvFile::load(dov_path)?;

    // Pre-validate all actions before making any changes
    validate_actions(&db, actions)?;

    // Capture time before applying so elapsed reflects actual apply duration
    let before_apply = Instant::now();
    let refresh_interval = Duration::from_secs(REFRESH_INTERVAL_SECS);

    // Apply all actions at once
    apply_actions(&mut db, actions)?;

    // Refresh EXEC timestamp if applying took long enough to risk eviction
    if before_apply.elapsed() >= refresh_interval {
        lock_mgr.refresh_timestamp()?;
    }

    // Compact if threshold exceeded
    maybe_compact(&mut db)?;

    // Step 7: Atomic write
    atomic_write(&db, dov_path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile_helper::TempDir;

    // Simple temp dir helper inline to avoid extra dep
    mod tempfile_helper {
        use std::path::{Path, PathBuf};

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn new() -> Self {
                let path =
                    std::env::temp_dir().join(format!("tsdb_test_{:016x}", rand::random::<u64>()));
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

    #[test]
    fn test_full_workflow_append_and_read() {
        let tmp = TempDir::new();
        let dov = tmp.path().join("test.dov");
        let action_file = tmp.path().join("actions.txt");

        fs::write(
            &action_file,
            "+AGk26cH00001\tname=Alice\tage=30\n+AGk26cH00002\tname=Bob\n",
        )
        .unwrap();

        run_with_actions(&dov, &action_file).unwrap();

        let db = DotsvFile::load(&dov).unwrap();
        assert!(db.uuid_exists("AGk26cH00001"));
        assert!(db.uuid_exists("AGk26cH00002"));
    }

    #[test]
    fn test_full_workflow_delete() {
        let tmp = TempDir::new();
        let dov = tmp.path().join("test.dov");
        let action1 = tmp.path().join("a1.txt");
        let action2 = tmp.path().join("a2.txt");

        fs::write(&action1, "+AGk26cH00001\tname=Alice\n").unwrap();
        run_with_actions(&dov, &action1).unwrap();

        fs::write(&action2, "-AGk26cH00001\n").unwrap();
        run_with_actions(&dov, &action2).unwrap();

        let db = DotsvFile::load(&dov).unwrap();
        assert!(!db.uuid_exists("AGk26cH00001"));
    }

    #[test]
    fn test_compact_only() {
        let tmp = TempDir::new();
        let dov = tmp.path().join("test.dov");
        let action = tmp.path().join("a.txt");

        fs::write(&action, "+AGk26cH00001\tname=Alice\n").unwrap();
        run_with_actions(&dov, &action).unwrap();

        run_compact_only(&dov).unwrap();

        let db = DotsvFile::load(&dov).unwrap();
        assert!(db.pending.is_empty());
        assert!(db.uuid_exists("AGk26cH00001"));
    }
}

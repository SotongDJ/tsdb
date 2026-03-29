mod action;
mod base62;
mod dotsv;
mod error;
mod escape;
mod lock;

use action::{collect_uuids, parse_action_file};
use dotsv::{apply_actions, atomic_write, maybe_compact, validate_actions, DotsvFile};
use error::{Result, TsdbError};
use lock::LockManager;
use std::path::Path;
use std::time::{Duration, Instant};

const REFRESH_INTERVAL_SECS: u64 = 5;

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  tsdb <target.dov> <action.txt>");
    eprintln!("  tsdb <target.dov> --compact");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        usage();
    }

    let dov_path = Path::new(&args[1]);
    let second_arg = &args[2];

    let result = if second_arg == "--compact" {
        run_compact_only(dov_path)
    } else {
        let action_path = Path::new(second_arg);
        run_with_actions(dov_path, action_path)
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run_compact_only(dov_path: &Path) -> Result<()> {
    let mut db = DotsvFile::load(dov_path)?;
    db.compact()?;
    atomic_write(&db, dov_path)?;
    eprintln!("Compacted: {}", dov_path.display());
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

    // Apply all actions
    let mut last_refresh = Instant::now();
    let refresh_interval = Duration::from_secs(REFRESH_INTERVAL_SECS);

    // Apply in batches, refreshing timestamp periodically
    for (i, action) in actions.iter().enumerate() {
        apply_actions(&mut db, std::slice::from_ref(action))?;

        // Refresh EXEC timestamp every ~5s
        if last_refresh.elapsed() >= refresh_interval {
            lock_mgr.refresh_timestamp()?;
            last_refresh = Instant::now();
        }

        // Maybe compact after each action (threshold check is cheap)
        if (i + 1) % 10 == 0 {
            maybe_compact(&mut db)?;
        }
    }

    // Final compact check
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
                let path = std::env::temp_dir().join(format!(
                    "tsdb_test_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
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

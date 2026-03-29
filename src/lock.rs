/// Lock file / queue management / concurrency.
///
/// Lock file format (`.dov.lock`):
///   EXEC\t<16-hex-pid>\t<uuid1,uuid2,...>\t<unix_secs>\n
///   WAIT\t<16-hex-pid>\t<uuid1,uuid2,...>\t<unix_secs>\n
///
/// Protocol:
///   1. Pre-scan action file → collect UUID set
///   2. flock .dov.lock exclusively (brief hold)
///   3. Check UUID overlap with existing EXEC/WAIT entries → error if conflict
///   4. Append WAIT entry; release lock
///   5. Poll until we are first WAIT with no EXEC → promote to EXEC; execute
///   6. After execution, remove our entry

use crate::error::{Result, TsdbError};
use fs2::FileExt;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STALE_TIMEOUT_SECS: u64 = 30;
const POLL_INTERVAL_MS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryStatus {
    Exec,
    Wait,
}

#[derive(Debug, Clone)]
pub struct LockEntry {
    pub status: EntryStatus,
    pub pid: u64,
    pub uuids: Vec<String>,
    pub timestamp: u64,
}

impl LockEntry {
    pub fn is_stale(&self) -> bool {
        let now = unix_secs();
        now.saturating_sub(self.timestamp) > STALE_TIMEOUT_SECS
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn current_pid() -> u64 {
    rand::random::<u64>()
}

/// Parse lock file contents into entries.
pub fn parse_lock_file(content: &str) -> Vec<LockEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            continue; // malformed, skip
        }
        let status = match parts[0] {
            "EXEC" => EntryStatus::Exec,
            "WAIT" => EntryStatus::Wait,
            _ => continue,
        };
        let pid = u64::from_str_radix(parts[1], 16).unwrap_or(0);
        let uuids: Vec<String> = if parts[2].is_empty() {
            Vec::new()
        } else {
            parts[2].split(',').map(|s| s.to_string()).collect()
        };
        let timestamp: u64 = parts[3].parse().unwrap_or(0);
        entries.push(LockEntry {
            status,
            pid,
            uuids,
            timestamp,
        });
    }
    entries
}

/// Serialize entries back to lock file content.
pub fn serialize_lock_file(entries: &[LockEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let status = match e.status {
            EntryStatus::Exec => "EXEC",
            EntryStatus::Wait => "WAIT",
        };
        out.push_str(&format!(
            "{}\t{:016x}\t{}\t{}\n",
            status,
            e.pid,
            e.uuids.join(","),
            e.timestamp
        ));
    }
    out
}

/// Lock manager for a single session.
pub struct LockManager {
    lock_path: PathBuf,
    pid: u64,
    uuids: Vec<String>,
}

impl LockManager {
    pub fn new(dov_path: &Path, uuids: Vec<String>) -> Self {
        let mut lock_os: std::ffi::OsString = dov_path.as_os_str().to_os_string();
        lock_os.push(".lock");
        let lock_path = PathBuf::from(lock_os);
        LockManager {
            lock_path,
            pid: current_pid(),
            uuids,
        }
    }

    /// Open or create the lock file.
    fn open_lock_file(&self) -> Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&self.lock_path)
            .map_err(TsdbError::Io)
    }

    /// Read lock file content while holding exclusive lock.
    fn read_entries(&self, file: &mut File) -> Result<Vec<LockEntry>> {
        file.seek(SeekFrom::Start(0))?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(parse_lock_file(&content))
    }

    /// Write entries to the lock file (truncate first).
    fn write_entries(&self, file: &mut File, entries: &[LockEntry]) -> Result<()> {
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        let content = serialize_lock_file(entries);
        file.write_all(content.as_bytes())?;
        file.flush()?;
        Ok(())
    }

    /// Register in the queue.
    /// Returns Err if UUID conflict detected with existing EXEC/WAIT entries.
    pub fn register(&self) -> Result<()> {
        let mut file = self.open_lock_file()?;
        file.lock_exclusive().map_err(TsdbError::Io)?;
        let result = self.do_register(&mut file);
        let _ = file.unlock();
        result
    }

    fn do_register(&self, file: &mut File) -> Result<()> {
        let now = unix_secs();
        let mut entries = self.read_entries(file)?;

        // Evict stale entries
        entries.retain(|e| !e.is_stale());

        // Check UUID overlap with existing entries
        let our_set: HashSet<&String> = self.uuids.iter().collect();
        for entry in &entries {
            let their_set: HashSet<&String> = entry.uuids.iter().collect();
            let overlap: Vec<String> = our_set
                .intersection(&their_set)
                .map(|s| (*s).clone())
                .collect();
            if !overlap.is_empty() {
                return Err(TsdbError::LockConflict {
                    pid: entry.pid,
                    uuids: overlap,
                });
            }
        }

        // Append WAIT entry
        entries.push(LockEntry {
            status: EntryStatus::Wait,
            pid: self.pid,
            uuids: self.uuids.clone(),
            timestamp: now,
        });

        self.write_entries(file, &entries)?;
        Ok(())
    }

    /// Poll until we can execute. Promotes our entry from WAIT to EXEC.
    pub fn wait_for_exec(&self) -> Result<()> {
        loop {
            let mut file = self.open_lock_file()?;
            file.lock_exclusive().map_err(TsdbError::Io)?;
            let result = self.try_promote(&mut file);
            let _ = file.unlock();

            match result? {
                true => return Ok(()), // promoted to EXEC
                false => {
                    // Wait and retry
                    std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                }
            }
        }
    }

    /// Try to promote ourselves from WAIT to EXEC.
    /// Returns Ok(true) if promoted, Ok(false) if we need to keep waiting.
    fn try_promote(&self, file: &mut File) -> Result<bool> {
        let now = unix_secs();
        let mut entries = self.read_entries(file)?;

        // Evict stale EXEC and stale WAIT entries
        entries.retain(|e| !e.is_stale());

        // Check if any non-stale EXEC entry has overlapping UUIDs with us
        let our_set: HashSet<&String> = self.uuids.iter().collect();
        let blocking_exec = entries.iter().any(|e| {
            e.status == EntryStatus::Exec
                && e.uuids.iter().any(|u| our_set.contains(u))
        });

        if blocking_exec {
            return Ok(false);
        }

        // Find the first WAIT entry
        let first_wait_pid = entries
            .iter()
            .find(|e| e.status == EntryStatus::Wait)
            .map(|e| e.pid);

        if first_wait_pid != Some(self.pid) {
            return Ok(false); // we're not first in queue
        }

        // Promote ourselves
        for entry in entries.iter_mut() {
            if entry.status == EntryStatus::Wait && entry.pid == self.pid {
                entry.status = EntryStatus::Exec;
                entry.timestamp = now;
            }
        }

        self.write_entries(file, &entries)?;
        Ok(true)
    }

    /// Refresh our EXEC entry timestamp to prevent eviction.
    pub fn refresh_timestamp(&self) -> Result<()> {
        let now = unix_secs();
        let mut file = self.open_lock_file()?;
        file.lock_exclusive().map_err(TsdbError::Io)?;
        let mut entries = self.read_entries(&mut file)?;
        for entry in entries.iter_mut() {
            if entry.status == EntryStatus::Exec && entry.pid == self.pid {
                entry.timestamp = now;
            }
        }
        let result = self.write_entries(&mut file, &entries);
        let _ = file.unlock();
        result
    }

    /// Remove our entry from the lock file after execution completes.
    pub fn release(&self) -> Result<()> {
        let mut file = self.open_lock_file()?;
        file.lock_exclusive().map_err(TsdbError::Io)?;
        let mut entries = self.read_entries(&mut file)?;
        entries.retain(|e| e.pid != self.pid);
        let result = self.write_entries(&mut file, &entries);
        let _ = file.unlock();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lock_file() {
        let content = "EXEC\t000000000000001a\tuuid1,uuid2\t1700000000\nWAIT\t000000000000002b\tuuid3\t1700000001\n";
        let entries = parse_lock_file(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, EntryStatus::Exec);
        assert_eq!(entries[0].pid, 26);
        assert_eq!(entries[0].uuids, vec!["uuid1", "uuid2"]);
        assert_eq!(entries[1].status, EntryStatus::Wait);
    }

    #[test]
    fn test_serialize_round_trip() {
        let entries = vec![
            LockEntry {
                status: EntryStatus::Exec,
                pid: 42,
                uuids: vec!["u1".to_string(), "u2".to_string()],
                timestamp: 12345,
            },
            LockEntry {
                status: EntryStatus::Wait,
                pid: 99,
                uuids: vec!["u3".to_string()],
                timestamp: 12346,
            },
        ];
        let serialized = serialize_lock_file(&entries);
        let parsed = parse_lock_file(&serialized);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 42);
        assert_eq!(parsed[1].pid, 99);
    }

    #[test]
    fn test_stale_detection() {
        let old_entry = LockEntry {
            status: EntryStatus::Exec,
            pid: 1,
            uuids: vec![],
            timestamp: 0, // very old
        };
        assert!(old_entry.is_stale());

        let fresh_entry = LockEntry {
            status: EntryStatus::Wait,
            pid: 2,
            uuids: vec![],
            timestamp: unix_secs(),
        };
        assert!(!fresh_entry.is_stale());
    }
}

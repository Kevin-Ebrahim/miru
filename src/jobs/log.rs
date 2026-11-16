use super::JobKind;
use anyhow::{Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const FINISHED_LOGS: usize = 32;

pub struct Log {
    path: PathBuf,
    lease: Option<File>,
}

impl Log {
    pub(super) fn new(state: &Path, kind: JobKind) -> Result<Self> {
        let directory = state.join("logs");
        fs::create_dir_all(&directory).context("Could not create diagnostic log directory")?;
        let prefix = match kind {
            JobKind::Playback => "player-",
            JobKind::Download => "download-",
        };
        let pending = tempfile::Builder::new()
            .prefix(prefix)
            .suffix(".pending")
            .tempfile_in(&directory)
            .context("Could not create job log")?;
        pending
            .as_file()
            .try_lock()
            .context("Could not lock job log")?;
        // Publish only after locking, so cleanup cannot mistake a new log for a finished job.
        let path = pending.path().with_extension("log");
        let file = pending
            .persist_noclobber(&path)
            .context("Could not publish job log")?;
        let log = Self {
            path,
            lease: Some(file),
        };
        let _ = prune(&directory);
        Ok(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn writer(&self) -> Result<File> {
        // Child output handles must not extend the job's ownership lock during teardown.
        OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
            .context("Could not capture job diagnostics")
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        drop(self.lease.take());
        if let Some(directory) = self.path.parent() {
            let _ = prune(directory);
        }
    }
}

fn prune(directory: &Path) -> io::Result<()> {
    let mut logs: Vec<_> = fs::read_dir(directory)?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.ends_with(".log")
                || !(name.starts_with("player-") || name.starts_with("download-"))
                || !entry.file_type().ok()?.is_file()
            {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, entry.path()))
        })
        .collect();
    logs.sort_unstable_by(|a, b| b.cmp(a));
    let mut finished = 0;
    for (_, path) in logs {
        let Ok(file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        else {
            continue;
        };
        if !file.metadata()?.is_file() || file.try_lock_shared().is_err() {
            continue;
        }
        // Shared cleaner locks do not make completed logs look active to other cleaners.
        finished += 1;
        if finished > FINISHED_LOGS {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::FileTimes,
        io::Write,
        os::unix::{fs::symlink, net::UnixListener},
        sync::{Arc, Barrier},
        time::{Duration, UNIX_EPOCH},
    };

    fn paths(state: &Path) -> Vec<PathBuf> {
        fs::read_dir(state.join("logs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
            .collect()
    }

    fn completed(state: &Path, number: usize) -> PathBuf {
        let kind = if number.is_multiple_of(2) {
            JobKind::Playback
        } else {
            JobKind::Download
        };
        let log = Log::new(state, kind).unwrap();
        let mut writer = log.writer().unwrap();
        writeln!(writer, "Completed {number}").unwrap();
        writer
            .set_times(
                FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(number as u64 + 2)),
            )
            .unwrap();
        drop(writer);
        let path = log.path().to_owned();
        drop(log);
        path
    }

    #[test]
    fn retention_keeps_recent_logs_and_protects_older_active_writers() {
        let state = tempfile::tempdir().unwrap();
        let active = Log::new(state.path(), JobKind::Playback).unwrap();
        writeln!(active.writer().unwrap(), "Active start").unwrap();
        active
            .writer()
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        let finished: Vec<_> = (0..FINISHED_LOGS + 4)
            .map(|number| completed(state.path(), number))
            .collect();
        assert!(finished[..4].iter().all(|path| !path.exists()));
        assert!(finished[4..].iter().all(|path| path.exists()));
        assert_eq!(paths(state.path()).len(), FINISHED_LOGS + 1);
        assert_eq!(fs::read_to_string(active.path()).unwrap(), "Active start\n");
        writeln!(active.writer().unwrap(), "Active end").unwrap();
        let path = active.path().to_owned();
        drop(active);
        assert_eq!(paths(state.path()).len(), FINISHED_LOGS);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "Active start\nActive end\n"
        );
    }

    #[test]
    fn diagnostic_handles_do_not_extend_a_completed_jobs_active_lease() {
        let state = tempfile::tempdir().unwrap();
        let log = Log::new(state.path(), JobKind::Download).unwrap();
        let mut child_writer = log.writer().unwrap();
        let path = log.path().to_owned();
        child_writer
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        drop(log);
        for number in 0..FINISHED_LOGS + 1 {
            completed(state.path(), number);
        }
        assert!(
            !path.exists(),
            "A diagnostic handle kept a completed job active"
        );
        assert_eq!(paths(state.path()).len(), FINISHED_LOGS);
        writeln!(child_writer, "Final output during teardown").unwrap();
    }

    #[test]
    fn concurrent_creation_and_cleanup_keep_retention_bounded() {
        let state = tempfile::tempdir().unwrap();
        let active = Log::new(state.path(), JobKind::Download).unwrap();
        active
            .writer()
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        let barrier = Arc::new(Barrier::new(4));
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let barrier = barrier.clone();
                let state = state.path();
                scope.spawn(move || {
                    barrier.wait();
                    for number in 0..20 {
                        completed(state, worker * 20 + number);
                    }
                });
            }
        });
        assert!(active.path().exists());
        assert_eq!(paths(state.path()).len(), FINISHED_LOGS + 1);
        drop(active);
        assert_eq!(paths(state.path()).len(), FINISHED_LOGS);
    }

    #[test]
    fn cleanup_preserves_unrelated_files_and_non_regular_entries() {
        let state = tempfile::tempdir().unwrap();
        let directory = state.path().join("logs");
        fs::create_dir(&directory).unwrap();
        let outside = state.path().join("player.log");
        fs::write(&outside, "Legacy diagnostic").unwrap();
        let unrelated = directory.join("notes.txt");
        fs::write(&unrelated, "Keep this").unwrap();
        let link = directory.join("player-link.log");
        symlink(&outside, &link).unwrap();
        let folder = directory.join("download-folder.log");
        fs::create_dir(&folder).unwrap();
        let socket = directory.join("player-socket.log");
        let _listener = UnixListener::bind(&socket).unwrap();
        for number in 0..FINISHED_LOGS + 1 {
            completed(state.path(), number);
        }
        assert_eq!(fs::read_to_string(outside).unwrap(), "Legacy diagnostic");
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "Keep this");
        assert!(link.is_symlink() && folder.is_dir() && socket.exists());
    }
}

use std::{
    fs::File,
    io,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseMode {
    Shared,
    Exclusive,
}

/// Advisory descriptor lease. Leases are never represented by a PID file and
/// the kernel releases them after a crashed process. Every participating tool
/// holds shared parent-directory leases while creating state, binding inputs,
/// or publishing files; destructive owners take an exclusive directory lease.
pub struct FileLease {
    file: File,
}

impl FileLease {
    pub fn acquire(file: File, mode: LeaseMode, timeout: Duration) -> io::Result<Self> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid lease timeout"))?;
        let operation = match mode {
            LeaseMode::Shared => libc::LOCK_SH,
            LeaseMode::Exclusive => libc::LOCK_EX,
        } | libc::LOCK_NB;
        loop {
            // SAFETY: the descriptor remains owned throughout the lease.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
                return Ok(Self { file });
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() != io::ErrorKind::WouldBlock || Instant::now() >= deadline {
                return Err(error);
            }
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    pub fn file(&self) -> &File {
        &self.file
    }
}

impl Drop for FileLease {
    fn drop(&mut self) {
        // SAFETY: unlocking the descriptor owned by this lease.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

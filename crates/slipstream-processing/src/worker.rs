use slipstream_processing::protocol::{Outcome, Workload};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    thread,
    time::{Duration, Instant},
};

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 3
        || arguments[1].len() != 32
        || !arguments[1]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        std::process::exit(70);
    }
    let deadline = arguments[2]
        .parse::<u64>()
        .unwrap_or_else(|_| std::process::exit(70));
    if deadline_timer(deadline).is_err() {
        std::process::exit(70);
    }
    let workload: Workload =
        match serde_json::from_value(serde_json::Value::String(arguments[0].clone())) {
            Ok(workload) => workload,
            Err(_) => std::process::exit(70),
        };
    if gate(&arguments[1]).is_err() {
        std::process::exit(70);
    }
    if storage_check().is_err() {
        std::process::exit(71);
    }
    let mut result = match OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open("/work/result")
    {
        Ok(result) => result,
        Err(_) => std::process::exit(71),
    };
    if result.write_all(&[0; 4096]).is_err() {
        std::process::exit(71);
    }
    let outcome = match workload {
        Workload::Film(_) => std::process::exit(70),
        Workload::ProbeSuccess => {
            for path in [
                "/sys/fs/cgroup/memory.max",
                "/sys/fs/cgroup/cgroup.procs",
                "/proc/1/root/sys/fs/cgroup/cgroup.procs",
            ] {
                if OpenOptions::new().write(true).open(path).is_ok() {
                    std::process::exit(72);
                }
            }
            if std::path::Path::new("/var/run/docker.sock").exists() {
                std::process::exit(72);
            }
            let block = [b'x'; 4096];
            for _ in 0..1024 {
                if std::io::stdout().write_all(&block).is_err()
                    || std::io::stderr().write_all(&block).is_err()
                {
                    std::process::exit(72);
                }
            }
            Outcome::Completed
        }
        Workload::ProbeNativeOom => allocate(),
        Workload::ProbeDescendantOom => {
            for _ in 0..2 {
                // SAFETY: this worker is single-threaded before fork; children use only the bounded fixture.
                match unsafe { libc::fork() } {
                    -1 => std::process::exit(73),
                    0 => {
                        let _ = allocate();
                        std::process::exit(20);
                    }
                    _ => {}
                }
            }
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        Workload::ProbeExit137 => std::process::exit(137),
        Workload::ProbeHold => loop {
            thread::sleep(Duration::from_secs(1));
        },
        Workload::ProbeStorageFull => {
            let mut file = File::create("/work/payload").unwrap_or_else(|_| std::process::exit(71));
            let block = [42u8; 65536];
            loop {
                if let Err(error) = file.write_all(&block) {
                    if error.raw_os_error() != Some(libc::ENOSPC) {
                        std::process::exit(74);
                    }
                    break;
                }
            }
            Outcome::StorageFull
        }
        Workload::ProbeInodesFull => {
            for index in 0..128 {
                if let Err(error) = File::create(format!("/work/inode-{index}")) {
                    if error.raw_os_error() != Some(libc::ENOSPC) {
                        std::process::exit(74);
                    }
                    return finish(&mut result, &arguments[1], Outcome::StorageFull);
                }
            }
            std::process::exit(74);
        }
    };
    finish(&mut result, &arguments[1], outcome);
}

fn finish(file: &mut File, launch_id: &str, outcome: Outcome) {
    let bytes = serde_json::to_vec(&serde_json::json!({"launch_id":launch_id,"outcome":outcome}))
        .expect("fixed result");
    let mut block = [0u8; 4096];
    block[..bytes.len()].copy_from_slice(&bytes);
    if file
        .seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&block))
        .and_then(|_| file.sync_all())
        .is_err()
    {
        std::process::exit(71);
    }
    std::process::exit(match outcome {
        Outcome::Completed => 0,
        Outcome::AllocationFailed => 20,
        Outcome::StorageFull => 21,
        _ => 75,
    });
}

fn gate(token: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/control/gate")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::with_capacity(33);
    while Instant::now() < deadline {
        let mut descriptor = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // SAFETY: descriptor points to one live pollfd for a valid owned descriptor.
        if unsafe { libc::poll(&mut descriptor, 1, 100) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if descriptor.revents == 0 {
            continue;
        }
        let mut part = [0u8; 34];
        let length = file.read(&mut part)?;
        if length == 0 {
            break;
        }
        bytes.extend_from_slice(&part[..length]);
        if bytes.len() >= 33 {
            if bytes == format!("{token}\n").as_bytes() {
                return Ok(());
            }
            break;
        }
    }
    Err(std::io::Error::other("bootstrap not released"))
}

fn allocate() -> Outcome {
    loop {
        const BYTES: usize = 2 * 1024 * 1024;
        // SAFETY: anonymous mmap creates a private writable region; only in-bounds pages are touched.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                BYTES,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if memory == libc::MAP_FAILED {
            return Outcome::AllocationFailed;
        }
        for index in (0..BYTES).step_by(4096) {
            // SAFETY: index lies within the successfully mapped region.
            unsafe {
                memory.cast::<u8>().add(index).write_volatile(42);
            }
        }
        // The fixture deliberately retains each mapping until contained by the kernel.
        thread::sleep(Duration::from_millis(5));
    }
}

fn storage_check() -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let root = std::fs::metadata("/work")?;
    for path in ["/work", "/tmp", "/dev/shm"] {
        let metadata = std::fs::metadata(path)?;
        if metadata.dev() != root.dev() || metadata.ino() != root.ino() || metadata.uid() != 1000 {
            return Err(std::io::Error::other("storage identity mismatch"));
        }
        let path = std::ffi::CString::new(path).expect("fixed path");
        // SAFETY: statfs initializes the supplied struct for a fixed existing path.
        let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(path.as_ptr(), &mut stats) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if stats.f_type != libc::TMPFS_MAGIC
            || stats.f_blocks * stats.f_bsize as u64 != 16 * 1024 * 1024
            || stats.f_files != 64
        {
            return Err(std::io::Error::other("unbounded temporary storage"));
        }
    }
    Ok(())
}

fn deadline_timer(deadline_ms: u64) -> std::io::Result<()> {
    extern "C" fn expired(_: libc::c_int) {
        // SAFETY: _exit is async-signal-safe. PID namespace teardown settles all descendants.
        unsafe {
            libc::_exit(76);
        }
    }
    // SAFETY: these C POD structures are initialized before their respective syscalls.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = expired as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGALRM, &action, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut event: libc::sigevent = std::mem::zeroed();
        event.sigev_notify = libc::SIGEV_SIGNAL;
        event.sigev_signo = libc::SIGALRM;
        let mut timer: libc::timer_t = std::mem::zeroed();
        if libc::timer_create(libc::CLOCK_REALTIME, &mut event, &mut timer) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let setting = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: (deadline_ms / 1000)
                    .try_into()
                    .map_err(|_| std::io::Error::other("deadline overflow"))?,
                tv_nsec: ((deadline_ms % 1000) * 1_000_000) as i64,
            },
        };
        if libc::timer_settime(timer, libc::TIMER_ABSTIME, &setting, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

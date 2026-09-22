//! Fixed Film PID 1. Only the native parent receives staging and result writers.
use sha2::{Digest, Sha256};
use slipstream_processing::{
    film::{
        self, Detail, EngineGrant, Phase, ProducerResult, Stage, WorkerFailure, WorkerResult,
        WorkerSuccess,
    },
    protocol::{Outcome, digest, now},
    staging,
};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::{FileExt, MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
fn bad(message: &str) -> io::Error {
    io::Error::other(message)
}
#[derive(Debug)]
struct LostController;
impl std::fmt::Display for LostController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("authenticated stage controller disconnected")
    }
}
impl std::error::Error for LostController {}
fn control<T>(result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| {
        if matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionRefused
                | io::ErrorKind::NotConnected
        ) {
            io::Error::other(LostController)
        } else {
            error
        }
    })
}
fn lost_controller(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.is::<LostController>())
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    // SAFETY: getpid reads the current namespace identity. This worker must never run as a host process.
    if unsafe { libc::getpid() } != 1 {
        std::process::exit(70);
    }
    if args.len() != 3
        || args[0] != "film-fixture"
        || args[1].len() != 32
        || !args[1]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        std::process::exit(70);
    }
    let deadline = args[2]
        .parse::<u64>()
        .unwrap_or_else(|_| std::process::exit(70));
    if deadline_timer(deadline).is_err() {
        std::process::exit(70);
    }
    // SAFETY: PR_SET_DUMPABLE removes same-UID process-interface access before any capability receipt.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        std::process::exit(70);
    }
    if let Err(error) = run(&args[1], deadline) {
        if lost_controller(&error) {
            // run has unwound every capability and stopped any child. Remain
            // unreleased for recovery; the original absolute timer still exits
            // 76 if no owner returns. Never reconnect or infer another permit.
            loop {
                thread::park();
            }
        }
        std::process::exit(75);
    }
}
fn run(launch: &str, deadline: u64) -> io::Result<()> {
    let started = Instant::now();
    let channel = control(staging::connect(Path::new("/control/stage.sock")))?;
    let initial = Instant::now() + Duration::from_secs(10);
    let (offer, rights): (film::StageOffer, Vec<OwnedFd>) = loop {
        if Instant::now() >= initial {
            return Err(bad("initial placement deadline"));
        }
        if let Some(packet) = control(staging::receive(channel.as_raw_fd()))? {
            break packet;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if offer.version != 2
        || offer.kind != "stage-offer"
        || offer.launch_id != launch
        || !["development-tiff", "synthetic-rgb"].contains(&offer.source.as_str())
    {
        return Err(bad("invalid offer"));
    }
    let expected_rights = if offer.source == "development-tiff" {
        3
    } else {
        1
    };
    if rights.len() != expected_rights {
        return Err(bad("wrong capabilities"));
    }
    let mut rights: Vec<File> = rights.into_iter().map(File::from).collect();
    let mut result = rights.pop().ok_or_else(|| bad("missing native result"))?;
    let meta = result.metadata()?;
    if staging::access(result.as_raw_fd())? != libc::O_WRONLY
        || !meta.is_file()
        || meta.uid() != 0
        || meta.nlink() != 1
        || meta.mode() & 0o777 != 0o444
        || meta.len() != film::FRAME as u64
        || meta.dev() != offer.result_device
        || meta.ino() != offer.result_inode
    {
        return Err(bad("wrong native result inode"));
    }
    storage()?;
    let input = File::open("/input")?;
    let grantfile = staging::safe_open(&input, "grant.json", libc::O_RDONLY)?;
    let meta = grantfile.metadata()?;
    if !meta.is_file()
        || meta.uid() != 0
        || meta.nlink() != 1
        || meta.mode() & 0o777 != 0o444
        || meta.len() > film::FRAME as u64
    {
        return Err(bad("grant metadata"));
    }
    let mut bytes = Vec::new();
    grantfile
        .take(film::FRAME as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > film::FRAME || digest(&bytes) != offer.grant_sha256 {
        return Err(bad("grant digest"));
    }
    let grant: EngineGrant = film::parse(&bytes, film::FRAME).map_err(|_| bad("grant syntax"))?;
    grant.validate().map_err(|_| bad("grant plan"))?;
    if grant.launch_id != launch
        || film::canonical(&grant).map_err(|_| bad("canonical grant"))? != bytes
    {
        return Err(bad("grant identity"));
    }
    let stage_started = Instant::now();
    let mut staging_reclaim_us = 0;
    let stage = (|| -> io::Result<()> {
        match &grant.fixture.source {
            film::Source::DevelopmentTiff { bytes, sha256 } => {
                if offer.source != "development-tiff"
                    || offer.source_bytes != *bytes
                    || offer.source_sha256.as_ref() != Some(sha256)
                {
                    return Err(bad("source manifest"));
                }
                let mut writer = rights.pop().ok_or_else(|| bad("snapshot capability"))?;
                let mut source = rights.pop().ok_or_else(|| bad("source capability"))?;
                let before = source.metadata()?;
                let snapshot = writer.metadata()?;
                if staging::access(source.as_raw_fd())? != libc::O_RDONLY
                    || staging::access(writer.as_raw_fd())? != libc::O_WRONLY
                    || !before.is_file()
                    || before.uid() != 0
                    || before.nlink() != 1
                    || before.mode() & 0o022 != 0
                    || before.len() != *bytes
                    || !snapshot.is_file()
                    || snapshot.uid() != 0
                    || snapshot.nlink() != 1
                    || snapshot.mode() & 0o777 != 0o444
                    || snapshot.len() != 0
                    || Some(snapshot.dev()) != offer.destination_device
                    || Some(snapshot.ino()) != offer.destination_inode
                {
                    return Err(bad("staging rights"));
                }
                let mut hasher = Sha256::new();
                let mut buffer = [0u8; 65536];
                let mut copied = 0u64;
                loop {
                    let count = source.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    copied = copied
                        .checked_add(count as u64)
                        .ok_or_else(|| bad("source length"))?;
                    if copied > *bytes {
                        return Err(bad("source grew"));
                    }
                    hasher.update(&buffer[..count]);
                    writer.write_all(&buffer[..count])?;
                }
                writer.sync_all()?;
                let after = source.metadata()?;
                if copied != *bytes
                    || format!("{:x}", hasher.finalize()) != *sha256
                    || stamp(&before) != stamp(&after)
                {
                    return Err(bad("source changed"));
                }
                let reclaim = Instant::now();
                drop(source);
                drop(writer);
                staging_reclaim_us = micros(reclaim);
            }
            film::Source::SyntheticRgb { .. } => {
                if offer.source != "synthetic-rgb"
                    || offer.source_bytes != 0
                    || offer.source_sha256.is_some()
                    || offer.destination_device.is_some()
                    || offer.destination_inode.is_some()
                {
                    return Err(bad("synthetic rights"));
                }
            }
        }
        if !rights.is_empty() {
            return Err(bad("extra capabilities"));
        }
        Ok(())
    })();
    if let Err(error) = stage {
        drop(rights);
        let outcome = if error.raw_os_error() == Some(libc::ENOSPC) {
            Outcome::StorageFull
        } else {
            Outcome::EngineFailed
        };
        return finish(
            &mut result,
            &grant,
            WorkerResult::Failure(failure(
                &grant,
                outcome,
                Some(Detail::SourceMismatch),
                Phase::Staging,
                started,
            )),
        );
    }
    let staging_us = micros(stage_started).saturating_sub(staging_reclaim_us);
    control(staging::send(channel.as_raw_fd(), &offer.ack(), &[]))?;
    let (permit, rights): (film::Permit, Vec<OwnedFd>) = loop {
        if now().map_err(|_| bad("clock"))? >= deadline {
            return Err(bad("execution deadline"));
        }
        if let Some(packet) = control(staging::receive(channel.as_raw_fd()))? {
            break packet;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !rights.is_empty()
        || permit
            != (film::Permit {
                version: 2,
                kind: "engine-permit".into(),
                launch_id: launch.into(),
                grant_sha256: offer.grant_sha256.clone(),
            })
    {
        return Err(bad("engine permit"));
    }
    let producer = engine(channel, &offer.grant_sha256, &permit, deadline);
    let validation = Instant::now();
    let worker = match producer {
        Err(error) if lost_controller(&error) => return Err(error),
        Ok((code, signal, Some(ProducerResult::Success(producer))))
            if code == Some(0) && signal.is_none() =>
        {
            match validate_output(&grant, &producer) {
                Ok((artifact, reclaim_us)) => {
                    let mut stages = producer.stages;
                    stages.insert(
                        0,
                        film::Timing {
                            stage: Stage::Staging,
                            elapsed_us: staging_us,
                            reclaim_us: staging_reclaim_us,
                        },
                    );
                    stages.push(film::Timing {
                        stage: Stage::Validation,
                        elapsed_us: micros(validation).saturating_sub(reclaim_us),
                        reclaim_us,
                    });
                    WorkerResult::Success(WorkerSuccess {
                        version: 2,
                        kind: "film-measurement-result".into(),
                        outcome: Outcome::Completed,
                        launch_id: launch.into(),
                        manifest: grant.manifest.clone(),
                        plan_sha256: film::hash(&grant.plan).map_err(|_| bad("plan hash"))?,
                        artifact,
                        execution_us: micros(started),
                        stages,
                    })
                }
                Err(_) => WorkerResult::Failure(failure(
                    &grant,
                    Outcome::EngineFailed,
                    Some(Detail::ArtifactInvalid),
                    Phase::Validating,
                    started,
                )),
            }
        }
        Ok((code, _, Some(ProducerResult::Failure(producer))))
            if valid_failure(&grant, &producer, code) =>
        {
            WorkerResult::Failure(failure(
                &grant,
                producer.outcome,
                producer.detail,
                Phase::Engine,
                started,
            ))
        }
        Ok((_, Some(libc::SIGXFSZ), _)) => WorkerResult::Failure(failure(
            &grant,
            Outcome::StorageFull,
            Some(Detail::OutputLimit),
            Phase::Engine,
            started,
        )),
        _ => WorkerResult::Failure(failure(
            &grant,
            Outcome::EngineFailed,
            Some(Detail::ArtifactInvalid),
            Phase::Engine,
            started,
        )),
    };
    finish(&mut result, &grant, worker)
}
fn stamp(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn micros(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}
fn failure(
    grant: &EngineGrant,
    outcome: Outcome,
    detail: Option<Detail>,
    phase: Phase,
    start: Instant,
) -> WorkerFailure {
    WorkerFailure {
        version: 2,
        kind: "film-measurement-result".into(),
        outcome,
        detail,
        phase,
        launch_id: grant.launch_id.clone(),
        manifest: grant.manifest.clone(),
        plan_sha256: film::hash(&grant.plan).unwrap_or_default(),
        execution_us: micros(start),
    }
}
fn finish(file: &mut File, grant: &EngineGrant, result: WorkerResult) -> io::Result<()> {
    result
        .validate(grant)
        .map_err(|_| bad("native result validation"))?;
    let bytes = serde_json::to_vec(&result)?;
    if bytes.len() > film::FRAME - 4 {
        return Err(bad("native result limit"));
    }
    let mut payload = [0u8; film::FRAME - 4];
    payload[..bytes.len()].copy_from_slice(&bytes);
    file.write_all_at(&payload, 4)?;
    file.sync_all()?;
    file.write_all_at(&(bytes.len() as u32).to_be_bytes(), 0)?;
    file.sync_all()?;
    std::process::exit(match result.outcome() {
        Outcome::Completed => 0,
        Outcome::AllocationFailed => 20,
        Outcome::StorageFull => 21,
        Outcome::Deadline => 76,
        _ => 75,
    });
}
fn valid_failure(grant: &EngineGrant, value: &film::ProducerFailure, code: Option<i32>) -> bool {
    let expected = match value.outcome {
        Outcome::AllocationFailed => 20,
        Outcome::StorageFull => 21,
        Outcome::EngineFailed => 75,
        _ => return false,
    };
    value.version == 2
        && value.kind == "film-producer-result"
        && value.launch_id == grant.launch_id
        && value.manifest == grant.manifest
        && film::hash(&grant.plan).is_ok_and(|h| h == value.plan_sha256)
        && code == Some(expected)
}
fn validate_output(
    grant: &EngineGrant,
    producer: &film::ProducerSuccess,
) -> io::Result<(film::Artifact, u64)> {
    if producer.version != 2
        || producer.kind != "film-producer-result"
        || producer.outcome != "produced"
        || producer.launch_id != grant.launch_id
        || producer.manifest != grant.manifest
        || producer.plan_sha256 != film::hash(&grant.plan).map_err(|_| bad("plan"))?
    {
        return Err(bad("producer identity"));
    }
    film::timings(&producer.stages, true).map_err(|_| bad("stage timing"))?;
    let p = &producer.pixels;
    let r = &grant.fixture.reference;
    if p.width != grant.fixture.width
        || p.height != grant.fixture.height
        || p.input_pixels_sha256 != r.input_pixels_sha256
        || p.film_pixels_sha256 != r.film_pixels_sha256
        || p.icc_sha256 != grant.output_icc_sha256
    {
        return Err(bad("pixel observations"));
    }
    let entries = fs::read_dir("/output")?.collect::<io::Result<Vec<_>>>()?;
    if entries.len() != 1 || entries[0].file_name() != "finished.jpg" {
        return Err(bad("extra output"));
    }
    let output = File::open("/output")?;
    let mut file = staging::safe_open(&output, "finished.jpg", libc::O_RDONLY)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != 1000
        || meta.nlink() != 1
        || meta.len() != r.jpeg_bytes
        || meta.len() > 512 * 1024 * 1024
    {
        return Err(bad("artifact metadata"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let jpeg_sha256 = format!("{:x}", hash.finalize());
    if jpeg_sha256 != r.jpeg_sha256 {
        return Err(bad("artifact digest"));
    }
    let reclaim = Instant::now();
    drop(file);
    drop(output);
    let reclaim_us = micros(reclaim);
    Ok((
        film::Artifact {
            input_pixels_sha256: p.input_pixels_sha256.clone(),
            film_pixels_sha256: p.film_pixels_sha256.clone(),
            jpeg_sha256,
            jpeg_bytes: meta.len(),
            width: p.width,
            height: p.height,
            icc_sha256: p.icc_sha256.clone(),
            reference_evidence_sha256: r.evidence_sha256.clone(),
        },
        reclaim_us,
    ))
}
struct Children {
    active: bool,
}
impl Drop for Children {
    fn drop(&mut self) {
        // This fixed binary only runs as PID1. Namespace-scoped kill never names a host PID.
        // SAFETY: getpid has no preconditions. PID1 is excluded from kill(-1).
        if self.active && unsafe { libc::getpid() } == 1 {
            // SAFETY: all visible descendants belong to this private unprivileged PID namespace.
            unsafe {
                libc::kill(-1, libc::SIGKILL);
            }
            loop {
                let mut status = 0;
                // SAFETY: waitpid(-1) reaps only our children, after their namespace-wide termination.
                let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
                if pid < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
        }
    }
}
fn engine(
    channel: OwnedFd,
    grant_hash: &str,
    permit: &film::Permit,
    deadline: u64,
) -> io::Result<(Option<i32>, Option<i32>, Option<ProducerResult>)> {
    for name in ["home", "cache", "config", "numba", "matplotlib", "tmp"] {
        fs::create_dir(format!("/work/{name}"))?;
        fs::set_permissions(format!("/work/{name}"), fs::Permissions::from_mode(0o700))?;
    }
    let mut pipe = [0; 2];
    // SAFETY: pipe2 initializes two fresh CLOEXEC descriptors; parent changes only the read end to nonblocking.
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful pipe2 returned fresh descriptors, each wrapped exactly once.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    // SAFETY: F_SETFL updates this owned read endpoint without affecting the separate writer endpoint.
    if unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let writer = write.as_raw_fd();
    let mut command = Command::new("/opt/runtime/bin/python");
    command
        .arg("/opt/film-measurement/adapter.py")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in [
        ("PATH", "/opt/runtime/bin:/usr/bin:/bin"),
        ("LANG", "C.UTF-8"),
        ("PYTHONPATH", "/opt/spektrafilm/src"),
        ("MPLBACKEND", "Agg"),
        ("HOME", "/work/home"),
        ("XDG_CACHE_HOME", "/work/cache"),
        ("XDG_CONFIG_HOME", "/work/config"),
        ("NUMBA_CACHE_DIR", "/work/numba"),
        ("MPLCONFIGDIR", "/work/matplotlib"),
        ("TMPDIR", "/work/tmp"),
        ("PYTHONNOUSERSITE", "1"),
        ("PYTHONDONTWRITEBYTECODE", "1"),
        ("NUMBA_NUM_THREADS", "1"),
        ("OMP_NUM_THREADS", "4"),
        ("OPENBLAS_NUM_THREADS", "4"),
        ("MKL_NUM_THREADS", "4"),
        ("NUMEXPR_NUM_THREADS", "4"),
        ("VECLIB_MAXIMUM_THREADS", "4"),
        ("SLIPSTREAM_FILM_GRANT_SHA256", grant_hash),
    ] {
        command.env(key, value);
    }
    // SAFETY: the fork child performs only async-signal-safe libc operations before exec. All other capability descriptors remain CLOEXEC.
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: 512 * 1024 * 1024,
                rlim_max: 512 * 1024 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0
                || libc::dup2(writer, 3) < 0
                || libc::fcntl(3, libc::F_SETFD, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // std::process uses a CLOEXEC error pipe: success proves the fixed child exec completed.
    let child = command.spawn()?;
    let mut children = Children { active: true };
    let child_id = child.id();
    drop(write);
    control(staging::send(
        channel.as_raw_fd(),
        &film::Permit {
            kind: "engine-started".into(),
            ..permit.clone()
        },
        &[],
    ))?;
    drop(channel);
    let mut bytes = Vec::with_capacity(film::FRAME);
    let mut eof = false;
    let mut status = None;
    let mut no_children = false;
    while !eof || !no_children {
        if now().map_err(|_| bad("clock"))? >= deadline {
            return Err(bad("deadline"));
        }
        if !eof {
            let mut buffer = [0u8; 4096];
            // SAFETY: read endpoint and stack buffer are valid for this nonblocking read.
            let count =
                unsafe { libc::read(read.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if count == 0 {
                eof = true;
            } else if count > 0 {
                if bytes.len() + count as usize > film::FRAME {
                    return Err(bad("producer overflow"));
                }
                bytes.extend_from_slice(&buffer[..count as usize]);
            } else {
                let e = io::Error::last_os_error();
                if ![io::ErrorKind::WouldBlock, io::ErrorKind::Interrupted].contains(&e.kind()) {
                    return Err(e);
                }
            }
        }
        loop {
            let mut observed = 0;
            // SAFETY: PID1 reaps only its own children; WNOHANG cannot target an unrelated process.
            let pid = unsafe { libc::waitpid(-1, &mut observed, libc::WNOHANG) };
            if pid > 0 {
                if pid as u32 == child_id {
                    status = Some(observed);
                }
                continue;
            }
            if pid < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                no_children = true;
            }
            break;
        }
        if !eof || !no_children {
            thread::sleep(Duration::from_millis(5));
        }
    }
    children.active = false;
    let status = status.ok_or_else(|| bad("child status"))?;
    let code = libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status));
    let signal = libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status));
    Ok((code, signal, decode_producer(&bytes)?))
}
fn decode_producer(bytes: &[u8]) -> io::Result<Option<ProducerResult>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() < 4 {
        return Err(bad("producer length"));
    }
    let size =
        u32::from_be_bytes(bytes[..4].try_into().map_err(|_| bad("producer length"))?) as usize;
    if size > film::FRAME - 4 || bytes.len() != 4 + size {
        return Err(bad("producer frame"));
    }
    let result = film::parse(&bytes[4..], film::FRAME - 4).map_err(|_| bad("producer syntax"))?;
    Ok(Some(result))
}

fn storage() -> io::Result<()> {
    let mut expected = None;
    for path in ["/input", "/work", "/output", "/tmp", "/dev/shm"] {
        let file = File::open(path)?;
        // SAFETY: fstatfs receives the live descriptor and a correctly sized output struct.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatfs(file.as_raw_fd(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let device = file.metadata()?.dev();
        if stat.f_type != 0x01021994
            || stat.f_blocks * stat.f_bsize as u64 != film::STORAGE
            || stat.f_files != 4096
            || expected.is_some_and(|d| d != device)
        {
            return Err(bad("shared storage"));
        }
        expected = Some(device);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lost_controller_requires_a_transport_failure_not_invalid_protocol_data() {
        for kind in [
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::NotConnected,
        ] {
            let error = control::<()>(Err(io::Error::from(kind))).unwrap_err();
            assert!(lost_controller(&error));
        }
        for kind in [
            io::ErrorKind::InvalidData,
            io::ErrorKind::Other,
            io::ErrorKind::WouldBlock,
        ] {
            let error = control::<()>(Err(io::Error::from(kind))).unwrap_err();
            assert!(!lost_controller(&error));
        }
    }
    #[test]
    fn producer_frames_require_one_complete_closed_message() {
        let payload = format!(
            r#"{{"version":2,"kind":"film-producer-result","outcome":"engine-failed","detail":null,"launch_id":"{}","manifest":"{}","plan_sha256":"{}"}}"#,
            "a".repeat(32),
            "b".repeat(64),
            "c".repeat(64)
        );
        let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(payload.as_bytes());
        assert!(matches!(
            decode_producer(&bytes).unwrap(),
            Some(ProducerResult::Failure(_))
        ));
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode_producer(&extra).is_err());
        assert!(decode_producer(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode_producer(&u32::MAX.to_be_bytes()).is_err());
        assert!(decode_producer(&[0, 0, 0]).is_err());
        let bad = payload.replace("\"version\":2", "\"version\":2,\"version\":2");
        let mut duplicate = (bad.len() as u32).to_be_bytes().to_vec();
        duplicate.extend_from_slice(bad.as_bytes());
        assert!(decode_producer(&duplicate).is_err());
    }
}

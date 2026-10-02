//! Reject known-impossible standalone Film Exports before durable admission.
//!
//! The pinned density interpolation retains at least 180 bytes per source
//! pixel simultaneously. This is a lower bound, not whole-attempt qualification:
//! a frame passing it still needs the deployment's independent resource proof.
use std::{fs, io, path::Path};

const LIVE_BYTES_PER_PIXEL: u64 = 180;

pub(super) fn minimum_live_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * LIVE_BYTES_PER_PIXEL
}

fn read_limit_tree(root: &Path, membership: &str) -> io::Result<u64> {
    let relative = membership
        .strip_prefix('/')
        .ok_or_else(|| io::Error::other("the processing cgroup membership is not absolute"))?;
    if relative.split('/').any(|part| part == "." || part == "..") {
        return Err(io::Error::other(
            "the processing cgroup membership is invalid",
        ));
    }
    let mut current = root.join(relative);
    let mut effective = None;
    loop {
        // The hierarchy root may lack the memory controller. Every workload
        // ancestor below it must expose its limit; otherwise admission fails.
        let file = current.join("memory.max");
        match fs::read_to_string(file) {
            Ok(value) if value.trim() == "max" => {}
            Ok(value) => {
                let limit = value.trim().parse::<u64>().map_err(|_| {
                    io::Error::other("the processing cgroup memory limit is invalid")
                })?;
                effective = Some(effective.map_or(limit, |prior: u64| prior.min(limit)));
            }
            Err(error) if current == root && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if current == root {
            break;
        }
        if !current.pop() || !current.starts_with(root) {
            return Err(io::Error::other(
                "the processing cgroup hierarchy is invalid",
            ));
        }
    }
    effective.ok_or_else(|| io::Error::other("no finite processing memory limit is visible"))
}

pub(super) fn effective_memory_limit() -> io::Result<u64> {
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::other("the processing cgroup v2 membership is unavailable"))?;
    read_limit_tree(Path::new("/sys/fs/cgroup"), relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn portrait_camera_frame_cannot_fit_eight_gib() {
        let required = minimum_live_bytes(6376, 9568);
        assert_eq!(required, 10_981_002_240);
        assert!(required > 8 * 1024 * 1024 * 1024);
        assert!(required < 32 * 1024 * 1024 * 1024);
    }

    #[test]
    fn finite_ancestor_limit_wins_over_leaf_and_unbounded_limits() {
        let root = std::env::temp_dir().join(format!(
            "slipstream-film-memory-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("parent/leaf")).unwrap();
        fs::write(root.join("memory.max"), "max\n").unwrap();
        fs::write(root.join("parent/memory.max"), "8589934592\n").unwrap();
        fs::write(root.join("parent/leaf/memory.max"), "34359738368\n").unwrap();
        assert_eq!(read_limit_tree(&root, "/parent/leaf").unwrap(), 8589934592);
        fs::write(root.join("parent/memory.max"), "max\n").unwrap();
        assert_eq!(read_limit_tree(&root, "/parent/leaf").unwrap(), 34359738368);
        fs::write(root.join("parent/leaf/memory.max"), "max\n").unwrap();
        assert!(read_limit_tree(&root, "/parent/leaf").is_err());
        fs::remove_file(root.join("parent/leaf/memory.max")).unwrap();
        assert!(read_limit_tree(&root, "/parent/leaf").is_err());
        assert!(read_limit_tree(&root, "/../outside").is_err());
        fs::remove_dir_all(root).unwrap();
    }
}

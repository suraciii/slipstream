//! Bounded observations of the host that actually runs the fixed Film worker.
//! None of these values can be supplied by a public request.
use crate::{film, protocol::ErrorCode, qualified::Environment};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CStr,
    fmt,
    fs::File,
    io::Read,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, ErrorCode>;
const MAX_OBSERVATION: usize = 1024 * 1024;

pub(crate) fn observe(version: &str, runtimes: &str) -> Result<Environment> {
    let mut uname = std::mem::MaybeUninit::<libc::utsname>::zeroed();
    // SAFETY: uname receives a correctly sized writable utsname buffer.
    if unsafe { libc::uname(uname.as_mut_ptr()) } != 0 {
        return Err(ErrorCode::Unavailable);
    }
    // SAFETY: successful uname initialized the structure and its NUL-terminated fields.
    let uname = unsafe { uname.assume_init() };
    let text = |field: &[libc::c_char]| -> Result<String> {
        // SAFETY: these slices are the successful uname's terminated character arrays.
        unsafe { CStr::from_ptr(field.as_ptr()) }
            .to_str()
            .map(str::to_owned)
            .map_err(|_| ErrorCode::Unavailable)
    };
    let mut cpu = Vec::new();
    File::open("/proc/cpuinfo")
        .map_err(|_| ErrorCode::Unavailable)?
        .take(MAX_OBSERVATION as u64 + 1)
        .read_to_end(&mut cpu)
        .map_err(|_| ErrorCode::Unavailable)?;
    // SAFETY: sysconf has no preconditions for this fixed name.
    let page_bytes = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
        .map_err(|_| ErrorCode::Unavailable)?;
    let environment = Environment {
        machine: text(&uname.machine)?,
        kernel_release: text(&uname.release)?,
        page_bytes,
        cpu_sha256: cpu_identity(&cpu)?,
        manager_sha256: manager_identity(version.as_bytes(), runtimes.as_bytes())?,
    };
    environment.validate().map_err(|_| ErrorCode::Unavailable)?;
    Ok(environment)
}

pub(crate) fn launcher_identity() -> Result<String> {
    // /proc/self/exe names the running executable, including after an operator
    // replaces its original pathname. Stream it instead of allocating a file-sized buffer.
    let mut file = File::open("/proc/self/exe").map_err(|_| ErrorCode::Unavailable)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut count = 0usize;
    loop {
        let size = file.read(&mut buffer).map_err(|_| ErrorCode::Unavailable)?;
        if size == 0 {
            break;
        }
        count = count.checked_add(size).ok_or(ErrorCode::Unavailable)?;
        if count > 256 * 1024 * 1024 || Instant::now() >= deadline {
            return Err(ErrorCode::Unavailable);
        }
        digest.update(&buffer[..size]);
    }
    if count == 0 {
        return Err(ErrorCode::Unavailable);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn cpu_identity(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() || bytes.len() > MAX_OBSERVATION || !bytes.is_ascii() {
        return Err(ErrorCode::Unavailable);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ErrorCode::Unavailable)?;
    let keys = [
        "processor",
        "vendor_id",
        "cpu family",
        "model",
        "stepping",
        "microcode",
        "physical id",
        "core id",
        "flags",
    ];
    let mut entries = Vec::new();
    let mut record = BTreeMap::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !record.is_empty() {
                entries.push(std::mem::take(&mut record));
            }
            if entries.len() > 4096 {
                return Err(ErrorCode::Unavailable);
            }
            continue;
        }
        let (key, value) = line.split_once(':').ok_or(ErrorCode::Unavailable)?;
        let key = key.trim();
        if keys.contains(&key) {
            let value = value.trim();
            if value.is_empty() || record.insert(key, value).is_some() {
                return Err(ErrorCode::Unavailable);
            }
        }
    }
    if entries.is_empty() {
        return Err(ErrorCode::Unavailable);
    }
    let mut processors = BTreeMap::new();
    for entry in entries {
        if entry.len() != keys.len() {
            return Err(ErrorCode::Unavailable);
        }
        let processor = entry["processor"];
        if !processor.bytes().all(|c| c.is_ascii_digit()) {
            return Err(ErrorCode::Unavailable);
        }
        let processor = processor
            .parse::<u64>()
            .map_err(|_| ErrorCode::Unavailable)?;
        let mut value = Map::new();
        for key in keys {
            let field = match key {
                "processor" => Value::from(processor),
                "flags" => Value::Array(
                    entry[key]
                        .split_ascii_whitespace()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .map(Value::from)
                        .collect(),
                ),
                _ => Value::from(entry[key]),
            };
            value.insert(key.into(), field);
        }
        if processors.insert(processor, Value::Object(value)).is_some() {
            return Err(ErrorCode::Unavailable);
        }
    }
    film::hash(&processors.into_values().collect::<Vec<_>>()).map_err(|_| ErrorCode::Unavailable)
}

fn manager_identity(version: &[u8], runtimes: &[u8]) -> Result<String> {
    let version = observation_json(version)?;
    let runtimes = observation_json(runtimes)?;
    let runtime = &runtimes["runc"];
    if runtime["path"] != "runc" {
        return Err(ErrorCode::Unavailable);
    }
    let features = runtime["status"]["org.opencontainers.runtime-spec.features"]
        .as_str()
        .ok_or(ErrorCode::Unavailable)?;
    let features = observation_json(features.as_bytes())?;
    if !features.is_object() {
        return Err(ErrorCode::Unavailable);
    }
    let components = version["Components"]
        .as_array()
        .ok_or(ErrorCode::Unavailable)?;
    let mut identity = Map::new();
    for name in ["Engine", "containerd", "runc"] {
        let matching: Vec<_> = components.iter().filter(|v| v["Name"] == name).collect();
        if matching.len() != 1 {
            return Err(ErrorCode::Unavailable);
        }
        let component = matching[0];
        let component_version = component["Version"]
            .as_str()
            .ok_or(ErrorCode::Unavailable)?;
        let commit = component["Details"]["GitCommit"]
            .as_str()
            .ok_or(ErrorCode::Unavailable)?;
        for value in [component_version, commit] {
            if value.is_empty() || !value.bytes().all(|c| (b'!'..=b'~').contains(&c)) {
                return Err(ErrorCode::Unavailable);
            }
        }
        if name == "runc" {
            for (key, expected) in [
                ("org.opencontainers.runc.version", component_version),
                ("org.opencontainers.runc.commit", commit),
            ] {
                let observed = features["annotations"][key]
                    .as_str()
                    .ok_or(ErrorCode::Unavailable)?;
                if observed.strip_suffix('\n').unwrap_or(observed) != expected {
                    return Err(ErrorCode::Unavailable);
                }
            }
        }
        identity.insert(
            name.to_ascii_lowercase(),
            serde_json::json!({"version": component_version, "commit": commit}),
        );
    }
    identity.insert("runtime_features".into(), features);
    film::hash(&identity).map_err(|_| ErrorCode::Unavailable)
}

// Runtime observations contain nested vendor-owned feature objects. Keep those
// objects in their canonical shape while rejecting duplicates instead of
// silently discarding contradictory identity fields as Value normally does.
struct Observation(Value);
impl<'de> Deserialize<'de> for Observation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Observation;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded unambiguous JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(Observation(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(Observation(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(Observation(v.into()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(Observation(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(Observation(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(Observation(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(Observation(v)) = seq.next_element()? {
                    values.push(v)
                }
                Ok(Observation(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, Observation(value))) =
                    map.next_entry::<String, Observation>()?
                {
                    if values.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate observation key"));
                    }
                }
                Ok(Observation(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}
fn observation_json(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_OBSERVATION {
        return Err(ErrorCode::Unavailable);
    }
    serde_json::from_slice::<Observation>(bytes)
        .map(|v| v.0)
        .map_err(|_| ErrorCode::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cpu(number: u64, flags: &str) -> String {
        format!(
            "processor: {number}\nvendor_id: Test\ncpu family: 6\nmodel: 7\nstepping: 8\nmicrocode: 0x9\nphysical id: 0\ncore id: 0\nflags: {flags}\n\n"
        )
    }
    #[test]
    fn cpu_identity_normalizes_order_but_rejects_incomplete_or_duplicate_observation() {
        let original = format!("{}{}", cpu(0, "a b"), cpu(1, "b a a"));
        let reordered = format!("{}{}", cpu(1, "a b"), cpu(0, "b a"));
        assert_eq!(
            cpu_identity(original.as_bytes()).unwrap(),
            "ef1728316e28308f1e10d46afbe19facc3e36dc22cb5bfdd709e830430122b7c"
        );
        assert_eq!(
            cpu_identity(original.as_bytes()).unwrap(),
            cpu_identity(reordered.as_bytes()).unwrap()
        );
        for changed in [
            format!("{}{}", cpu(0, "a b"), cpu(0, "a b")),
            original.replace("microcode: 0x9\n", ""),
            original.replace("model: 7", "model: 7\nmodel: 8"),
        ] {
            assert_eq!(
                cpu_identity(changed.as_bytes()),
                Err(ErrorCode::Unavailable)
            );
        }
        assert_ne!(
            cpu_identity(original.as_bytes()).unwrap(),
            cpu_identity(original.replace("0x9", "0xa").as_bytes()).unwrap()
        );
    }
    fn manager() -> (Value, Value) {
        let components: [Value; 3] = ["Engine", "containerd", "runc"].map(
            |name| serde_json::json!({"Name":name,"Version":"1","Details":{"GitCommit":"abc"}}),
        );
        let features = serde_json::json!({"annotations":{"org.opencontainers.runc.version":"1\n","org.opencontainers.runc.commit":"abc"},"linux":{"cgroup":{"v2":true}}});
        (
            serde_json::json!({"Components":components}),
            serde_json::json!({"runc":{"path":"runc","status":{"org.opencontainers.runtime-spec.features":features.to_string()}}}),
        )
    }
    #[test]
    fn manager_identity_binds_the_selected_runtime_not_an_unrelated_installed_version() {
        let (version, runtimes) = manager();
        let original = manager_identity(
            &film::canonical(&version).unwrap(),
            &film::canonical(&runtimes).unwrap(),
        )
        .unwrap();
        assert_eq!(
            original,
            "6f88f77b954decd5a7830e984ec080f88301c90ac8ba2a6a833d97bc892d9841"
        );
        let mut changed = runtimes.clone();
        changed["other"] = serde_json::json!({"Version":"999"});
        assert_eq!(
            original,
            manager_identity(
                &film::canonical(&version).unwrap(),
                &film::canonical(&changed).unwrap()
            )
            .unwrap()
        );
        changed["runc"]["path"] = "other-runc".into();
        assert!(
            manager_identity(
                &film::canonical(&version).unwrap(),
                &film::canonical(&changed).unwrap()
            )
            .is_err()
        );
        let mut changed = version.clone();
        changed["Components"][2]["Version"] = "2".into();
        assert!(
            manager_identity(
                &film::canonical(&changed).unwrap(),
                &film::canonical(&runtimes).unwrap()
            )
            .is_err()
        );
        let mut changed = version.clone();
        let duplicate = changed["Components"][2].clone();
        changed["Components"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(
            manager_identity(
                &film::canonical(&changed).unwrap(),
                &film::canonical(&runtimes).unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn nested_observation_ambiguity_and_unbounded_inputs_are_unavailable() {
        for bytes in [
            br#"{"nested":{"key":1,"key":2}}"#.as_slice(),
            b"{\"key\":1e0}",
            b"{}{}",
            b"{\"key\":\"\\ud800\"}",
        ] {
            assert_eq!(observation_json(bytes), Err(ErrorCode::Unavailable));
        }
        assert!(observation_json(&vec![b' '; MAX_OBSERVATION + 1]).is_err());
        assert!(cpu_identity(&vec![b' '; MAX_OBSERVATION + 1]).is_err());
    }
}

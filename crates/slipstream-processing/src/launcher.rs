use slipstream_processing::{
    Executor,
    photo::{self, Response, ResultBody},
    protocol::{Availability, Config, PHOTO_CAPABILITY},
    serve,
};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

fn main() {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 4 && arguments[0] == "--check-production" {
        let Some(instance) = arguments[1].to_str() else {
            eprintln!("processing launcher readiness check requires a UTF-8 instance ID");
            std::process::exit(2);
        };
        let Some(policy) = arguments[2].to_str() else {
            eprintln!("processing launcher readiness check requires a UTF-8 policy identity");
            std::process::exit(2);
        };
        let Some(bundle) = arguments[3].to_str() else {
            eprintln!("processing launcher readiness check requires a UTF-8 bundle identity");
            std::process::exit(2);
        };
        if unsafe { libc::geteuid() } != 1000 {
            eprintln!("processing launcher readiness check must run as Web UID 1000");
            std::process::exit(2);
        }
        if let Err(error) = check_production(instance, policy, bundle) {
            eprintln!("processing launcher is not production-ready: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.len() != 2 || arguments[0] != "--config" || !Path::new(&arguments[1]).is_absolute()
    {
        eprintln!("Usage: slipstream-processing-launcher --config /absolute/config.json");
        eprintln!(
            "       slipstream-processing-launcher --check-production INSTANCE_ID POLICY_SHA256 BUNDLE_SHA256"
        );
        std::process::exit(2);
    }
    let result = (|| {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&arguments[1])
            .map_err(|_| ())?;
        let metadata = file.metadata().map_err(|_| ())?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
        {
            return Err(());
        }
        let mut bytes = Vec::new();
        file.take(16385).read_to_end(&mut bytes).map_err(|_| ())?;
        match selected_authority(&bytes) {
            Some(Authority::Photo) => {
                let config = photo::Config::parse(&bytes).map_err(|_| ())?;
                photo::serve(config).map_err(|_| ())
            }
            Some(Authority::Qualification) => {
                let config = Config::parse(&bytes).map_err(|_| ())?;
                let executor = Executor::open(config).map_err(|_| ())?;
                serve(executor).map_err(|_| ())
            }
            Some(Authority::Film) => {
                let config = slipstream_processing::film::Config::parse(&bytes).map_err(|_| ())?;
                let executor = Executor::open_film(config).map_err(|_| ())?;
                serve(executor).map_err(|_| ())
            }
            Some(Authority::Qualified) => {
                let config =
                    slipstream_processing::qualified::Config::parse(&bytes).map_err(|_| ())?;
                let executor = Executor::open_qualified(config).map_err(|_| ())?;
                serve(executor).map_err(|_| ())
            }
            None => Err(()),
        }
    })();
    if result.is_err() {
        eprintln!("Processing launcher unavailable; no image processing capability was enabled");
        std::process::exit(1);
    }
}

/// The processing authority a root-owned configuration selects. The production
/// Photo capability is matched first and the decision is exclusive: a
/// photo-processing configuration can never fall through to the fixture,
/// Film, or qualified executor, and no configuration selects two authorities.
fn selected_authority(bytes: &[u8]) -> Option<Authority> {
    if photo::Config::parse(bytes).is_ok() {
        return Some(Authority::Photo);
    }
    if Config::parse(bytes).is_ok() {
        return Some(Authority::Qualification);
    }
    if slipstream_processing::film::Config::parse(bytes).is_ok() {
        return Some(Authority::Film);
    }
    if slipstream_processing::qualified::Config::parse(bytes).is_ok() {
        return Some(Authority::Qualified);
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Authority {
    Photo,
    Qualification,
    Film,
    Qualified,
}

fn check_production(
    instance: &str,
    expected_policy: &str,
    expected_bundle: &str,
) -> Result<(), &'static str> {
    if instance.len() != 32
        || !instance
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("instance ID must be 32 lowercase hexadecimal characters");
    }
    if !hex(expected_policy, 64) || !hex(expected_bundle, 64) {
        return Err("policy and bundle IDs must be lowercase SHA-256 digests");
    }

    let socket = format!("/run/slipstream-processing/{instance}/launcher.sock");
    let response = photo::reconcile(&socket, instance.to_owned())
        .map_err(|_| "bounded launcher reconciliation request failed")?;

    ready_response(response, instance, expected_policy, expected_bundle)
}

fn ready_response(
    response: Response,
    instance: &str,
    expected_policy: &str,
    expected_bundle: &str,
) -> Result<(), &'static str> {
    match response {
        Response::Result {
            mode,
            version: 1,
            result,
        } if mode == PHOTO_CAPABILITY => match *result {
            ResultBody::Capability {
                capability,
                instance: response_instance,
                incarnation,
                next_sequence,
                policy,
                bundle,
                availability: Availability::Available,
                ..
            } if capability == PHOTO_CAPABILITY
                && response_instance == instance
                && hex(&incarnation, 32)
                && next_sequence > 0
                && policy == expected_policy
                && bundle == expected_bundle =>
            {
                Ok(())
            }
            _ => Err("launcher did not report an available photo-processing capability"),
        },
        _ => Err("launcher returned an incompatible reconciliation response"),
    }
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(capability: &str, availability: Availability) -> Response {
        Response::result(ResultBody::Capability {
            capability: capability.to_owned(),
            instance: "0123456789abcdef0123456789abcdef".to_owned(),
            incarnation: "a".repeat(32),
            next_sequence: 1,
            policy: "b".repeat(64),
            bundle: "c".repeat(64),
            availability,
            active: None,
        })
    }

    #[test]
    fn rejects_fixture_capabilities_even_when_the_socket_reports_available() {
        let response = capability("qualification-only", Availability::Available);

        assert!(
            ready_response(
                response,
                "0123456789abcdef0123456789abcdef",
                &"b".repeat(64),
                &"c".repeat(64),
            )
            .is_err()
        );
    }

    #[test]
    fn accepts_only_a_bound_available_photo_processing_capability() {
        let response = capability("photo-processing", Availability::Available);

        assert!(
            ready_response(
                response,
                "0123456789abcdef0123456789abcdef",
                &"b".repeat(64),
                &"c".repeat(64),
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_blocked_or_mismatched_production_capability() {
        assert!(
            ready_response(
                capability("photo-processing", Availability::Blocked),
                "0123456789abcdef0123456789abcdef",
                &"b".repeat(64),
                &"c".repeat(64),
            )
            .is_err()
        );
        assert!(
            ready_response(
                capability("photo-processing", Availability::Available),
                "ffffffffffffffffffffffffffffffff",
                &"b".repeat(64),
                &"c".repeat(64),
            )
            .is_err()
        );
        assert!(
            ready_response(
                capability("photo-processing", Availability::Available),
                "0123456789abcdef0123456789abcdef",
                &"d".repeat(64),
                &"c".repeat(64),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_noncanonical_instance_ids_before_connecting() {
        assert!(
            check_production(
                "../0123456789abcdef0123456789abcdef",
                &"b".repeat(64),
                &"c".repeat(64),
            )
            .is_err()
        );
        assert!(
            check_production(
                "0123456789ABCDEF0123456789ABCDEF",
                &"b".repeat(64),
                &"c".repeat(64),
            )
            .is_err()
        );
    }

    fn photo_configuration() -> Vec<u8> {
        let instance = "0123456789abcdef0123456789abcdef";
        let config = photo::Config {
            version: 1,
            mode: "photo-processing".into(),
            instance: instance.into(),
            root: format!("/var/lib/slipstream-processing/{instance}"),
            socket: format!("/run/slipstream-processing/{instance}/launcher.sock"),
            peer_uid: 1000,
            image: format!("sha256:{}", "1".repeat(64)),
            bundle: "2".repeat(64),
            policy: "3".repeat(64),
            source_bytes_max: 4 * 1024 * 1024 * 1024,
            staged_storage_bytes_max: 4 * 1024 * 1024 * 1024 + 4 * 1024 * 1024 * 1024,
            staged_storage_inodes_max: 4096,
            output_bytes_max: 4 * 1024 * 1024 * 1024,
            memory_bytes: 8 * 1024 * 1024 * 1024,
            cpu_quota_us: 400_000,
            tasks: 256,
            swap_bytes: 0,
            control_reserve_bytes: 256 * 1024 * 1024,
            shared_ancestor_headroom_bytes: 512 * 1024 * 1024,
            receipt_retention_seconds: 86_400,
        };
        serde_json::to_vec(&config).unwrap()
    }

    fn qualification_configuration() -> Vec<u8> {
        let config = Config {
            version: 1,
            mode: "qualification".into(),
            instance: "0123456789abcdef0123456789abcdef".into(),
            root: "/var/lib/slipstream-processing/qualification".into(),
            socket: "/run/slipstream-processing/qualification/launcher.sock".into(),
            peer_uid: 1000,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 128 * 1024 * 1024,
            receipt_retention_seconds: 86_400,
        };
        serde_json::to_vec(&config).unwrap()
    }

    fn film_configuration() -> Vec<u8> {
        let config = slipstream_processing::film::Config {
            version: 2,
            mode: "film-measurement".into(),
            instance: "0123456789abcdef0123456789abcdef".into(),
            root: "/var/lib/slipstream-processing/film".into(),
            socket: "/run/slipstream-processing/film/launcher.sock".into(),
            peer_uid: 0,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 8 * 1024 * 1024 * 1024,
            receipt_retention_seconds: 86_400,
            catalogue_sha256: "2".repeat(64),
            resource_model_sha256: "3".repeat(64),
        };
        serde_json::to_vec(&config).unwrap()
    }

    fn qualified_configuration() -> Vec<u8> {
        let config = slipstream_processing::qualified::Config {
            version: 3,
            mode: "film-qualified-fixtures".into(),
            instance: "0123456789abcdef0123456789abcdef".into(),
            root: "/var/lib/slipstream-processing/qualified".into(),
            socket: "/run/slipstream-processing/qualified/launcher.sock".into(),
            peer_uid: 0,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 8 * 1024 * 1024 * 1024,
            receipt_retention_seconds: 86_400,
            catalogue_sha256: "2".repeat(64),
            envelope_sha256: "3".repeat(64),
        };
        serde_json::to_vec(&config).unwrap()
    }

    #[test]
    fn a_photo_processing_configuration_selects_only_the_photo_authority() {
        let bytes = photo_configuration();
        assert_eq!(selected_authority(&bytes), Some(Authority::Photo));
        // The production Photo configuration is never accepted by another
        // authority's parser, so it can never open a fixture, Film, or
        // qualified executor.
        assert!(Config::parse(&bytes).is_err());
        assert!(slipstream_processing::film::Config::parse(&bytes).is_err());
        assert!(slipstream_processing::qualified::Config::parse(&bytes).is_err());
    }

    #[test]
    fn every_other_authority_selects_its_own_executor() {
        let bytes = qualification_configuration();
        assert_eq!(selected_authority(&bytes), Some(Authority::Qualification));

        let film = film_configuration();
        assert_eq!(selected_authority(&film), Some(Authority::Film));
        assert_eq!(
            selected_authority(&qualified_configuration()),
            Some(Authority::Qualified)
        );
    }

    #[test]
    fn an_unrecognized_configuration_selects_no_authority() {
        assert_eq!(selected_authority(b"{}"), None);
        assert_eq!(selected_authority(b""), None);
        // A fixture-mode or Film-mode envelope with the production workload
        // value is still that authority's configuration, never Photo's.
        let bytes = qualification_configuration();
        assert_eq!(selected_authority(&bytes), Some(Authority::Qualification));
    }
}

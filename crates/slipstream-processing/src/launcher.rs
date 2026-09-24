use slipstream_processing::{
    Executor,
    protocol::{Availability, Config, Request, Response, ResultBody},
    request, serve,
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
        let executor = match Config::parse(&bytes) {
            Ok(config) => Executor::open(config),
            Err(_) => match slipstream_processing::film::Config::parse(&bytes) {
                Ok(config) => Executor::open_film(config),
                Err(_) => slipstream_processing::qualified::Config::parse(&bytes)
                    .and_then(Executor::open_qualified),
            },
        }
        .map_err(|_| ())?;
        serve(executor).map_err(|_| ())
    })();
    if result.is_err() {
        eprintln!(
            "Processing qualification launcher unavailable; no image processing capability was enabled"
        );
        std::process::exit(1);
    }
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
    let response = request(
        &socket,
        &Request::Reconcile {
            version: 1,
            instance: instance.to_owned(),
        },
    )
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
        Response::Result { version: 1, result } => match *result {
            ResultBody::Capability {
                capability,
                instance: response_instance,
                incarnation,
                next_sequence,
                policy,
                bundle,
                availability: Availability::Available,
                ..
            } if capability == "photo-processing"
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
        Response::Result {
            version: 1,
            result: Box::new(ResultBody::Capability {
                capability: capability.to_owned(),
                instance: "0123456789abcdef0123456789abcdef".to_owned(),
                incarnation: "a".repeat(32),
                next_sequence: 1,
                policy: "b".repeat(64),
                bundle: "c".repeat(64),
                availability,
                active: None,
            }),
        }
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
}

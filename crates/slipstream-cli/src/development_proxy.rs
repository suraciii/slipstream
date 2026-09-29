//! On-demand Development Proxy reads and guarded writes. A create carries
//! the caller's observed source revision exactly once; the CLI never refreshes
//! that guard or retries an admitted build.

use super::{
    AdmissionState, CommandFailure, MutationIdentity, Operation, ServiceClient, read_input_bytes,
    valid_sha256,
};
use clap::Subcommand;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

const GET_OPERATION: Operation = Operation::PhotosProxyGet;
const CREATE_OPERATION: Operation = Operation::PhotosProxyCreate;
const REMOVE_OPERATION: Operation = Operation::PhotosProxyRemove;
const QUALITY_LIMIT: &str = "2560-long-edge";
const MAXIMUM_FAILURE_REASON_BYTES: usize = 120;

/// The `photos proxy` subcommands.
#[derive(Debug, Subcommand)]
pub enum DevelopmentProxyCommand {
    /// Read one Photo's Development Proxy state and facts.
    Get {
        /// One Photo ID.
        #[arg(value_name = "PHOTO_ID", value_parser = super::nonempty)]
        photo_id: String,
    },
    /// Build or adopt the current Development Proxy against an observed
    /// source revision.
    Create {
        /// One Photo ID.
        #[arg(value_name = "PHOTO_ID", value_parser = super::nonempty)]
        photo_id: String,
        /// UTF-8 JSON file holding exactly the server create body
        /// `{"expectedSourceRevision":"observed-source-revision"}`; `-` reads
        /// the document from stdin. A RAW source revision contains NUL
        /// separators, so it cannot be passed as a command-line argument.
        #[arg(long, value_name = "FILE", value_parser = super::nonempty)]
        input: String,
    },
    /// Remove one Photo's Development Proxy.
    Remove {
        /// One Photo ID.
        #[arg(value_name = "PHOTO_ID", value_parser = super::nonempty)]
        photo_id: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyStateWire {
    photo_id: String,
    state: String,
    #[serde(deserialize_with = "required_nullable")]
    proxy: Option<ProxyFactsWire>,
    #[serde(deserialize_with = "required_nullable")]
    failure: Option<ProxyFailureWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyFactsWire {
    proxy_id: String,
    source_revision: String,
    source_size: u64,
    source_sha256: String,
    source_profile_id: String,
    pipeline_version: String,
    long_edge: u32,
    width: u32,
    height: u32,
    quality_limit: String,
    byte_length: u64,
    sha256: String,
    created_at: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyFailureWire {
    reason: String,
    at: u64,
    retryable: bool,
    category: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyBuildingWire {
    photo_id: String,
    state: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProxyRemovalWire {
    photo_id: String,
    state: String,
    removed: bool,
}

/// The one accepted `photos proxy create --input` document shape: exactly the
/// server's create body. Derived decoding rejects unknown keys, duplicate
/// keys, trailing content, and non-object documents. The revision is carried
/// verbatim, including the NUL separators a RAW source revision contains.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateProxyInput {
    expected_source_revision: String,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn proxy_valid(proxy: &ProxyFactsWire) -> bool {
    valid_sha256(&proxy.proxy_id)
        && !proxy.source_revision.is_empty()
        && proxy.source_size > 0
        && valid_sha256(&proxy.source_sha256)
        && !proxy.source_profile_id.is_empty()
        && !proxy.pipeline_version.is_empty()
        && proxy.long_edge > 0
        && proxy.width > 0
        && proxy.height > 0
        && proxy.quality_limit == QUALITY_LIMIT
        && proxy.byte_length > 0
        && valid_sha256(&proxy.sha256)
        && proxy.created_at > 0
}

fn failure_valid(failure: &ProxyFailureWire) -> bool {
    !failure.reason.is_empty()
        && failure.reason.chars().count() <= MAXIMUM_FAILURE_REASON_BYTES
        && failure.at > 0
        && match failure.category.as_str() {
            "capacity" => failure.retryable,
            "terminal" => !failure.retryable,
            _ => false,
        }
}

fn state_valid(state: &str, has_proxy: bool) -> bool {
    match state {
        "current" | "stale" => has_proxy,
        "building" => true,
        "absent" => !has_proxy,
        _ => false,
    }
}

fn validated_state<F>(
    value: ProxyStateWire,
    photo_id: &str,
    invalid: F,
    require_current: bool,
) -> Result<Value, CommandFailure>
where
    F: Fn() -> CommandFailure,
{
    if value.photo_id != photo_id
        || !state_valid(&value.state, value.proxy.is_some())
        || (require_current && (value.state != "current" || value.proxy.is_none()))
        || !value.proxy.as_ref().is_none_or(proxy_valid)
        || !value.failure.as_ref().is_none_or(failure_valid)
    {
        return Err(invalid());
    }
    serde_json::to_value(value).map_err(|_| invalid())
}

/// Reads and validates one `photos proxy create --input` document before any
/// service contact, mirroring the other `--input` writes: the guard is the
/// caller's observed revision, carried verbatim.
pub(super) async fn prepare(
    command: &DevelopmentProxyCommand,
) -> Result<Option<Value>, CommandFailure> {
    match command {
        DevelopmentProxyCommand::Create { input, .. } => {
            Ok(Some(parse_create(read_input_bytes(input).await?)?))
        }
        _ => Ok(None),
    }
}

/// Validates the create body in the service's own admission order: exactly
/// one revision guard, then its nonempty requirement. The revision is opaque
/// and is never re-read or repaired here.
fn parse_create(bytes: Vec<u8>) -> Result<Value, CommandFailure> {
    let input: CreateProxyInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly expectedSourceRevision, carrying the \
             observed source revision verbatim.",
        )
    })?;
    if input.expected_source_revision.is_empty() {
        return Err(CommandFailure::invalid(
            "expectedSourceRevision",
            "The source revision is required.",
        ));
    }
    serde_json::to_value(&input)
        .map_err(|_| CommandFailure::invalid("input", "The input document could not be rendered."))
}

pub(super) async fn execute(
    client: &ServiceClient,
    admission: &AdmissionState,
    command: &DevelopmentProxyCommand,
    prepared: Option<Value>,
) -> Result<Value, CommandFailure> {
    match command {
        DevelopmentProxyCommand::Get { photo_id } => {
            let value: ProxyStateWire = client
                .json(
                    GET_OPERATION,
                    Method::GET,
                    client.endpoint(&["api", "photos", photo_id, "development-proxy"]),
                    None,
                )
                .await?;
            validated_state(
                value,
                photo_id,
                || CommandFailure::transport(GET_OPERATION),
                false,
            )
        }
        DevelopmentProxyCommand::Create { photo_id, .. } => {
            let body = prepared.ok_or_else(|| {
                CommandFailure::invalid("input", "The input document could not be rendered.")
            })?;
            let identity = MutationIdentity {
                operation: CREATE_OPERATION,
                photo_ids: vec![photo_id.clone()],
                album_id: None,
                album_name: None,
                mappings: Vec::new(),
            };
            let (status, bytes) = client
                .mutation_statuses(
                    Method::POST,
                    &identity,
                    admission,
                    client.endpoint(&["api", "photos", photo_id, "development-proxy"]),
                    Some(body),
                    &[StatusCode::OK, StatusCode::ACCEPTED],
                )
                .await?;
            let unknown = || CommandFailure::unknown(&identity);
            if status == StatusCode::ACCEPTED {
                let building: ProxyBuildingWire =
                    serde_json::from_slice(&bytes).map_err(|_| unknown())?;
                if building.photo_id != *photo_id || building.state != "building" {
                    return Err(unknown());
                }
                return Ok(json!({
                    "photoId": building.photo_id,
                    "state": "building",
                }));
            }
            let value: ProxyStateWire = serde_json::from_slice(&bytes).map_err(|_| unknown())?;
            validated_state(value, photo_id, unknown, true)
        }
        DevelopmentProxyCommand::Remove { photo_id } => {
            let identity = MutationIdentity {
                operation: REMOVE_OPERATION,
                photo_ids: vec![photo_id.clone()],
                album_id: None,
                album_name: None,
                mappings: Vec::new(),
            };
            let (_status, bytes) = client
                .mutation_statuses(
                    Method::DELETE,
                    &identity,
                    admission,
                    client.endpoint(&["api", "photos", photo_id, "development-proxy"]),
                    None,
                    &[StatusCode::OK],
                )
                .await?;
            let unknown = || CommandFailure::unknown(&identity);
            let value: ProxyRemovalWire = serde_json::from_slice(&bytes).map_err(|_| unknown())?;
            if value.photo_id != *photo_id || value.state != "absent" {
                return Err(unknown());
            }
            serde_json::to_value(value).map_err(|_| unknown())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> String {
        "a".repeat(64)
    }

    fn proxy() -> ProxyFactsWire {
        ProxyFactsWire {
            proxy_id: digest(),
            source_revision: "source-1".to_owned(),
            source_size: 1,
            source_sha256: digest(),
            source_profile_id: "profile-1".to_owned(),
            pipeline_version: "pipeline-1".to_owned(),
            long_edge: 2560,
            width: 100,
            height: 80,
            quality_limit: QUALITY_LIMIT.to_owned(),
            byte_length: 1,
            sha256: digest(),
            created_at: 1,
        }
    }

    fn state(state: &str, proxy: Option<ProxyFactsWire>) -> ProxyStateWire {
        ProxyStateWire {
            photo_id: "p1".to_owned(),
            state: state.to_owned(),
            proxy,
            failure: None,
        }
    }

    fn transport() -> CommandFailure {
        CommandFailure::transport(GET_OPERATION)
    }

    #[test]
    fn state_matrix_accepts_closed_states_and_proxy_shapes() {
        for (name, facts) in [
            ("current", Some(proxy())),
            ("stale", Some(proxy())),
            ("building", Some(proxy())),
            ("building", None),
            ("absent", None),
        ] {
            validated_state(state(name, facts), "p1", transport, false)
                .unwrap_or_else(|_| panic!("{name} should validate"));
        }
        for (name, facts) in [
            ("current", None),
            ("stale", None),
            ("absent", Some(proxy())),
            ("unknown", None),
        ] {
            assert!(validated_state(state(name, facts), "p1", transport, false).is_err());
        }
    }

    #[test]
    fn proxy_facts_and_failures_have_closed_bounds() {
        let mut value = state("current", Some(proxy()));
        value.failure = Some(ProxyFailureWire {
            reason: "failed".to_owned(),
            at: 1,
            retryable: false,
            category: "terminal".to_owned(),
        });
        validated_state(value, "p1", transport, false).expect("valid failure");

        let mut invalid = state("current", Some(proxy()));
        invalid.proxy.as_mut().unwrap().proxy_id = "bad".to_owned();
        assert!(validated_state(invalid, "p1", transport, false).is_err());
        let mut invalid = state("current", Some(proxy()));
        invalid.proxy.as_mut().unwrap().byte_length = 0;
        assert!(validated_state(invalid, "p1", transport, false).is_err());
        let mut invalid = state("current", Some(proxy()));
        invalid.proxy.as_mut().unwrap().quality_limit = "other".to_owned();
        assert!(validated_state(invalid, "p1", transport, false).is_err());
        let mut invalid = state("current", Some(proxy()));
        invalid.failure = Some(ProxyFailureWire {
            reason: "x".repeat(MAXIMUM_FAILURE_REASON_BYTES + 1),
            at: 1,
            retryable: false,
            category: "terminal".to_owned(),
        });
        assert!(validated_state(invalid, "p1", transport, false).is_err());
    }

    #[test]
    fn wire_shapes_reject_unknown_fields_and_missing_nullable_keys() {
        let mut document = serde_json::to_value(state("absent", None)).unwrap();
        document["extra"] = json!(true);
        assert!(serde_json::from_value::<ProxyStateWire>(document).is_err());
        let mut document = serde_json::to_value(state("absent", None)).unwrap();
        document.as_object_mut().unwrap().remove("failure");
        assert!(serde_json::from_value::<ProxyStateWire>(document).is_err());

        assert!(
            serde_json::from_str::<ProxyBuildingWire>(
                r#"{"photoId":"p1","state":"building","extra":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<ProxyRemovalWire>(
                r#"{"photoId":"p1","state":"absent","removed":true,"extra":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn current_create_response_requires_a_proxy() {
        assert!(validated_state(state("building", None), "p1", transport, true).is_err());
        validated_state(state("current", Some(proxy())), "p1", transport, true)
            .expect("current create response");
    }

    #[test]
    fn server_building_and_failed_states_parse_and_expose_retryability() {
        // The exact documents the service emits for one admitted build and
        // for one settled failure.
        let building: ProxyStateWire = serde_json::from_str(
            r#"{"photoId":"p1","state":"building","proxy":null,"failure":null}"#,
        )
        .expect("server building state");
        let building = validated_state(building, "p1", transport, false).expect("building valid");
        assert_eq!(building["state"], "building");
        assert!(building["failure"].is_null());

        let failed: ProxyStateWire = serde_json::from_str(
            r#"{"photoId":"p1","state":"absent","proxy":null,"failure":{"reason":"processing capacity exhausted","at":1790000000,"retryable":true,"category":"capacity"}}"#,
        )
        .expect("server retryable failure");
        let failed = validated_state(failed, "p1", transport, false).expect("failure valid");
        assert_eq!(failed["failure"]["reason"], "processing capacity exhausted");
        assert_eq!(failed["failure"]["at"], 1_790_000_000_u64);
        assert_eq!(failed["failure"]["retryable"], true);
        assert_eq!(failed["failure"]["category"], "capacity");

        let terminal: ProxyStateWire = serde_json::from_str(
            r#"{"photoId":"p1","state":"absent","proxy":null,"failure":{"reason":"source profile unsupported","at":1790000001,"retryable":false,"category":"terminal"}}"#,
        )
        .expect("server terminal failure");
        let terminal = validated_state(terminal, "p1", transport, false).expect("terminal valid");
        assert_eq!(terminal["failure"]["retryable"], false);
        assert_eq!(terminal["failure"]["category"], "terminal");
    }

    #[test]
    fn failure_retryability_must_match_its_category() {
        for (retryable, category) in [
            (true, "terminal"),
            (false, "capacity"),
            (true, "unknown"),
            (false, "unknown"),
        ] {
            let mut value = state("absent", None);
            value.failure = Some(ProxyFailureWire {
                reason: "failed".to_owned(),
                at: 1,
                retryable,
                category: category.to_owned(),
            });
            assert!(
                validated_state(value, "p1", transport, false).is_err(),
                "retryable={retryable} category={category} must not validate"
            );
        }
        // The service's closed set validates truthfully.
        for (retryable, category) in [(true, "capacity"), (false, "terminal")] {
            let mut value = state("absent", None);
            value.failure = Some(ProxyFailureWire {
                reason: "failed".to_owned(),
                at: 1,
                retryable,
                category: category.to_owned(),
            });
            validated_state(value, "p1", transport, false)
                .unwrap_or_else(|_| panic!("retryable={retryable} category={category}"));
        }
    }

    #[test]
    fn failure_wire_requires_the_full_server_shape() {
        // The pre-fix two-field failure document must no longer parse.
        assert!(
            serde_json::from_str::<ProxyStateWire>(
                r#"{"photoId":"p1","state":"absent","proxy":null,"failure":{"reason":"failed","at":1}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<ProxyStateWire>(
                r#"{"photoId":"p1","state":"absent","proxy":null,"failure":{"reason":"failed","at":1,"retryable":false,"category":"terminal","extra":true}}"#
            )
            .is_err()
        );
    }

    /// A RAW source revision is `name\0size\0mtimeMs`. The create guard must
    /// survive the JSON document intact because argv cannot carry a NUL byte.
    #[test]
    fn create_document_carries_a_nul_bearing_revision_verbatim() {
        let revision = "_R5_5063.ARW\u{0}130281472\u{0}1790602639675.6853";
        let body = parse_create(
            serde_json::json!({ "expectedSourceRevision": revision })
                .to_string()
                .into_bytes(),
        )
        .expect("the observed revision must be accepted");
        assert_eq!(body["expectedSourceRevision"], serde_json::json!(revision));
        for refused in [
            serde_json::json!({}),
            serde_json::json!({ "expectedSourceRevision": "" }),
            serde_json::json!({ "expectedSourceRevision": "rev", "requestId": "edit-001" }),
            serde_json::json!({ "expected_source_revision": "rev" }),
            serde_json::json!([{ "expectedSourceRevision": "rev" }]),
            serde_json::json!("expectedSourceRevision"),
        ] {
            assert!(
                parse_create(refused.to_string().into_bytes()).is_err(),
                "must refuse: {refused}"
            );
        }
    }
}

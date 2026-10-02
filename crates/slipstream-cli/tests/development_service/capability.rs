use super::*;

/// The report an enabled deployment with a missing engine bundle serves:
/// `bundle-unavailable` with null identities, in the service's own shape.
fn bundle_unavailable_capability() -> Value {
    json!({
        "state": "bundle-unavailable",
        "bundleId": null,
        "incarnation": null,
        "exposure": {"minimumEv": 0.0, "maximumEv": 1.0, "stepEv": 0.001},
        "profiles": [
            {
                "profileId": "sony-ilce-7rm5-arw",
                "whiteBalanceModes": ["as-shot"],
                "whiteBalanceRanges": null
            },
            {
                "profileId": "sony-ilce-7cm2-arw",
                "whiteBalanceModes": ["as-shot"],
                "whiteBalanceRanges": null
            }
        ],
        "stages": {"develop": "unavailable", "film": "unavailable"}
    })
}

/// A deployment whose engine bundle is missing or invalid answers with the
/// closed `bundle-unavailable` condition and null identities; the CLI passes
/// the report through unchanged instead of failing the read.
#[tokio::test]
async fn processing_capability_accepts_the_missing_extension_report() {
    let report = bundle_unavailable_capability();
    let service = fake_service(vec![
        Step::Capabilities,
        Step::CapabilityReport(report.clone()),
    ]);
    let (exit, envelope) = command(&service.url, &["processing", "capability"]).await;
    assert_eq!(exit, 0);
    assert_eq!(envelope["data"], report);
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2, "the handshake and the one report read");
    assert!(
        requests[0]
            .request_line
            .starts_with("GET /api/capabilities")
    );
    assert!(
        requests[1]
            .request_line
            .starts_with("GET /api/processing/capability")
    );
}

/// The retired `launcher-unavailable` condition is no longer a state the
/// service may report: a response carrying it is a transport failure, not a
/// claimed condition.
#[tokio::test]
async fn processing_capability_refuses_the_retired_launcher_state() {
    let mut report = bundle_unavailable_capability();
    report["state"] = json!("launcher-unavailable");
    let service = fake_service(vec![Step::Capabilities, Step::CapabilityReport(report)]);
    let (exit, envelope) = command(&service.url, &["processing", "capability"]).await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "processing-capability"
    );
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2, "the handshake and the one report read");
    assert!(
        requests[1]
            .request_line
            .starts_with("GET /api/processing/capability")
    );
}

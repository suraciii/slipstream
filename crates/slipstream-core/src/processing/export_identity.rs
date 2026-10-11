use super::*;

/// The confirmed acceptance identity of one explicit Export: the confirmed
/// input binding with its verified byte evidence, the selected module with
/// its pinned adapter/schema version, the canonical digest of the complete
/// captured step parameters, the intended output contract, and the processing
/// bundle. It is captured at acceptance, never inherits whichever settings
/// happen to be current when queued work starts, and deliberately carries no
/// Preview geometry or display derivation: the Export output geometry is the
/// one inside the intended output contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingExportIdentity {
    pub input: ProcessingInputEvidence,
    pub module: ProcessingModuleId,
    /// The pinned adapter and parameter-schema identity used at acceptance.
    pub adapter_schema_version: String,
    /// The canonical digest of the complete captured parameter snapshot.
    pub parameter_digest: String,
    /// The intended output contract, including the separately admitted output
    /// geometry.
    pub output_contract: ProcessingImageContract,
    /// The exact processing bundle identity.
    pub bundle_id: String,
}

impl ProcessingExportIdentity {
    /// Validates every component of the identity.
    pub fn validate(&self) -> Result<(), ProcessingContractError> {
        self.input.validate()?;
        validate_bounded_name(&self.adapter_schema_version, MAXIMUM_CONTRACT_NAME_BYTES)?;
        validate_digest(&self.parameter_digest)?;
        self.output_contract.validate()?;
        validate_bounded_name(&self.bundle_id, MAXIMUM_CONTRACT_NAME_BYTES)?;
        Ok(())
    }

    /// The canonical digest of the complete Export acceptance identity.
    pub fn digest(&self) -> String {
        let payload = serde_json::json!({
            "kind": "processing-export-identity-v1",
            "input": self.input.digest(),
            "module": self.module.as_str(),
            "adapter_schema_version": self.adapter_schema_version,
            "parameter_digest": self.parameter_digest,
            "output_contract": self.output_contract.canonical_digest(),
            "bundle_id": self.bundle_id,
        });
        digest_json(&payload)
    }
}

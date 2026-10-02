//! The closed reading of one published immutable Processing Artifact: the
//! provenance record the service settles, the explicit input binding a later
//! Processing Step may select from it, and the field-for-field check an
//! artifact bytes download must pass before the browser is offered the file.
//!
//! [Composable Photo Processing Modules](../../../../../../design/processing-modules.md)
//! keeps an artifact immutable: its provenance identifies its input, module,
//! complete parameter snapshot, bundle, output contract, and validated byte
//! identity. A downstream step binds to that record explicitly; nothing here
//! retargets a binding when upstream changes, and a download that does not
//! repeat the record field for field is refused rather than offered.

import type { ComposableRecipeInput } from "../api/composable-recipe.js";
import { isRecord } from "../api/editor.js";
import { blobSha256Hex } from "./browser-crypto.js";

/// One published immutable Processing Artifact: the exact provenance the
/// service validated and settled, with the published bytes' own identity.
export type ProcessingArtifactRecord = Readonly<{
  artifactId: string;
  photoId: string;
  stepId: string;
  module: string;
  adapterSchemaVersion: string;
  parameters: Readonly<{ schemaVersion: string; tree: unknown }>;
  input: Readonly<{
    binding: ComposableRecipeInput;
    sha256: string;
    byteLength: number;
  }>;
  outputContract: Readonly<{
    format: string;
    precision: string;
    colorSpace: string;
    transfer: string;
    geometry: Readonly<{ width: number; height: number }>;
    encoding: string;
  }>;
  bundleId: string;
  sha256: string;
  byteLength: number;
}>;

const readString = (
  source: Record<string, unknown>,
  key: string,
): string | undefined => {
  const value = source[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
};

const readCount = (
  source: Record<string, unknown>,
  key: string,
): number | undefined => {
  const value = source[key];
  return typeof value === "number" && Number.isInteger(value) && value > 0
    ? value
    : undefined;
};

const readInputBinding = (
  value: unknown,
): ComposableRecipeInput | undefined => {
  if (!isRecord(value) || typeof value["kind"] !== "string") return undefined;
  if (
    value["kind"] === "original" &&
    typeof value["photoId"] === "string" &&
    typeof value["sourceRevision"] === "string"
  )
    return Object.freeze({
      kind: "original",
      photoId: value["photoId"],
      sourceRevision: value["sourceRevision"],
    });
  if (
    value["kind"] === "artifact" &&
    typeof value["artifactId"] === "string" &&
    isRecord(value["contract"])
  )
    return Object.freeze({
      kind: "artifact",
      artifactId: value["artifactId"],
      contract: Object.freeze({ ...value["contract"] }),
    });
  return undefined;
};

/// The closed reading of one artifact provenance record. An unknown shape is
/// not an artifact, so a client never presents provenance it cannot
/// attribute to the service.
export const parseProcessingArtifactRecord = (
  value: unknown,
): ProcessingArtifactRecord | undefined => {
  if (!isRecord(value)) return undefined;
  const artifactId = readString(value, "artifactId");
  const photoId = readString(value, "photoId");
  const stepId = readString(value, "stepId");
  const module = readString(value, "module");
  const adapterSchemaVersion = readString(value, "adapterSchemaVersion");
  const bundleId = readString(value, "bundleId");
  const sha256 = readString(value, "sha256");
  const byteLength = readCount(value, "byteLength");
  if (
    !artifactId ||
    !photoId ||
    !stepId ||
    !module ||
    !adapterSchemaVersion ||
    !bundleId ||
    !sha256 ||
    !/^[0-9a-f]{64}$/.test(sha256) ||
    !byteLength
  )
    return undefined;
  const parameters = value["parameters"];
  if (
    !isRecord(parameters) ||
    typeof parameters["schemaVersion"] !== "string" ||
    !parameters["schemaVersion"]
  )
    return undefined;
  const input = value["input"];
  if (
    !isRecord(input) ||
    typeof input["sha256"] !== "string" ||
    !/^[0-9a-f]{64}$/.test(input["sha256"]) ||
    typeof input["byteLength"] !== "number" ||
    !Number.isInteger(input["byteLength"]) ||
    input["byteLength"] <= 0
  )
    return undefined;
  const binding = readInputBinding(input["binding"]);
  if (!binding) return undefined;
  const contract = value["outputContract"];
  const format = isRecord(contract)
    ? readString(contract, "format")
    : undefined;
  const precision = isRecord(contract)
    ? readString(contract, "precision")
    : undefined;
  const colorSpace = isRecord(contract)
    ? readString(contract, "colorSpace")
    : undefined;
  const transfer = isRecord(contract)
    ? readString(contract, "transfer")
    : undefined;
  const encoding = isRecord(contract)
    ? readString(contract, "encoding")
    : undefined;
  const width =
    isRecord(contract) && isRecord(contract["geometry"])
      ? readCount(contract["geometry"], "width")
      : undefined;
  const height =
    isRecord(contract) && isRecord(contract["geometry"])
      ? readCount(contract["geometry"], "height")
      : undefined;
  if (
    !isRecord(contract) ||
    !isRecord(contract["geometry"]) ||
    !format ||
    !precision ||
    !colorSpace ||
    !transfer ||
    !encoding ||
    !width ||
    !height
  )
    return undefined;
  return Object.freeze({
    artifactId,
    photoId,
    stepId,
    module,
    adapterSchemaVersion,
    parameters: Object.freeze({
      schemaVersion: parameters["schemaVersion"],
      tree: parameters["tree"],
    }),
    input: Object.freeze({
      binding,
      sha256: input["sha256"],
      byteLength: input["byteLength"],
    }),
    outputContract: Object.freeze({
      format,
      precision,
      colorSpace,
      transfer,
      geometry: Object.freeze({ width, height }),
      encoding,
    }),
    bundleId,
    sha256,
    byteLength,
  });
};

/// The explicit artifact input binding one later Processing Step may select:
/// the artifact's own identity plus the concrete image contract its record
/// validated. This is the only composition edge; no "latest result" binding
/// exists.
export const processingArtifactInput = (
  artifact: ProcessingArtifactRecord,
): ComposableRecipeInput =>
  Object.freeze({
    kind: "artifact",
    artifactId: artifact.artifactId,
    contract: Object.freeze({
      format: artifact.outputContract.format,
      precision: artifact.outputContract.precision,
      colorSpace: artifact.outputContract.colorSpace,
      transfer: artifact.outputContract.transfer,
      geometry: Object.freeze({
        width: artifact.outputContract.geometry.width,
        height: artifact.outputContract.geometry.height,
      }),
      encoding: artifact.outputContract.encoding,
    }),
  });

/// Whether a served artifact bytes stream is the one the record described.
/// Every fact the download publishes must match, so a substituted,
/// truncated, or differently sized artifact is refused instead of offered.
export const processingArtifactMatchesHeaders = (
  artifact: ProcessingArtifactRecord,
  headers: Readonly<{ get(name: string): string | null }>,
): boolean =>
  headers.get("slipstream-artifact-id") === artifact.artifactId &&
  headers.get("slipstream-artifact-photo-id") === artifact.photoId &&
  headers.get("slipstream-artifact-step-id") === artifact.stepId &&
  headers.get("slipstream-artifact-module") === artifact.module &&
  headers.get("slipstream-artifact-adapter-schema-version") ===
    artifact.adapterSchemaVersion &&
  headers.get("slipstream-artifact-bundle-id") === artifact.bundleId &&
  headers.get("slipstream-artifact-width") ===
    String(artifact.outputContract.geometry.width) &&
  headers.get("slipstream-artifact-height") ===
    String(artifact.outputContract.geometry.height) &&
  headers.get("slipstream-artifact-byte-length") ===
    String(artifact.byteLength) &&
  headers.get("slipstream-artifact-sha256") === artifact.sha256;

/// Why a served artifact bytes body cannot be published, or `""` when it
/// can: the body must be exactly the record's byte length and must hash to
/// the record's published digest.
export const processingArtifactDigestRefusal = async (
  artifact: ProcessingArtifactRecord,
  bytes: ArrayBuffer,
): Promise<string> => {
  if (bytes.byteLength !== artifact.byteLength)
    return "The downloaded Processing Artifact did not match its record and was discarded.";
  const digest = await blobSha256Hex(new Blob([bytes])).catch(() => undefined);
  return digest === artifact.sha256
    ? ""
    : "The downloaded Processing Artifact did not match its digest and was discarded.";
};

/// The disclosed provenance line of one retained artifact, in the
/// workspace's words.
export const describeProcessingArtifact = (
  artifact: ProcessingArtifactRecord,
  byteCount: (bytes: number) => string,
): string =>
  `Processing Artifact ${artifact.artifactId}: ${artifact.module} over ${
    artifact.input.binding.kind === "artifact"
      ? `artifact ${artifact.input.binding.artifactId}`
      : "the Original"
  }, ${artifact.outputContract.format} ${artifact.outputContract.geometry.width}×${artifact.outputContract.geometry.height}, ${byteCount(artifact.byteLength)}, sha256 ${artifact.sha256.slice(0, 12)}…`;

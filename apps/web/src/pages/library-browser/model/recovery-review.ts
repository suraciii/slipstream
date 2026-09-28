import type { RecoveryApplyMapping, RecoveryMapping } from "../api/recovery.js";

export type { RecoveryApplyMapping, RecoveryMapping } from "../api/recovery.js";

/// The mapping facts the apply decision reads. A reviewed mapping carries
/// them all; the recovery panel presents exactly these facts.
export type ReviewableRecoveryMapping = Readonly<{
  mappingId: string;
  originalId: string;
  toLocation: string;
  verified: boolean;
  blockedReason: RecoveryMapping["blockedReason"];
  retire: Readonly<{ photoId: string }> | null;
}>;

/// The explicit per-mapping choices a Photographer made in the recovery
/// panel. Both default to not chosen: a mapping is never applied on the
/// strength of a default-selected control.
export type RecoveryMappingChoice = Readonly<{
  /// The Photographer explicitly accepted retiring the destination's
  /// discovered Photo so this mapping can bind to its path.
  retireChosen: boolean;
  /// The Photographer explicitly acknowledged that no fingerprint can prove
  /// the destination's old content and accepted applying anyway.
  unverifiedConfirmed: boolean;
}>;

/// The reviewed choices keyed by the mappingId they confirm.
export type RecoveryMappingChoices = ReadonlyMap<string, RecoveryMappingChoice>;

export const noRecoveryChoice: RecoveryMappingChoice = Object.freeze({
  retireChosen: false,
  unverifiedConfirmed: false,
});

/// A mapping the apply step may commit: never blocked, and carrying every
/// confirmation its evidence requires. An unverified mapping needs the
/// explicit unverified-content acknowledgement, and a mapping with a retire
/// candidate needs the explicit retire choice.
export const isRecoveryMappingApplicable = (
  mapping: ReviewableRecoveryMapping,
  choice: RecoveryMappingChoice = noRecoveryChoice,
): boolean =>
  mapping.blockedReason === null &&
  (mapping.verified || choice.unverifiedConfirmed) &&
  (mapping.retire === null || choice.retireChosen);

/// Builds the reviewed apply batch from the mappings the panel shows and the
/// choices made for them. Each item repeats its reviewed mappingId; the
/// unverified-content confirmation is sent exactly for the unverified
/// mappings in the batch, and `retirePhotoId` exactly for retire candidates
/// whose retire choice was made.
export const recoveryApplyMappings = (
  mappings: ReadonlyArray<ReviewableRecoveryMapping>,
  choices: RecoveryMappingChoices,
): ReadonlyArray<RecoveryApplyMapping> =>
  mappings.flatMap((mapping) => {
    const choice = choices.get(mapping.mappingId) ?? noRecoveryChoice;
    if (!isRecoveryMappingApplicable(mapping, choice)) return [];
    return [
      {
        originalId: mapping.originalId,
        newLocation: mapping.toLocation,
        mappingId: mapping.mappingId,
        ...(mapping.verified ? {} : { confirmUnverifiedContent: true }),
        ...(mapping.retire !== null && choice.retireChosen
          ? { retirePhotoId: mapping.retire.photoId }
          : {}),
      },
    ];
  });

import type { BrowserFetch } from "../model/access-session.js";
import type {
  AdmittedWhiteBalance,
  EditorFacts,
  EditorWhiteBalance,
  SaveRefusal,
  SaveRequest,
  WhiteBalanceRange,
} from "../model/photo-editor.js";

const isStringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === "string");

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
/// The facts of one `GET /api/processing/capability` report this workspace
/// uses: the deployment's state, one state per stage, and the adjustable
/// white-balance ranges its approved profiles admit.
export type ProcessingCapability = Readonly<{
  state: string;
  stages: Readonly<{ develop: string; film: string }>;
  /// The approved source classes the report named, each with its admitted
  /// white-balance modes and ranges.
  profiles: ReadonlyArray<Record<string, unknown>>;
}>;

/// The closed reading of one admitted white-balance range. A report whose
/// range this client cannot read leaves the mode disabled: the report is the
/// only source of what an editor may enable.
const readWhiteBalanceRange = (
  value: unknown,
  dimension: "temperature" | "tint",
): WhiteBalanceRange | undefined => {
  if (!isRecord(value)) return undefined;
  const named =
    dimension === "temperature"
      ? (value["temperatureKelvin"] ?? value["temperature"])
      : (value["tintMilli"] ?? value["tint"]);
  const bound = isRecord(named) ? named : value;
  const minimum = bound["minimum"];
  const maximum = bound["maximum"];
  if (
    typeof minimum !== "number" ||
    typeof maximum !== "number" ||
    !Number.isFinite(minimum) ||
    !Number.isFinite(maximum) ||
    minimum > maximum
  )
    return undefined;
  const bounds =
    dimension === "temperature"
      ? { minimum: 1000, maximum: 40000 }
      : { minimum: -150000, maximum: 150000 };
  return Object.freeze({
    minimum: Math.max(bounds.minimum, minimum),
    maximum: Math.min(bounds.maximum, maximum),
  });
};

/// The adjustable modes admitted for the source class whose modes the recipe
/// read reported. The read does not name the Photo's profile, so the modes are
/// enabled only where every candidate profile admits them with a readable
/// range: an editor never enables a control the report may not admit.
const admittedWhiteBalanceFor = (
  profiles: ReadonlyArray<Record<string, unknown>>,
  modes: ReadonlyArray<string>,
): ReadonlyArray<AdmittedWhiteBalance> => {
  const candidates = profiles.filter((profile) => {
    const admitted = profile["whiteBalanceModes"];
    return (
      Array.isArray(admitted) && modes.every((mode) => admitted.includes(mode))
    );
  });
  if (candidates.length === 0) return Object.freeze([]);
  const ranges = candidates.map((profile) => profile["whiteBalanceRanges"]);
  if (ranges.some((value) => !isRecord(value))) return Object.freeze([]);
  const entries = ranges.map(
    (value) => (value as Record<string, unknown>)["temperature-tint"],
  );
  const temperatureKelvin = entries.map((entry) =>
    readWhiteBalanceRange(entry, "temperature"),
  );
  const tintMilli = entries.map((entry) =>
    readWhiteBalanceRange(entry, "tint"),
  );
  if (
    temperatureKelvin.some((range) => range === undefined) ||
    tintMilli.some((range) => range === undefined)
  )
    return Object.freeze([]);
  const temperatures = temperatureKelvin as ReadonlyArray<WhiteBalanceRange>;
  const tints = tintMilli as ReadonlyArray<WhiteBalanceRange>;
  const admitted = Object.freeze({
    mode: "temperature-tint" as const,
    temperatureKelvin: Object.freeze({
      minimum: Math.max(...temperatures.map((range) => range.minimum)),
      maximum: Math.min(...temperatures.map((range) => range.maximum)),
    }),
    tintMilli: Object.freeze({
      minimum: Math.max(...tints.map((range) => range.minimum)),
      maximum: Math.min(...tints.map((range) => range.maximum)),
    }),
  });
  if (
    admitted.temperatureKelvin.minimum > admitted.temperatureKelvin.maximum ||
    admitted.tintMilli.minimum > admitted.tintMilli.maximum
  )
    return Object.freeze([]);
  return Object.freeze([admitted]);
};

export const parseProcessingCapability = (
  value: unknown,
): ProcessingCapability | undefined => {
  if (!isRecord(value)) return undefined;
  const state = value["state"];
  const stages = value["stages"];
  const profiles = value["profiles"];
  if (
    typeof state !== "string" ||
    !isRecord(stages) ||
    typeof stages["develop"] !== "string" ||
    typeof stages["film"] !== "string" ||
    !Array.isArray(profiles)
  )
    return undefined;
  return Object.freeze({
    state,
    stages: Object.freeze({
      develop: stages["develop"],
      film: stages["film"],
    }),
    profiles: Object.freeze(profiles.filter(isRecord)),
  });
};

/// The editable facts of one Photo with the deployment's capability applied:
/// the adjustable white-balance modes are enabled only from the report.
export const withProcessingCapability = (
  facts: EditorFacts,
  capability: ProcessingCapability,
): EditorFacts =>
  Object.freeze({
    ...facts,
    controls: Object.freeze({
      ...facts.controls,
      adjustableWhiteBalance: admittedWhiteBalanceFor(
        capability.profiles,
        facts.controls.whiteBalanceModes,
      ),
    }),
  });

/// The closed reading of one stored white-balance intent. The service reports
/// the mode it can read, including one the capability does not admit; a shape
/// outside the contract is not a recipe this client may present.
const parseStoredWhiteBalance = (
  value: unknown,
): EditorWhiteBalance | undefined => {
  if (!isRecord(value)) return undefined;
  const mode = value["mode"];
  if (mode === "as-shot") return Object.freeze({ mode: "as-shot" });
  if (mode !== "temperature-tint") return undefined;
  const temperatureKelvin = value["temperatureKelvin"];
  const tintMilli = value["tintMilli"];
  if (
    typeof temperatureKelvin !== "number" ||
    typeof tintMilli !== "number" ||
    !Number.isInteger(temperatureKelvin) ||
    !Number.isInteger(tintMilli)
  )
    return undefined;
  return Object.freeze({
    mode: "temperature-tint",
    temperatureKelvin,
    tintMilli,
  });
};

/// The closed reading of one `GET /api/photos/{id}/edit-recipe` response. The
/// facts are the model's input, so an incomplete read is a refusal rather than
/// a partially believed recipe.
const parseEditFacts = (
  value: unknown,
  photoId: string,
): EditorFacts | undefined => {
  if (!isRecord(value)) return undefined;
  const sourceSupport = value["sourceSupport"];
  const supportReason = value["supportReason"];
  const sourceRevision = value["sourceRevision"];
  const recipe = value["recipe"];
  const processingAvailable = value["processingAvailable"];
  const controls = value["controls"];
  if (
    (sourceSupport !== "supported" &&
      sourceSupport !== "unsupported" &&
      sourceSupport !== "unavailable") ||
    (sourceRevision !== null && typeof sourceRevision !== "string") ||
    typeof processingAvailable !== "boolean" ||
    !isRecord(controls) ||
    (supportReason !== null &&
      supportReason !== "original-missing" &&
      supportReason !== "original-unreadable")
  )
    return undefined;
  const exposure = controls["exposure"];
  const modes = controls["whiteBalanceModes"];
  if (!isRecord(exposure) || !isStringArray(modes)) return undefined;
  const minimumEv = exposure["minimumEv"];
  const maximumEv = exposure["maximumEv"];
  const stepEv = exposure["stepEv"];
  if (
    typeof minimumEv !== "number" ||
    typeof maximumEv !== "number" ||
    typeof stepEv !== "number" ||
    !Number.isFinite(minimumEv) ||
    !Number.isFinite(maximumEv) ||
    !Number.isFinite(stepEv) ||
    stepEv <= 0 ||
    maximumEv < minimumEv
  )
    return undefined;
  let recipeVersion: string | null = null;
  let exposureEv = minimumEv;
  let whiteBalance: EditorWhiteBalance = Object.freeze({ mode: "as-shot" });
  if (recipe !== null) {
    if (!isRecord(recipe)) return undefined;
    const version = recipe["recipeVersion"];
    const storedExposure = recipe["exposureEv"];
    const stored = parseStoredWhiteBalance(recipe["whiteBalance"]);
    if (
      typeof version !== "string" ||
      typeof storedExposure !== "number" ||
      !Number.isFinite(storedExposure) ||
      stored === undefined
    )
      return undefined;
    recipeVersion = version;
    exposureEv = Math.min(maximumEv, Math.max(minimumEv, storedExposure));
    whiteBalance = stored;
  }
  if ((sourceSupport === "unavailable") !== (sourceRevision === null))
    return undefined;
  return Object.freeze({
    photoId,
    sourceRevision,
    recipeVersion,
    settings: Object.freeze({ exposureEv, whiteBalance }),
    sourceSupport,
    supportReason: typeof supportReason === "string" ? supportReason : "",
    processingAvailable,
    controls: Object.freeze({
      minimumEv,
      maximumEv,
      stepEv,
      whiteBalanceModes: Object.freeze([...modes]),
      // The capability report is the only source of admitted adjustable
      // ranges; it is merged into these facts when that report arrives.
      adjustableWhiteBalance: Object.freeze(
        [],
      ) as ReadonlyArray<AdmittedWhiteBalance>,
    }),
  });
};

/// The refusal of one guarded recipe write, with the conflict facts.
const readEditRefusal = async (response: Response): Promise<SaveRefusal> => {
  const refusal: {
    status: number;
    code: string;
    message: string;
    currentRecipeVersion: string | null;
    currentSourceRevision: string | null;
  } = {
    status: response.status,
    code: `HTTP ${response.status}`,
    message: "",
    currentRecipeVersion: null,
    currentSourceRevision: null,
  };
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return Object.freeze(refusal);
  }
  if (!isRecord(body)) return Object.freeze(refusal);
  const error = body["error"];
  if (!isRecord(error)) return Object.freeze(refusal);
  if (typeof error["code"] === "string") refusal.code = error["code"];
  if (typeof error["message"] === "string") refusal.message = error["message"];
  const details = error["details"];
  if (isRecord(details)) {
    if (typeof details["currentRecipeVersion"] === "string")
      refusal.currentRecipeVersion = details["currentRecipeVersion"];
    if (typeof details["currentSourceRevision"] === "string")
      refusal.currentSourceRevision = details["currentSourceRevision"];
  }
  return Object.freeze(refusal);
};

type EditFactsOutcome =
  | Readonly<{ kind: "ok"; facts: EditorFacts }>
  | Readonly<{ kind: "failed"; message: string }>;

export const fetchEditRecipe = async (
  fetcher: BrowserFetch,
  photoId: string,
  signal: AbortSignal,
): Promise<EditFactsOutcome> => {
  let response: Response;
  try {
    response = await fetcher(
      `/api/photos/${encodeURIComponent(photoId)}/edit-recipe`,
      { signal, priority: "high" },
    );
  } catch {
    return {
      kind: "failed",
      message: "Current edit facts did not reach the service. Retry Edit.",
    };
  }
  if (!response.ok) {
    const refusal = await readEditRefusal(response);
    return {
      kind: "failed",
      message:
        refusal.code === "unknown_photo"
          ? "This Photo is no longer in the Library."
          : `Current edit facts could not be read: ${refusal.code}.`,
    };
  }
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    return { kind: "failed", message: "Current edit facts could not be read." };
  }
  const facts = parseEditFacts(value, photoId);
  return facts
    ? { kind: "ok", facts }
    : {
        kind: "failed",
        message: "Current edit facts are outside the supported shape.",
      };
};

type EditWriteOutcome =
  | Readonly<{ kind: "saved"; recipeVersion: string; sourceRevision: string }>
  | Readonly<{ kind: "refused"; refusal: SaveRefusal }>;

export const saveEditRecipe = async (
  fetcher: BrowserFetch,
  request: SaveRequest,
  signal: AbortSignal,
): Promise<EditWriteOutcome> => {
  let response: Response;
  try {
    response = await fetcher(
      `/api/photos/${encodeURIComponent(request.photoId)}/edit-recipe`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        signal,
        body: JSON.stringify({
          requestId: request.id,
          expectedRecipeVersion: request.expectedRecipeVersion,
          expectedSourceRevision: request.expectedSourceRevision,
          settings: {
            exposureEv: request.settings.exposureEv,
            whiteBalance:
              request.settings.whiteBalance.mode === "temperature-tint"
                ? {
                    mode: "temperature-tint",
                    temperatureKelvin:
                      request.settings.whiteBalance.temperatureKelvin,
                    tintMilli: request.settings.whiteBalance.tintMilli,
                  }
                : { mode: "as-shot" },
          },
        }),
      },
    );
  } catch {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: 0,
        code: "transport_lost",
        message: "The save did not reach the service.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  if (!response.ok)
    return { kind: "refused", refusal: await readEditRefusal(response) };
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: response.status,
        code: "outcome_unknown",
        message: "The save outcome could not be read.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  if (!isRecord(body)) {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: response.status,
        code: "outcome_unknown",
        message: "The save outcome is outside the supported shape.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  const outcome = body["outcome"];
  const recipeVersion = body["recipeVersion"];
  const sourceRevision = body["sourceRevision"];
  if (
    (outcome !== "saved" && outcome !== "unchanged") ||
    typeof recipeVersion !== "string" ||
    typeof sourceRevision !== "string"
  ) {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: response.status,
        code: "outcome_unknown",
        message: "The save outcome is outside the supported shape.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  return { kind: "saved", recipeVersion, sourceRevision };
};

/// Rebinds the stored recipe to the currently observed source revision, the
/// one reconciliation the workspace offers when a saved recipe is bound to
/// different content.
export const rebindEditRecipe = async (
  fetcher: BrowserFetch,
  photoId: string,
  expectedRecipeVersion: string,
  newSourceRevision: string,
): Promise<EditWriteOutcome> => {
  let response: Response;
  try {
    response = await fetcher(
      `/api/photos/${encodeURIComponent(photoId)}/edit-recipe/rebind`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          requestId: `web-rebind-${crypto.randomUUID().replaceAll("-", "").slice(0, 24)}`,
          expectedRecipeVersion,
          newSourceRevision,
        }),
      },
    );
  } catch {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: 0,
        code: "transport_lost",
        message: "The rebind did not reach the service.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  if (!response.ok)
    return { kind: "refused", refusal: await readEditRefusal(response) };
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: response.status,
        code: "outcome_unknown",
        message: "The rebind outcome could not be read.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  if (!isRecord(body)) {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: response.status,
        code: "outcome_unknown",
        message: "The rebind outcome is outside the supported shape.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  const recipeVersion = body["recipeVersion"];
  const sourceRevision = body["sourceRevision"];
  if (
    (body["outcome"] !== "saved" && body["outcome"] !== "unchanged") ||
    typeof recipeVersion !== "string" ||
    typeof sourceRevision !== "string"
  ) {
    return {
      kind: "refused",
      refusal: Object.freeze({
        status: response.status,
        code: "outcome_unknown",
        message: "The rebind outcome is outside the supported shape.",
        currentRecipeVersion: null,
        currentSourceRevision: null,
      }),
    };
  }
  return { kind: "saved", recipeVersion, sourceRevision };
};

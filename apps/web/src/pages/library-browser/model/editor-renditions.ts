import {
  BASELINE_SETTINGS,
  CURRENT_SETTINGS,
  comparisonIsCurrent,
  comparisonRefusal,
  currentRenditionRefusal,
  editPreviewUri,
  type Comparison,
  type CurrentRendition,
  type EditPreviewSettings,
} from "./edit-preview.js";
import { isRecord } from "../api/editor.js";
import type { BrowserFetch } from "./access-session.js";

const POLL_MS = 750;
const POLL_LIMIT = 400;
type Outcome = "pending" | "ready" | "failed" | "unknown";
type Lane = {
  url: string | undefined;
  note: string;
  busy: boolean;
  pending: boolean;
  outcome: Outcome;
  stale: boolean;
  refused: boolean;
  retained: Comparison | undefined;
  requestKey: string | undefined;
  generation: number;
  abort: AbortController | undefined;
  attempts: number;
  timer: ReturnType<typeof setTimeout> | undefined;
};
const emptyLane = (): Lane => ({
  url: undefined,
  note: "",
  busy: false,
  pending: false,
  outcome: "pending",
  stale: false,
  refused: false,
  retained: undefined,
  requestKey: undefined,
  generation: 0,
  abort: undefined,
  attempts: 0,
  timer: undefined,
});

export type EditorRenditions = Readonly<{
  current: Readonly<
    Pick<
      Lane,
      "url" | "note" | "busy" | "pending" | "outcome" | "stale" | "refused"
    >
  >;
  comparison: Readonly<Pick<Lane, "url" | "note" | "busy" | "retained">>;
  requestCurrent: (photoId: string) => Promise<void>;
  requestComparison: (photoId: string) => Promise<void>;
  markStale: () => void;
  retainComparison: (expected: Comparison) => void;
  clearComparison: () => void;
  clear: () => void;
}>;

/// Current and baseline renditions have independent admissions and retained
/// images. Only the current rendition follows saved settings and edit-source
/// identity; the baseline survives settings changes for the same development.
export function createEditorRenditions(
  dependencies: Readonly<{
    fetcher: BrowserFetch;
    capture: (photoId: string) => CurrentRendition | undefined;
    ownsPhoto: (photoId: string) => boolean;
    describeRefusal: (response: Response) => Promise<string>;
    changed: () => void;
    present: (settings: EditPreviewSettings, url: string) => void;
  }>,
): EditorRenditions {
  const { fetcher, capture, ownsPhoto, describeRefusal, changed, present } =
    dependencies;
  const current = emptyLane();
  const comparison = emptyLane();
  const clear = (lane: Lane): void => {
    lane.abort?.abort();
    lane.abort = undefined;
    lane.generation += 1;
    clearTimeout(lane.timer);
    lane.timer = undefined;
    if (lane.url) URL.revokeObjectURL(lane.url);
    lane.url = undefined;
    lane.note = "";
    lane.busy = false;
    lane.pending = false;
    lane.outcome = "pending";
    lane.stale = false;
    lane.refused = false;
    lane.retained = undefined;
    lane.requestKey = undefined;
    lane.attempts = 0;
  };
  const request = async (
    photoId: string,
    settings: EditPreviewSettings,
    followUp = false,
  ): Promise<void> => {
    const expected = capture(photoId);
    if (!expected || !ownsPhoto(photoId)) return;
    const baseline = settings === BASELINE_SETTINGS;
    const lane = baseline ? comparison : current;
    const identity = baseline
      ? undefined
      : [
          photoId,
          expected.stage,
          expected.sourceRevision ?? "",
          expected.recipeVersion,
          expected.editSource,
          expected.editSourceProxyId ?? "",
        ].join("|");
    if (!baseline && lane.busy && lane.requestKey === identity) return;
    if (!followUp) lane.attempts = 0;
    clearTimeout(lane.timer);
    lane.timer = undefined;
    lane.pending = false;
    const generation = ++lane.generation;
    lane.abort?.abort();
    const controller = new AbortController();
    lane.abort = controller;
    lane.requestKey = identity;
    lane.busy = true;
    lane.outcome = "pending";
    lane.refused = false;
    if (!baseline && lane.url) lane.stale = true;
    lane.note = baseline
      ? "Preparing the comparison…"
      : lane.url
        ? "This preview is older than the current settings."
        : "Updating preview…";
    changed();
    const owns = (): boolean =>
      generation === lane.generation && ownsPhoto(photoId);
    const fail = (note: string): void => {
      if (!owns()) return;
      lane.busy = false;
      lane.note = note;
      lane.outcome = "failed";
      lane.refused = !lane.url;
      changed();
    };
    let response: Response;
    try {
      response = await fetcher(
        editPreviewUri(photoId, expected.stage, settings),
        {
          signal: controller.signal,
          priority: "high",
        },
      );
    } catch {
      fail(
        baseline
          ? "The comparison request did not reach the service."
          : "The preview request did not reach the service.",
      );
      return;
    }
    if (!owns()) return;
    if (response.status === 202) {
      const body: unknown = await response.json().catch(() => undefined);
      if (!owns()) return;
      lane.busy = false;
      const running = isRecord(body) && body["state"] === "running";
      lane.note = baseline
        ? running
          ? "Rendering the comparison…"
          : "Preparing the comparison…"
        : running
          ? "Rendering the preview. The image shown is older than the current settings."
          : "Updating preview…";
      if (!baseline && lane.url) lane.stale = true;
      // A full RAW render may take minutes, but an unanswered operation must
      // not keep polling indefinitely. Each lane has its own finite budget.
      if (lane.attempts >= POLL_LIMIT) {
        lane.note = baseline
          ? "The comparison is taking too long. Compare again to check its result."
          : "The preview is taking too long. Refresh the preview to check its result.";
        lane.outcome = "unknown";
      } else {
        lane.attempts += 1;
        lane.pending = true;
        lane.timer = setTimeout(() => {
          lane.timer = undefined;
          lane.pending = false;
          if (owns()) void request(photoId, settings, true);
        }, POLL_MS);
      }
      changed();
      return;
    }
    if (!response.ok) {
      fail(await describeRefusal(response));
      return;
    }
    let image: Blob;
    try {
      image = await response.blob();
    } catch {
      fail(
        baseline
          ? "The comparison could not be read. Compare again."
          : "The preview could not be read. Refresh the preview to try again.",
      );
      return;
    }
    if (!owns()) return;
    const refusal = baseline
      ? comparisonRefusal(response.headers, expected)
      : currentRenditionRefusal(response.headers, image.size, expected);
    if (refusal) {
      fail(refusal);
      return;
    }
    lane.busy = false;
    if (lane.url) URL.revokeObjectURL(lane.url);
    lane.url = URL.createObjectURL(image);
    lane.retained = expected;
    lane.stale = false;
    lane.refused = false;
    lane.outcome = "ready";
    const width = response.headers.get("slipstream-edit-preview-width") ?? "?";
    const height =
      response.headers.get("slipstream-edit-preview-height") ?? "?";
    const transform =
      response.headers.get("slipstream-edit-preview-display-transform") ?? "";
    const provenance =
      expected.editSource === "development-proxy"
        ? ", from the Development Proxy edit source"
        : "";
    lane.note = baseline
      ? `Comparison ${width}×${height}: the unadjusted rendering${expected.stage === "film" ? " with the film look" : ""}. The current settings are unchanged.`
      : `${expected.stage === "film" ? "Film" : "Edit"} preview ${width}×${height} at the current settings${provenance}${transform ? `, display transform ${transform}` : ""}.`;
    present(settings, lane.url);
    changed();
  };
  return {
    current,
    comparison,
    requestCurrent: (photoId: string) => request(photoId, CURRENT_SETTINGS),
    requestComparison: (photoId: string) => request(photoId, BASELINE_SETTINGS),
    markStale: (): void => {
      if (current.url) current.stale = true;
    },
    retainComparison: (expected: Comparison): void => {
      if (
        comparison.retained &&
        !comparisonIsCurrent(comparison.retained, expected)
      )
        clear(comparison);
    },
    clearComparison: (): void => clear(comparison),
    clear: (): void => {
      clear(current);
      clear(comparison);
    },
  };
}

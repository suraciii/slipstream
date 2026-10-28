/// View options' fact-condition controls: the staged Rating, Original kind,
/// Original availability, and Capture Time range controls beside Selection
/// State.
///
/// This module owns the draft state for the defined Photo facts and the
/// header text that names every active non-default condition. It holds no
/// DOM owner of its own beyond the controls the View options dialog renders,
/// so the dialog keeps one Apply action that commits order, Selection State,
/// and the fact conditions together. Clearing returns to the source's default
/// filter without changing the source.
import type { ViewFilterConditions } from "./library-browser-view-contract.js";

export type ViewOptionsFilterElements = Readonly<{
  ratingMinimum: HTMLSelectElement;
  ratingMaximum: HTMLSelectElement;
  kindSelect: HTMLSelectElement;
  availableSelect: HTMLSelectElement;
  capturedFrom: HTMLInputElement;
  capturedBefore: HTMLInputElement;
}>;

/// Rating bounds are inclusive integers from 0 through 5. Rating 0 means No
/// Rating, so it is a real option rather than a missing value.
const RATING_OPTIONS: ReadonlyArray<
  Readonly<{ value: string; label: string }>
> = [
  { value: "", label: "No minimum" },
  { value: "0", label: "0 · No Rating" },
  { value: "1", label: "1 star" },
  { value: "2", label: "2 stars" },
  { value: "3", label: "3 stars" },
  { value: "4", label: "4 stars" },
  { value: "5", label: "5 stars" },
];

const MAXIMUM_RATING_OPTIONS: ReadonlyArray<
  Readonly<{ value: string; label: string }>
> = [
  { value: "", label: "No maximum" },
  { value: "0", label: "0 · No Rating" },
  { value: "1", label: "1 star" },
  { value: "2", label: "2 stars" },
  { value: "3", label: "3 stars" },
  { value: "4", label: "4 stars" },
  { value: "5", label: "5 stars" },
];

const KIND_OPTIONS: ReadonlyArray<Readonly<{ value: string; label: string }>> =
  [
    { value: "", label: "Any kind" },
    { value: "raw", label: "RAW" },
    { value: "jpeg", label: "JPEG" },
  ];

const AVAILABILITY_OPTIONS: ReadonlyArray<
  Readonly<{ value: string; label: string }>
> = [
  { value: "", label: "Any availability" },
  { value: "true", label: "Available" },
  { value: "false", label: "Unavailable" },
];

/// The camera-local Capture Time placeholder, so the control names the exact
/// shape the server validates before it admits a Snapshot.
const CAPTURE_TIME_PLACEHOLDER = "YYYY-MM-DDTHH:MM:SS";

/// The staged fact conditions and the controls that hold them. One owner per
/// draft: the dialog reads it on Apply and discards it on Cancel or close.
export interface ViewOptionsFilters {
  /// The staged conditions.
  readonly draft: ViewFilterConditions;
  /// Replaces the draft with the committed conditions of the open source.
  reset(committed: ViewFilterConditions): void;
  /// Applies the staged conditions as the committed ones.
  commit(): ViewFilterConditions;
  /// True when the staged draft differs from the committed conditions.
  changed(): boolean;
  /// Every active non-default condition named in text, in the order View
  /// options presents them. State and order are named by the dialog itself.
  labels(): ReadonlyArray<string>;
  /// Every active condition of the committed source view, so Apply can tell a
  /// real change from a re-selection of the same conditions.
  committedLabels(): ReadonlyArray<string>;
  /// The accessible name suffix that names the active conditions for the
  /// View options entry and the random-Photo action.
  describe(): string;
}

const fillSelect = (
  select: HTMLSelectElement,
  options: ReadonlyArray<Readonly<{ value: string; label: string }>>,
): void => {
  select.replaceChildren(
    ...options.map((option) => {
      const element = document.createElement("option");
      element.value = option.value;
      element.textContent = option.label;
      return element;
    }),
  );
};

export function createViewOptionsFilters(
  elements: ViewOptionsFilterElements,
): ViewOptionsFilters {
  const { ratingMinimum, ratingMaximum, kindSelect, availableSelect } =
    elements;
  const { capturedFrom, capturedBefore } = elements;
  fillSelect(ratingMinimum, RATING_OPTIONS);
  fillSelect(ratingMaximum, MAXIMUM_RATING_OPTIONS);
  fillSelect(kindSelect, KIND_OPTIONS);
  fillSelect(availableSelect, AVAILABILITY_OPTIONS);
  capturedFrom.placeholder = CAPTURE_TIME_PLACEHOLDER;
  capturedBefore.placeholder = CAPTURE_TIME_PLACEHOLDER;

  let committed: ViewFilterConditions = {};
  let draft: ViewFilterConditions = {};

  const readControls = (): ViewFilterConditions => {
    const minimum = ratingMinimum.value;
    const maximum = ratingMaximum.value;
    const kind = kindSelect.value;
    const available = availableSelect.value;
    const from = capturedFrom.value.trim();
    const before = capturedBefore.value.trim();
    return {
      ...(minimum === "" ? {} : { ratingMinimum: Number(minimum) }),
      ...(maximum === "" ? {} : { ratingMaximum: Number(maximum) }),
      ...(kind === "" ? {} : { kind: kind as "raw" | "jpeg" }),
      ...(available === "" ? {} : { available: available === "true" }),
      ...(from === "" ? {} : { capturedFrom: from }),
      ...(before === "" ? {} : { capturedBefore: before }),
    };
  };

  const writeControls = (conditions: ViewFilterConditions): void => {
    ratingMinimum.value =
      conditions.ratingMinimum === undefined
        ? ""
        : String(conditions.ratingMinimum);
    ratingMaximum.value =
      conditions.ratingMaximum === undefined
        ? ""
        : String(conditions.ratingMaximum);
    kindSelect.value = conditions.kind ?? "";
    availableSelect.value =
      conditions.available === undefined ? "" : String(conditions.available);
    capturedFrom.value = conditions.capturedFrom ?? "";
    capturedBefore.value = conditions.capturedBefore ?? "";
  };

  ratingMinimum.addEventListener("change", () => {
    draft = readControls();
  });
  ratingMaximum.addEventListener("change", () => {
    draft = readControls();
  });
  kindSelect.addEventListener("change", () => {
    draft = readControls();
  });
  availableSelect.addEventListener("change", () => {
    draft = readControls();
  });
  capturedFrom.addEventListener("change", () => {
    draft = readControls();
  });
  capturedBefore.addEventListener("change", () => {
    draft = readControls();
  });

  writeControls(committed);

  const labels = (conditions: ViewFilterConditions): ReadonlyArray<string> => {
    const parts: string[] = [];
    if (
      conditions.ratingMinimum !== undefined &&
      conditions.ratingMaximum !== undefined
    ) {
      parts.push(
        conditions.ratingMinimum === conditions.ratingMaximum
          ? `Rating ${conditions.ratingMinimum}`
          : `Rating ${conditions.ratingMinimum}–${conditions.ratingMaximum}`,
      );
    } else if (conditions.ratingMinimum !== undefined) {
      parts.push(`Rating ${conditions.ratingMinimum} and up`);
    } else if (conditions.ratingMaximum !== undefined) {
      parts.push(`Rating ${conditions.ratingMaximum} and below`);
    }
    if (conditions.kind !== undefined)
      parts.push(conditions.kind === "raw" ? "RAW only" : "JPEG only");
    if (conditions.available !== undefined)
      parts.push(
        conditions.available ? "Available Originals" : "Unavailable Originals",
      );
    if (
      conditions.capturedFrom !== undefined ||
      conditions.capturedBefore !== undefined
    ) {
      const from = conditions.capturedFrom ?? "any time";
      const before = conditions.capturedBefore ?? "any time";
      parts.push(`Capture Time ${from} to ${before}`);
    }
    return parts;
  };

  return {
    get draft() {
      return draft;
    },
    reset(next) {
      committed = next;
      draft = next;
      writeControls(next);
    },
    commit() {
      committed = draft;
      return committed;
    },
    changed() {
      return labels(draft).join(" · ") !== labels(committed).join(" · ");
    },
    committedLabels() {
      return labels(committed);
    },
    labels() {
      return labels(draft);
    },
    describe() {
      const active = labels(draft);
      return active.length === 0 ? "" : ` · ${active.join(" · ")}`;
    },
  };
}

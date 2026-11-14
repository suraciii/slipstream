/// View options' fact-condition controls: the staged Rating, Original kind,
/// Original availability, and Capture Time range controls beside Selection
/// State.
///
/// Capture Time has one source of truth: the exact camera-local bounds. The
/// date controls are a whole-day shortcut; the exact controls remain available
/// for second precision. The module holds no DOM owner beyond the controls the
/// View options dialog renders, so the dialog keeps one Apply action that
/// commits order, Selection State, and the fact conditions together.
import type { ViewFilterConditions } from "./library-browser-view-contract.js";
import {
  sameConditions,
  validCaptureTimeBound,
} from "../model/navigation-address.js";

export type ViewOptionsFilterElements = Readonly<{
  ratingMinimum: HTMLSelectElement;
  ratingMaximum: HTMLSelectElement;
  ratingMatch: HTMLButtonElement;
  ratingCollapse: HTMLButtonElement;
  ratingFields: HTMLElement;
  ratingError: HTMLElement;
  kindSelect: HTMLSelectElement;
  availableSelect: HTMLSelectElement;
  capturedFromDate: HTMLInputElement;
  capturedThroughDate: HTMLInputElement;
  captureExact: HTMLDetailsElement;
  captureError: HTMLElement;
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

/// The camera-local Capture Time placeholder, so the exact control names the
/// shape the server validates before it admits a Snapshot.
const CAPTURE_TIME_PLACEHOLDER = "YYYY-MM-DDTHH:MM:SS";
const DAY_BOUNDARY = "T00:00:00";
const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;

const shiftDate = (date: string, days: number): string => {
  if (!DATE_PATTERN.test(date)) return "";
  const value = new Date(`${date}T00:00:00Z`);
  if (!Number.isFinite(value.getTime())) return "";
  value.setUTCDate(value.getUTCDate() + days);
  return value.toISOString().slice(0, 10);
};

const dayOfBound = (value: string | undefined): string | undefined => {
  if (value === undefined || !value.endsWith(DAY_BOUNDARY)) return undefined;
  const date = value.slice(0, 10);
  return DATE_PATTERN.test(date) ? date : undefined;
};

const exactTime = (value: string): string => value.replace("T", " ");

/// The staged fact conditions and the controls that hold them. One owner per
/// draft: the dialog reads it on Apply and discards it on Cancel or close.
export interface ViewOptionsFilters {
  /// Replaces the draft with the committed conditions of the open source.
  reset(committed: ViewFilterConditions): void;
  /// Updates committed conditions without replacing an open draft.
  setCommitted(conditions: ViewFilterConditions): void;
  /// Replaces the draft only, retaining committed conditions.
  resetDraft(): void;
  /// Reads the current controls, or reports invalid bounds without committing.
  read(): ViewFilterConditions | undefined;
  draftChanged(): boolean;
  committedLabels(): ReadonlyArray<string>;
  /// Whether a committed non-default advanced condition is active, so the
  /// dialog opens the advanced disclosure instead of hiding a value.
  advancedActive(): boolean;
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

const ratingValue = (select: HTMLSelectElement): number | undefined =>
  select.value === "" ? undefined : Number(select.value);

export function createViewOptionsFilters(
  elements: ViewOptionsFilterElements,
  advancedFilters?: HTMLDetailsElement | null,
): ViewOptionsFilters {
  const {
    ratingMinimum,
    ratingMaximum,
    ratingMatch,
    ratingCollapse,
    ratingFields,
    ratingError,
    kindSelect,
    availableSelect,
    capturedFromDate,
    capturedThroughDate,
    captureExact,
    captureError,
    capturedFrom,
    capturedBefore,
  } = elements;
  fillSelect(ratingMinimum, RATING_OPTIONS);
  fillSelect(ratingMaximum, MAXIMUM_RATING_OPTIONS);
  fillSelect(kindSelect, KIND_OPTIONS);
  fillSelect(availableSelect, AVAILABILITY_OPTIONS);
  capturedFrom.placeholder = CAPTURE_TIME_PLACEHOLDER;
  capturedBefore.placeholder = CAPTURE_TIME_PLACEHOLDER;

  let committed: ViewFilterConditions = {};

  /// The Rating bounds sit behind one disclosure, so the closed row names the
  /// active bound instead of hiding a committed value. The label reuses the
  /// committed summary's own vocabulary, so one Rating reads the same whether
  /// it is collapsed here or listed in the applied-filter summary.
  const ratingBoundLabel = (): string => {
    const minimum = committed.ratingMinimum;
    const maximum = committed.ratingMaximum;
    if (minimum !== undefined && maximum !== undefined)
      return minimum === maximum
        ? `Rating ${minimum}`
        : `Rating ${minimum}–${maximum}`;
    if (minimum !== undefined) return `Rating ${minimum} and up`;
    if (maximum !== undefined) return `Rating ${maximum} and below`;
    return "Rating";
  };
  const presentRatingCollapse = () => {
    ratingCollapse.setAttribute("aria-expanded", String(!ratingFields.hidden));
    ratingCollapse.textContent = ratingBoundLabel();
  };
  ratingCollapse.addEventListener("click", () => {
    ratingFields.hidden = !ratingFields.hidden;
    presentRatingCollapse();
  });

  const clearRatingError = () => {
    for (const control of [ratingMinimum, ratingMaximum]) {
      control.setCustomValidity("");
      control.removeAttribute("aria-invalid");
    }
    ratingError.textContent = "";
    ratingError.hidden = true;
  };

  const clearCaptureError = () => {
    for (const control of [
      capturedFromDate,
      capturedThroughDate,
      capturedFrom,
      capturedBefore,
    ]) {
      control.setCustomValidity("");
      control.removeAttribute("aria-invalid");
    }
    captureError.textContent = "";
    captureError.hidden = true;
  };

  const clearValidity = () => {
    clearRatingError();
    clearCaptureError();
  };

  const showError = (
    control: HTMLInputElement | HTMLSelectElement,
    error: HTMLElement,
    message: string,
  ) => {
    control.setCustomValidity(message);
    control.setAttribute("aria-invalid", "true");
    error.textContent = message;
    error.hidden = false;
  };

  const reportInvalid = (
    control: HTMLInputElement | HTMLSelectElement,
    error: HTMLElement,
    message: string,
  ) => {
    if (advancedFilters?.contains(control)) advancedFilters.open = true;
    if (captureExact.contains(control)) captureExact.open = true;
    showError(control, error, message);
    control.focus();
    control.reportValidity();
  };

  const syncRatingOptions = () => {
    const minimum = ratingValue(ratingMinimum);
    const maximum = ratingValue(ratingMaximum);
    for (let index = 0; index < ratingMaximum.options.length; index++) {
      const option = ratingMaximum.options.item(index);
      if (!option) continue;
      option.disabled =
        minimum !== undefined &&
        option.value !== "" &&
        Number(option.value) < minimum;
    }
    for (let index = 0; index < ratingMinimum.options.length; index++) {
      const option = ratingMinimum.options.item(index);
      if (!option) continue;
      option.disabled =
        maximum !== undefined &&
        option.value !== "" &&
        Number(option.value) > maximum;
    }
    ratingMatch.disabled = minimum === undefined || minimum === maximum;
  };

  const syncRatingValidity = () => {
    const minimum = ratingValue(ratingMinimum);
    const maximum = ratingValue(ratingMaximum);
    if (minimum !== undefined && maximum !== undefined && minimum > maximum) {
      showError(
        ratingMaximum,
        ratingError,
        "Maximum must be at least the Minimum.",
      );
      return;
    }
    ratingMaximum.setCustomValidity("");
    ratingMaximum.removeAttribute("aria-invalid");
    ratingError.textContent = "";
    ratingError.hidden = true;
  };

  const syncFromDateFromExact = () => {
    capturedFromDate.value = dayOfBound(capturedFrom.value) ?? "";
  };

  const syncThroughDateFromExact = () => {
    const beforeDay = dayOfBound(capturedBefore.value);
    capturedThroughDate.value = beforeDay ? shiftDate(beforeDay, -1) : "";
  };

  const syncDatesFromExact = () => {
    // A second-precision bound has no date value of its own, but it must not
    // erase the other bound's whole-day shortcut or an in-progress date.
    syncFromDateFromExact();
    syncThroughDateFromExact();
  };

  const syncFromDate = () => {
    capturedFrom.value = capturedFromDate.value
      ? `${capturedFromDate.value}${DAY_BOUNDARY}`
      : "";
  };

  const syncThroughDate = () => {
    capturedBefore.value = capturedThroughDate.value
      ? `${shiftDate(capturedThroughDate.value, 1)}${DAY_BOUNDARY}`
      : "";
  };

  const syncDateValidity = () => {
    const from = capturedFromDate.value;
    const through = capturedThroughDate.value;
    if (from && through && through < from) {
      showError(
        capturedThroughDate,
        captureError,
        "Through date must not be before the From date.",
      );
      return;
    }
    capturedThroughDate.setCustomValidity("");
    capturedThroughDate.removeAttribute("aria-invalid");
    captureError.textContent = "";
    captureError.hidden = true;
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
    const exact =
      capturedFrom.value !== "" && !capturedFrom.value.endsWith(DAY_BOUNDARY);
    const exactBefore =
      capturedBefore.value !== "" &&
      !capturedBefore.value.endsWith(DAY_BOUNDARY);
    captureExact.open = exact || exactBefore;
    syncDatesFromExact();
    clearValidity();
    syncRatingOptions();
    syncRatingValidity();
    // A committed Rating bound opens its disclosure, so View options never
    // hides an active condition behind a collapsed row.
    if (
      committed.ratingMinimum !== undefined ||
      committed.ratingMaximum !== undefined
    )
      ratingFields.hidden = false;
    presentRatingCollapse();
  };

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
    const fromDay = dayOfBound(conditions.capturedFrom);
    const beforeDay = dayOfBound(conditions.capturedBefore);
    const wholeDays =
      (conditions.capturedFrom === undefined || fromDay !== undefined) &&
      (conditions.capturedBefore === undefined || beforeDay !== undefined);
    if (wholeDays) {
      if (fromDay !== undefined && beforeDay !== undefined) {
        const through = shiftDate(beforeDay, -1);
        parts.push(
          fromDay === through
            ? `Capture Time ${fromDay}`
            : `Capture Time ${fromDay}–${through}`,
        );
      } else if (fromDay !== undefined) {
        parts.push(`Capture Time from ${fromDay}`);
      } else if (beforeDay !== undefined) {
        parts.push(`Capture Time before ${beforeDay}`);
      }
    } else {
      const bounds = [
        conditions.capturedFrom === undefined
          ? ""
          : `from ${exactTime(conditions.capturedFrom)}${fromDay === undefined ? " inclusive" : ""}`,
        conditions.capturedBefore === undefined
          ? ""
          : `before ${exactTime(conditions.capturedBefore)}${beforeDay === undefined ? " exclusive" : ""}`,
      ].filter(Boolean);
      parts.push(`Capture Time ${bounds.join(" · ")}`);
    }
    return parts;
  };

  const readControls = (): ViewFilterConditions => {
    const minimum = ratingMinimum.value;
    const maximum = ratingMaximum.value;
    const kind = kindSelect.value;
    const available = availableSelect.value;
    const from = capturedFrom.value;
    const before = capturedBefore.value;
    return {
      ...(minimum === "" ? {} : { ratingMinimum: Number(minimum) }),
      ...(maximum === "" ? {} : { ratingMaximum: Number(maximum) }),
      ...(kind === "" ? {} : { kind: kind as "raw" | "jpeg" }),
      ...(available === "" ? {} : { available: available === "true" }),
      ...(from === "" ? {} : { capturedFrom: from }),
      ...(before === "" ? {} : { capturedBefore: before }),
    };
  };

  ratingMinimum.addEventListener("change", () => {
    clearRatingError();
    syncRatingOptions();
    syncRatingValidity();
  });
  ratingMaximum.addEventListener("change", () => {
    clearRatingError();
    syncRatingOptions();
    syncRatingValidity();
  });
  ratingMatch.addEventListener("click", () => {
    if (ratingMatch.disabled) return;
    ratingMaximum.value = ratingMinimum.value;
    // The select's change handler refreshes the options and validity; the
    // bubbling event also lets View options refresh its dirty-draft notice.
    ratingMaximum.dispatchEvent(new Event("change", { bubbles: true }));
  });
  capturedFromDate.addEventListener("input", () => {
    clearCaptureError();
    if (capturedFromDate.validity.badInput) return;
    syncFromDate();
    syncDateValidity();
  });
  capturedThroughDate.addEventListener("input", () => {
    clearCaptureError();
    if (capturedThroughDate.validity.badInput) return;
    syncThroughDate();
    syncDateValidity();
  });
  capturedFrom.addEventListener("input", () => {
    clearCaptureError();
    syncFromDateFromExact();
  });
  capturedBefore.addEventListener("input", () => {
    clearCaptureError();
    syncThroughDateFromExact();
  });

  return {
    setCommitted(next) {
      committed = next;
      presentRatingCollapse();
    },
    reset(next) {
      committed = next;
      writeControls(next);
    },
    resetDraft() {
      writeControls({});
    },
    read() {
      clearValidity();
      const conditions = readControls();
      if (
        conditions.ratingMinimum !== undefined &&
        conditions.ratingMaximum !== undefined &&
        conditions.ratingMinimum > conditions.ratingMaximum
      ) {
        reportInvalid(
          ratingMaximum,
          ratingError,
          "Maximum must be at least the Minimum.",
        );
        return undefined;
      }
      for (const control of [capturedFromDate, capturedThroughDate]) {
        if (control.validity.badInput) {
          reportInvalid(
            control,
            captureError,
            "Enter a complete date or clear the field.",
          );
          return undefined;
        }
      }
      if (
        capturedFromDate.value &&
        capturedThroughDate.value &&
        capturedThroughDate.value < capturedFromDate.value
      ) {
        reportInvalid(
          capturedThroughDate,
          captureError,
          "Through date must not be before the From date.",
        );
        return undefined;
      }
      for (const [control, value, error, message] of [
        [
          capturedFrom,
          conditions.capturedFrom,
          captureError,
          "Enter a camera-local time as YYYY-MM-DDTHH:MM:SS, without a time zone.",
        ],
        [
          capturedBefore,
          conditions.capturedBefore,
          captureError,
          "Enter a camera-local time as YYYY-MM-DDTHH:MM:SS, without a time zone.",
        ],
      ] as const) {
        if (value !== undefined && !validCaptureTimeBound(value)) {
          reportInvalid(control, error, message);
          return undefined;
        }
      }
      if (
        conditions.capturedFrom !== undefined &&
        conditions.capturedBefore !== undefined &&
        conditions.capturedFrom >= conditions.capturedBefore
      ) {
        reportInvalid(
          capturedBefore,
          captureError,
          "Before must be later than From.",
        );
        return undefined;
      }
      return conditions;
    },
    draftChanged() {
      return (
        capturedFromDate.validity.badInput ||
        capturedThroughDate.validity.badInput ||
        !sameConditions(readControls(), committed)
      );
    },
    committedLabels() {
      return labels(committed);
    },
    advancedActive() {
      return (
        committed.kind !== undefined ||
        committed.available !== undefined ||
        committed.capturedFrom !== undefined ||
        committed.capturedBefore !== undefined
      );
    },
  };
}

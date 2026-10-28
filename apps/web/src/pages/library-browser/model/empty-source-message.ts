/// The message an empty source presents, and whether it may offer a Library
/// check.
///
/// A source with no matches is not an empty Library, and an active filter or
/// order never turns one into the other: the message names the view the
/// Photographer is actually looking at. This is page-local presentation, so it
/// holds no state and issues no HTTP.
import type { BrowseFilterConditions } from "../api/contracts.js";
import type { SelectionFilter } from "../api/contracts.js";

/// The kind of source the open view names.
export type EmptySourceKind = "library" | "album" | "folder";

/// True when the open view applies any restriction at all, so an empty result
/// is a no-match state rather than an empty source.
const restricts = (
  selection: SelectionFilter,
  conditions: BrowseFilterConditions,
): boolean =>
  selection !== "all" ||
  conditions.ratingMinimum !== undefined ||
  conditions.ratingMaximum !== undefined ||
  conditions.kind !== undefined ||
  conditions.available !== undefined ||
  conditions.capturedFrom !== undefined ||
  conditions.capturedBefore !== undefined;

export const emptySourceMessage = (
  kind: EmptySourceKind,
  selection: SelectionFilter,
  conditions: BrowseFilterConditions,
): Readonly<{ message: string; offersLibraryCheck: boolean }> => {
  // A filter that matched nothing never offers a Library check: the Library is
  // available, the view is empty.
  if (restricts(selection, conditions))
    return {
      message: "No Photos match this filter.",
      offersLibraryCheck: false,
    };
  if (kind === "album")
    return {
      message:
        "This Album contains no Photos. Add Photos from another source's Photo View.",
      offersLibraryCheck: false,
    };
  return {
    message:
      "No supported Photos found. Check the Library Folder or add supported files, then run Check Library.",
    offersLibraryCheck: true,
  };
};

/// The wire request one open source sends, and the frozen copy of the source
/// descriptor the owner keeps.
///
/// Extracted from the Grid owner so the owner stays one bounded module: these
/// two shapes describe the same decision — which source, in which order, under
/// which view — and freeze it before the first request. The conditions are
/// frozen with the source, because a filter is a view of one source, not a new
/// source, and a decision never moves a Photo in or out of the open view.
import type {
  BrowseFilterConditions,
  SelectionFilter,
} from "../api/contracts.js";
import type { BrowseSourceRequest } from "../api/source-grid.js";

export type SourceGridSource =
  | Readonly<{ kind: "library" }>
  | Readonly<{
      kind: "album";
      album: Readonly<{ id: string; name: string }>;
    }>
  | Readonly<{
      kind: "folder";
      folder: Readonly<{ location: string; name: string }>;
      publication: string;
    }>;

export type SourceViewOrder =
  | "source-default"
  | "capture-time-asc"
  | "capture-time-desc";

/// The request one source, order, Selection State, and fact-condition set
/// produces. An omitted preferred Photo or resume is omitted from the wire, so
/// an unset option is never sent as an empty value.
export const sourceRequest = (
  source: SourceGridSource,
  order: SourceViewOrder,
  selection: SelectionFilter,
  conditions: BrowseFilterConditions,
  preferredPhotoId?: string,
  resume = false,
): BrowseSourceRequest =>
  source.kind === "library"
    ? {
        kind: "library",
        order,
        selection,
        conditions,
        ...(preferredPhotoId ? { preferredPhotoId } : {}),
        ...(resume ? { resume: true } : {}),
      }
    : source.kind === "album"
      ? {
          kind: "album",
          albumId: source.album.id,
          order,
          selection,
          conditions,
          ...(preferredPhotoId ? { preferredPhotoId } : {}),
          ...(resume ? { resume: true } : {}),
        }
      : {
          kind: "folder",
          folderPath: source.folder.location,
          publication: source.publication,
          order,
          selection,
          conditions,
          ...(preferredPhotoId ? { preferredPhotoId } : {}),
          ...(resume ? { resume: true } : {}),
        };

/// A deep-frozen source descriptor, so a background rescan can never move a
/// Photo underneath an already open source.
export const freezeSource = (source: SourceGridSource): SourceGridSource =>
  source.kind === "library"
    ? Object.freeze({ kind: "library" })
    : source.kind === "album"
      ? Object.freeze({
          kind: "album",
          album: Object.freeze({ ...source.album }),
        })
      : Object.freeze({
          kind: "folder",
          folder: Object.freeze({ ...source.folder }),
          publication: source.publication,
        });

import type {
  SourceGridOwner,
  SourceGridSource,
  SourceAuthority,
} from "./source-grid-owner.js";
import type {
  FileLocationAuthority,
  FileLocationOwner,
} from "./file-location-owner.js";
import type { SourceViewOrder } from "../api/source-grid.js";
import type { SelectionFilter } from "../api/contracts.js";

type SourceLifecycleOutcome =
  | Readonly<{ kind: "opened"; position: number }>
  | Readonly<{ kind: "missing" }>
  | Readonly<{ kind: "superseded" }>
  | Readonly<{ kind: "failed" }>;

type SourceOpenOptions = Readonly<{
  preferredPhotoId?: string;
  mode?: "replace" | "reopen";
  order?: SourceViewOrder;
  selection?: SelectionFilter;
}>;

export interface SourceLifecycleOwner {
  beginOpen(
    source: SourceGridSource,
    options?: SourceOpenOptions,
  ): Readonly<{
    authority: SourceAuthority;
    generation: number;
    outcome: Promise<SourceLifecycleOutcome>;
  }>;
}

export function createSourceOpenOwner({
  sourceGrid,
  fileLocations,
  rebindFileLocations,
  onPublicationConflict,
}: Readonly<{
  sourceGrid: SourceGridOwner;
  fileLocations: Pick<FileLocationOwner, "publication" | "isCurrent">;
  rebindFileLocations: () => Promise<FileLocationAuthority>;
  onPublicationConflict: (publication: string) => void;
}>): SourceLifecycleOwner {
  return {
    beginOpen(source, options = {}) {
      const publication = fileLocations.publication;
      const descriptor =
        source.kind === "folder" && publication
          ? { ...source, publication }
          : source;
      const pending = sourceGrid.open(descriptor, options);
      const authority = sourceGrid.authority;
      const generation = sourceGrid.generation;
      const outcome = pending.then(
        async (opened): Promise<SourceLifecycleOutcome> => {
          if (!sourceGrid.isCurrent(authority) || opened.kind === "detached")
            return { kind: "superseded" };
          if (opened.kind === "publication-conflict") {
            const rebound = await rebindFileLocations();
            if (!sourceGrid.isCurrent(authority)) return { kind: "superseded" };
            const reboundPublication = fileLocations.publication;
            if (fileLocations.isCurrent(rebound) && reboundPublication)
              onPublicationConflict(reboundPublication);
            return { kind: "failed" };
          }
          if (opened.kind === "failed")
            return { kind: opened.status === 404 ? "missing" : "failed" };
          const position = sourceGrid.readGridPosition(authority);
          return position === undefined
            ? { kind: "superseded" }
            : { kind: "opened", position };
        },
      );
      return { authority, generation, outcome };
    },
  };
}

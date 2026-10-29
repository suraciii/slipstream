import type { ProcessingModuleDescription } from "../api/processing-modules.js";
import type {
  EditView,
  JsonObject,
  ToolJob,
  Workspace,
  WorkspaceApiError,
  WorkspaceFile,
} from "../api/workspace-edit.js";
import type { UncertainRequest } from "./uncertain-request.js";

export type EditChange = JsonObject & { type: string };
export type WorkspaceEditModel = {
  photoId: string;
  workspace?: Workspace;
  files: WorkspaceFile[];
  edit?: EditView;
  modules: JsonObject[];
  descriptions: Record<string, JsonObject>;
  instanceDetails: Record<string, JsonObject>;
  maskDetails: Record<string, JsonObject>;
  resourceKinds: JsonObject[];
  catalogs: Record<string, JsonObject>;
  selectedInput?: string;
  inspectedFile?: WorkspaceFile;
  draft: EditChange[];
  saving: boolean;
  loading: boolean;
  status: string;
  error?: WorkspaceApiError;
  blocked: boolean;
  preview?: {
    url: string;
    revision: string;
    file: WorkspaceFile;
    comparisonUrl?: string;
    identity: string;
  };
  previewPending: boolean;
  comparing: boolean;
  cameraReference: boolean;
  resultSelected?: boolean;
  cameraReferenceUrl?: string;
  toolDiscovery?: {
    input: string;
    revision: string;
    state: "loading" | "unknown" | "ready" | "unavailable";
    reasons: readonly string[];
  };
  jobs: ToolJob[];
  history: JsonObject[];
  /** The tool's retained revisions, restored as their own operation. */
  retainedRevisions: JsonObject[];
  /** Completed session edit actions available to session Undo. */
  sessionUndoCount: number;
  /** Reversed session actions available to session Redo. */
  sessionRedoCount: number;
  /** The Library's own progress, disclosed separately from the Original. */
  libraryPhase: string;
  fileCursor: string | null;
  uncertain?: UncertainRequest;
  filmElement?: HTMLElement;
  catalogElement?: HTMLElement;
  maskGraphSchema?: JsonObject;
  filmModules?: readonly ProcessingModuleDescription[];
  conflictInspected?: boolean;
};

export function createWorkspaceEditModel(photoId: string): WorkspaceEditModel {
  return {
    photoId,
    files: [],
    modules: [],
    descriptions: {},
    instanceDetails: {},
    maskDetails: {},
    resourceKinds: [],
    catalogs: {},
    draft: [],
    saving: false,
    loading: false,
    status: "Preparing input",
    blocked: false,
    previewPending: false,
    comparing: false,
    cameraReference: false,
    jobs: [],
    history: [],
    retainedRevisions: [],
    sessionUndoCount: 0,
    sessionRedoCount: 0,
    libraryPhase: "",
    fileCursor: null,
  };
}

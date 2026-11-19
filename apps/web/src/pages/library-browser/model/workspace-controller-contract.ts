import type {
  JsonObject,
  Workspace,
  WorkspaceApiError,
  WorkspaceFile,
} from "../api/workspace.js";

export type WorkspaceViewModel = {
  photoId: string;
  workspace?: Workspace;
  files: WorkspaceFile[];
  fileTotal: number;
  fileCursor: string | null;
  inspectedFile?: WorkspaceFile;
  loading: boolean;
  inspectingArtifactId?: string;
  removingArtifactId?: string;
  downloadingArtifactId?: string;
  download?: Readonly<{
    artifactId: string;
    url: string;
  }>;
  status: string;
  notice?: string;
  error?: WorkspaceApiError;
};

export type WorkspaceViewPort = Readonly<{
  workspaceVisible: () => boolean;
  renderWorkspace: (model: WorkspaceViewModel) => void;
}>;

export type WorkspaceControllerDependencies = Readonly<{
  isAlive: () => boolean;
  isCurrentPhoto: (photoId: string) => boolean;
}>;

export type WorkspaceController = Readonly<{
  open: (photoId: string) => void;
  refresh: (photoId: string) => void;
  action: (photoId: string, action: string, args?: JsonObject) => Promise<void>;
  leave: () => void;
  dispose: () => void;
}>;

import type { BrowserFetch } from "./access-session.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
import type {
  EditorController,
  EditorControllerDependencies,
} from "./editor-controller-contract.js";
import {
  createWorkspaceEditOwner,
  type WorkspaceEditOwner,
} from "./workspace-edit-owner.js";
import {
  createWorkspaceFilmSurface,
  type WorkspaceFilmSurface,
} from "../ui/workspace-film-surface.js";
import { isRecord } from "../api/guards.js";
import { workspacePath, workspaceRequest } from "../api/workspace-edit.js";
import {
  createWorkspaceCatalogSurface,
  type WorkspaceCatalogSurface,
} from "../ui/workspace-catalog-surface.js";
import { fetchWorkspaceFilmModules } from "../api/workspace-film.js";
export type { EditorController } from "./editor-controller-contract.js";
export function createEditorController(
  fetcher: BrowserFetch,
  view: LibraryBrowserView,
  dependencies: EditorControllerDependencies,
): EditorController {
  const owners = new Map<string, WorkspaceEditOwner>();
  const filmSurfaces = new Map<string, WorkspaceFilmSurface>();
  const catalogSurfaces = new Map<string, WorkspaceCatalogSurface>();
  let selected: string | undefined;
  const owns = (photoId: string) =>
    selected === photoId &&
    dependencies.isAlive() &&
    dependencies.isCurrentPhoto(photoId) &&
    view.editorVisible();
  const open = (photoId: string) => {
    if (!dependencies.isAlive() || !dependencies.isCurrentPhoto(photoId))
      return;
    selected = photoId;
    let owner = owners.get(photoId);
    if (!owner) {
      owner = createWorkspaceEditOwner(fetcher, photoId, (model) => {
        if (!owns(photoId)) return;
        if (model.edit) {
          const details = Object.fromEntries(
            model.edit.instances.items.map((instance) => [
              instance.instanceId,
              {
                ...model.instanceDetails[instance.instanceId],
                descriptor: model.descriptions[instance.moduleId],
              },
            ]),
          );
          catalogSurfaces
            .get(photoId)
            ?.bind(
              model.edit,
              model.edit.instances.items,
              details,
              model.resourceKinds,
            );
        }
        view.renderEditor(model);
        if (model.preview && !model.cameraReference)
          view.presentEditorPreview(
            model.comparing && model.preview.comparisonUrl
              ? model.preview.comparisonUrl
              : model.preview.url,
          );
      });
      owners.set(photoId, owner);
      if (typeof document !== "undefined") {
        const film = createWorkspaceFilmSurface(fetcher, photoId, () => {
          void owners.get(photoId)?.refresh();
        });
        owner.model.filmElement = film.element;
        filmSurfaces.set(photoId, film);
        const catalog = createWorkspaceCatalogSurface(
          fetcher,
          photoId,
          async () => {
            const current = owners.get(photoId);
            await current?.refresh();
            await current?.act("preview");
          },
          async () => (await owners.get(photoId)?.settle()) ?? false,
        );
        owner.model.catalogElement = catalog.element;
        catalogSurfaces.set(photoId, catalog);
        void fetchWorkspaceFilmModules(
          fetcher,
          new AbortController().signal,
        ).then((modules) => {
          if (modules) owner!.model.filmModules = modules;
          if (owns(photoId)) view.renderEditor(owner!.model);
        });
      }
    }
    view.renderEditor(owner.model);
    void owner.refresh().then(async () => {
      if (typeof window === "undefined" || !owns(photoId)) return;
      const addressed = new URLSearchParams(window.location.search).get(
        "editId",
      );
      if (
        addressed &&
        owner.model.edit?.editId !== addressed &&
        owner.model.workspace?.edits.some((edit) => edit.editId === addressed)
      )
        await owner.act("edit-select", { editId: addressed });
    });
  };
  return {
    open,
    refresh(photoId) {
      if (owns(photoId)) void owners.get(photoId)?.refresh();
    },
    act(photoId, action, args) {
      if (!owns(photoId)) return;
      const owner = owners.get(photoId);
      if (action === "continue-film" && owner?.model.workspace) {
        const workspaceId = owner.model.workspace.workspaceId;
        const file = owner.model.files.find(
          (item) =>
            item.artifactId === args.artifactId &&
            item.revision === args.revision,
        );
        if (
          !file ||
          file.kind !== "output" ||
          file.state !== "ready" ||
          !file.revision
        )
          return;
        const revision = file.revision;
        void workspaceRequest(
          fetcher,
          `${workspacePath(workspaceId)}/files/${encodeURIComponent(file.artifactId)}`,
        )
          .then(async (inspected) => {
            if (
              !owns(photoId) ||
              inspected.revision !== file.revision ||
              !isRecord(inspected.imageContract)
            )
              return;
            await filmSurfaces.get(photoId)?.open({
              workspaceId,
              artifactId: file.artifactId,
              revision,
              imageContract: inspected.imageContract,
              relativePath: file.relativePath,
            });
          })
          .catch((error) => {
            owner.model.status = `Continue processing failed: ${String(error instanceof Error ? error.message : error)}`;
            if (owns(photoId)) view.renderEditor(owner.model);
          });
        return;
      }
      void owner?.act(action, args);
    },
    leave() {
      selected = undefined;
      view.clearEditorPreview();
      if (!dependencies.isAlive()) {
        for (const owner of owners.values()) owner.dispose();
        for (const film of filmSurfaces.values()) film.dispose();
        for (const catalog of catalogSurfaces.values()) catalog.dispose();
        owners.clear();
        filmSurfaces.clear();
        catalogSurfaces.clear();
      }
    },
  };
}

import {
  createDevelopmentProxy,
  fetchDevelopmentProxy,
  removeDevelopmentProxy,
  type DevelopmentProxyStatus,
} from "../api/development-proxy.js";
import { formatByteCount } from "./editor-presentation.js";
import type { BrowserFetch } from "./access-session.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
export function createEditorProxyController(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    sourceRevision: (photoId: string) => string | null | undefined;
    editorOwnsPhoto: (photoId: string) => boolean;
    renderEditor: () => void;
    refreshSource: (photoId: string) => void;
  }>,
) {
  const { sourceRevision, editorOwnsPhoto, renderEditor, refreshSource } =
    dependencies;
  let editorProxy: DevelopmentProxyStatus = {
    photoId: "",
    state: "absent",
    proxy: null,
    failure: null,
  };
  let editorProxyFailure = "";
  let editorProxyTimer: number | undefined;
  let editorProxyGeneration = 0;
  const readProxy = async (photoId: string): Promise<void> => {
    const generation = ++editorProxyGeneration;
    const result = await fetchDevelopmentProxy(fetcher, photoId);
    if (generation !== editorProxyGeneration || !editorOwnsPhoto(photoId))
      return;
    if (result.kind === "failed") {
      editorProxyFailure = result.message;
      renderEditor();
      return;
    }
    editorProxyFailure = "";
    editorProxy = result.status;
    renderEditor();
    if (result.status.state === "building") {
      if (editorProxyTimer !== undefined) clearTimeout(editorProxyTimer);
      editorProxyTimer = window.setTimeout(() => {
        editorProxyTimer = undefined;
        void readProxy(photoId);
      }, 500);
    } else if (result.status.state === "current") {
      refreshSource(photoId);
    }
  };
  const createProxy = async (photoId: string): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const revision = sourceRevision(photoId);
    if (!revision) return;
    const generation = ++editorProxyGeneration;
    editorProxyFailure = "";
    editorProxy = { photoId, state: "building", proxy: null, failure: null };
    renderEditor();
    const result = await createDevelopmentProxy(fetcher, photoId, revision);
    if (generation !== editorProxyGeneration || !editorOwnsPhoto(photoId))
      return;
    if (result.kind === "failed") editorProxyFailure = result.message;
    else editorProxy = result.status;
    renderEditor();
    void readProxy(photoId);
  };
  const removeProxy = async (photoId: string): Promise<void> => {
    if (!editorOwnsPhoto(photoId)) return;
    const generation = ++editorProxyGeneration;
    const result = await removeDevelopmentProxy(fetcher, photoId);
    if (generation !== editorProxyGeneration || !editorOwnsPhoto(photoId))
      return;
    if (result.kind === "failed") editorProxyFailure = result.message;
    else editorProxy = result.status;
    renderEditor();
    if (result.kind === "ok") {
      refreshSource(photoId);
    }
  };
  return {
    read: readProxy,
    create: createProxy,
    remove: removeProxy,
    leave: (): void => {
      editorProxyGeneration += 1;
      editorProxy = {
        photoId: "",
        state: "absent",
        proxy: null,
        failure: null,
      };
      editorProxyFailure = "";
      clearTimeout(editorProxyTimer);
      editorProxyTimer = undefined;
    },
    view: (
      photoId: string,
    ): NonNullable<
      Parameters<LibraryBrowserView["renderEditor"]>[0]["proxy"]
    > => {
      const revision = sourceRevision(photoId);
      return {
        state: editorProxy.state,
        note:
          editorProxyFailure ||
          (editorProxy.state === "building"
            ? "Building Development Proxy…"
            : editorProxy.state === "current"
              ? `Current proxy ${editorProxy.proxy?.width ?? "?"}×${editorProxy.proxy?.height ?? "?"}, ${formatByteCount(editorProxy.proxy?.byteLength ?? 0)}.`
              : editorProxy.state === "stale"
                ? "Stale Development Proxy; rebuild it for this Original."
                : "No Development Proxy."),
        proxy: editorProxy.proxy
          ? {
              width: editorProxy.proxy.width,
              height: editorProxy.proxy.height,
              longEdge: editorProxy.proxy.longEdge,
              qualityLimit: editorProxy.proxy.qualityLimit,
              byteLength: editorProxy.proxy.byteLength,
              sourceRevision: editorProxy.proxy.sourceRevision,
              sourceProfileId: editorProxy.proxy.sourceProfileId,
              pipelineVersion: editorProxy.proxy.pipelineVersion,
            }
          : null,
        canCreate: Boolean(revision) && editorProxy.state !== "building",
        canRemove: editorProxy.state === "current",
      };
    },
  };
}

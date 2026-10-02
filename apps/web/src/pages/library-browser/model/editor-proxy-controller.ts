import {
  createDevelopmentProxy,
  fetchDevelopmentProxy,
  removeDevelopmentProxy,
  type DevelopmentProxyStatus,
} from "../api/development-proxy.js";
import { formatByteCount } from "./editor-presentation.js";
import type { BrowserFetch } from "./access-session.js";
import type { PhotoEditor } from "./photo-editor.js";
import type { LibraryBrowserView } from "../ui/library-browser-view.js";
export function createEditorProxyController(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    session: (photoId: string) => PhotoEditor | undefined;
    editorOwnsPhoto: (photoId: string) => boolean;
    renderEditor: () => void;
    refreshSource: (photoId: string) => void;
  }>,
) {
  const { session, editorOwnsPhoto, renderEditor, refreshSource } =
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
    const editor = session(photoId);
    const sourceRevision = editor?.facts()?.sourceRevision;
    if (!editor || !sourceRevision) return;
    editorProxyFailure = "";
    editorProxy = { photoId, state: "building", proxy: null, failure: null };
    renderEditor();
    const result = await createDevelopmentProxy(
      fetcher,
      photoId,
      sourceRevision,
    );
    if (!editorOwnsPhoto(photoId)) return;
    if (result.kind === "failed") editorProxyFailure = result.message;
    else editorProxy = result.status;
    renderEditor();
    void readProxy(photoId);
  };
  const removeProxy = async (photoId: string): Promise<void> => {
    const result = await removeDevelopmentProxy(fetcher, photoId);
    if (!editorOwnsPhoto(photoId)) return;
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
      clearTimeout(editorProxyTimer);
      editorProxyTimer = undefined;
    },
    view: (
      photoId: string,
    ): NonNullable<
      Parameters<LibraryBrowserView["renderEditor"]>[0]["proxy"]
    > => {
      const editor = session(photoId);
      if (!editor)
        throw new Error("Proxy presentation requires an editing session.");
      const presented = editor.presentation();
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
        canCreate:
          presented.editSourceReadiness === "ready" &&
          Boolean(editor.facts()?.sourceRevision) &&
          editorProxy.state !== "building" &&
          presented.editSourceKind !== "development-proxy",
        canRemove: editorProxy.state === "current",
      };
    },
  };
}

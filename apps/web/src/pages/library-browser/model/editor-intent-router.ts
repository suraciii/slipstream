import type { EditorController } from "./editor-controller-contract.js";
import type { LibraryBrowserIntent } from "../ui/library-browser-view.js";
export function routeEditorIntent(
  editor: EditorController,
  intent: LibraryBrowserIntent,
): boolean {
  switch (intent.kind) {
    case "editor-open":
      editor.open(intent.photoId);
      return true;
    case "editor-refresh":
      editor.refresh(intent.photoId);
      return true;
    case "editor-workspace-action":
      editor.act(intent.photoId, intent.action, intent.arguments);
      return true;
    default:
      return false;
  }
}

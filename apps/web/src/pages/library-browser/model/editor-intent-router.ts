import type { EditorController } from "./editor-controller-contract.js";
import type { EditorIntent } from "./editor-intent-contract.js";
export function routeEditorIntent(
  editor: EditorController,
  intent: Readonly<{ kind: string }>,
): boolean {
  const editorIntent = intent as EditorIntent;
  switch (editorIntent.kind) {
    case "editor-open":
      editor.open(editorIntent.photoId);
      return true;
    case "editor-refresh":
      editor.refresh(editorIntent.photoId);
      return true;
    case "editor-workspace-action":
      editor.act(
        editorIntent.photoId,
        editorIntent.action,
        editorIntent.arguments,
      );
      return true;
    default:
      return false;
  }
}

import { WORKSPACE_OUTPUT_TEMPLATE } from "./workspace-output-surface.js";

export const PHOTO_EDITOR_TEMPLATE = `
<div class="photo-tools-view" id="photo-tools-view-edit" data-photo-tools-view="edit" hidden>
  <header class="photo-tools-view-header"><h3>Edit</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
  <div class="photo-editor-controls" aria-label="Photo edit">
    <p class="photo-editor-status" data-photo-editor-status role="status" aria-live="polite"></p>
    <p class="photo-editor-preview-note" data-photo-editor-render-status role="status"></p>
    <img class="photo-editor-preview-image" data-photo-editor-preview-image alt="Current step preview" hidden>
    <p class="photo-editor-preview-note" data-photo-editor-preview-note hidden></p>
    <button type="button" class="quiet" data-photo-editor-camera-reference aria-pressed="false">Original reference</button>
    <button type="button" class="quiet" data-photo-editor-compare aria-pressed="false" disabled>Baseline comparison</button>
    <div class="photo-editor-actions"><button type="button" class="quiet" data-photo-editor-undo disabled>Undo</button><button type="button" class="quiet" data-photo-editor-redo disabled>Redo</button><button type="button" data-photo-editor-preview disabled>Refresh preview</button><button type="button" class="quiet" data-photo-editor-rebind hidden>Keep edit for the current file</button><button type="button" class="quiet" data-photo-editor-refresh>Reload edit</button></div>
    <p class="photo-editor-draft" data-photo-editor-draft hidden role="status"></p>
    <div class="photo-editor-conflict" data-photo-editor-conflict hidden><p data-photo-editor-conflict-message role="alert"></p><div class="photo-editor-actions"><button type="button" data-photo-editor-use-saved>Use saved recipe</button><button type="button" class="quiet" data-photo-editor-reapply>Reapply my changes</button><button type="button" class="quiet" data-photo-editor-discard-draft>Discard draft</button></div></div>
    <div class="photo-editor-composable" data-photo-editor-composable aria-label="Processing Steps">
      <p class="photo-editor-export-heading">Processing Steps</p>
      <p class="photo-editor-composable-state" data-photo-editor-composable-state role="status"></p>
      <div data-photo-editor-composable-start hidden></div>
      <div class="photo-editor-composable-body" data-photo-editor-composable-body>
        <ul class="photo-editor-composable-steps" data-photo-editor-composable-steps></ul>
        <div class="photo-editor-actions"><label for="photo-editor-composable-module">Module</label><select id="photo-editor-composable-module" data-photo-editor-composable-module></select><label for="photo-editor-new-step-input">New step input</label><select id="photo-editor-new-step-input" data-photo-editor-new-step-input><option value="original">Original of this Photo</option></select><button type="button" data-photo-editor-composable-add>Add step</button></div>
        <div class="photo-editor-composable-editor" data-photo-editor-composable-editor hidden>
          <p><span data-photo-editor-composable-editing-step></span> <span data-photo-editor-composable-editing-module></span></p>
          <p>Parameter version <span data-photo-editor-composable-schema></span></p>
          <fieldset class="photo-editor-composable-input"><legend>Step input</legend><label><input type="radio" name="photo-editor-composable-input" data-photo-editor-composable-input-original> The guarded Original of this Photo</label><label><input type="radio" name="photo-editor-composable-input" data-photo-editor-composable-input-artifact> One retained Processing Artifact</label><select data-photo-editor-composable-artifact aria-label="Retained Processing Artifacts"></select></fieldset>
          <div data-photo-editor-module-controls></div>
          <p class="photo-editor-note" data-photo-editor-composable-parameters-note></p>
        </div>
        <div class="photo-editor-composable-artifacts" data-photo-editor-composable-artifacts>
          <p class="photo-editor-export-heading">Retained outputs and tasks</p>
          <ul class="photo-editor-composable-artifact-list" data-photo-editor-composable-artifact-list></ul>
          <ul class="photo-editor-composable-artifact-list" data-photo-editor-export-list></ul>
          <div class="photo-editor-actions"><label for="photo-editor-composable-artifact-id">Artifact identity</label><input id="photo-editor-composable-artifact-id" data-photo-editor-composable-artifact-id type="text" autocomplete="off"><button type="button" class="quiet" data-photo-editor-composable-artifact-fetch>Read artifact</button></div>
        </div>
      </div>
    </div>
    <div class="photo-editor-export" aria-label="Export"><p class="photo-editor-export-heading">Export <span data-photo-editor-export-target></span></p><p data-photo-editor-export-state role="status"></p><div class="photo-editor-actions"><button type="button" data-photo-editor-export-submit>Export</button><button type="button" class="quiet" data-photo-editor-export-check hidden>Check status</button><button type="button" class="quiet" data-photo-editor-export-cancel hidden>Cancel</button><button type="button" class="quiet" data-photo-editor-export-retry hidden>Retry</button><button type="button" class="quiet" data-photo-editor-export-download hidden>Download</button></div></div>
    ${WORKSPACE_OUTPUT_TEMPLATE}
    <details class="photo-editor-details"><summary>Details</summary><p data-photo-editor-provenance></p><p data-photo-editor-detail hidden></p><p data-photo-editor-capability hidden></p><p data-photo-editor-support></p><p data-photo-editor-proxy-state></p><div class="photo-editor-actions"><button type="button" class="quiet" data-photo-editor-proxy-create>Create Development Proxy</button><button type="button" class="quiet" data-photo-editor-proxy-remove hidden>Remove Development Proxy</button></div></details>
  </div>
</div>`;

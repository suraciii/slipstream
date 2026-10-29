export const PHOTO_EDITOR_TEMPLATE = `
<div class="photo-tools-view photo-editor-workspace" id="photo-tools-view-edit" data-photo-tools-view="edit" hidden>
  <header class="photo-tools-view-header"><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
  <div class="photo-editor-layout">
    <section class="photo-editor-image-region" aria-label="Edit image and reference">
      <p class="photo-editor-preview-note" data-photo-editor-render-status role="status"></p>
      <div class="photo-editor-image-frame">
        <img class="photo-editor-preview-image" data-photo-editor-preview-image alt="Selected workspace preview" hidden>
        <img class="photo-editor-reference-image" data-photo-editor-reference-image alt="Camera reference" hidden>
        <p class="photo-editor-image-empty" data-photo-editor-image-empty>Camera reference unavailable</p>
      </div>
      <div class="photo-editor-view-actions"><button type="button" class="quiet" data-photo-editor-current>Current edit</button><button type="button" class="quiet" data-photo-editor-camera-reference aria-pressed="false">Camera reference</button><button type="button" class="quiet" data-photo-editor-compare aria-pressed="false">Baseline comparison</button></div>
      <div data-workspace-editor-actions class="photo-editor-actions" role="group" aria-label="Primary actions"></div>
    </section>
    <div class="photo-editor-controls workspace-editor" aria-label="Photo edit controls" tabindex="0">
      <p data-photo-editor-status role="status" aria-live="polite">Preparing input</p>
      <p data-photo-editor-input></p>
      <div data-workspace-error role="alert" hidden></div>
      <section data-workspace-selection aria-label="Editing input and saved edits"></section>
      <section data-workspace-controls aria-label="Tool settings"></section>
      <section data-workspace-jobs aria-label="Processing jobs"></section>
      <details data-workspace-revisions><summary>Retained revisions</summary><div data-workspace-revision-list></div></details>
      <details data-workspace-files><summary>Files</summary><div data-workspace-file-groups></div><div data-workspace-file-inspection></div></details>
      <section data-workspace-film aria-label="Continue processing"></section>
      <section data-workspace-catalog aria-label="Resources, presets and styles"></section>
      <details><summary>Edit facts</summary><div class="photo-editor-facts" data-workspace-facts></div></details>
      <details><summary>Details</summary><pre data-workspace-details></pre></details>
    </div>
  </div>
</div>`;

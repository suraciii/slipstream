import { PHOTO_EDITOR_TEMPLATE } from "./photo-editor-template.js";

export const PHOTO_TOOLS_TEMPLATE = `
          <dialog class="photo-tools-dialog" data-photo-tools aria-labelledby="photo-tools-title">
            <div class="photo-tools-sheet" id="photo-tools-panel">
              <header class="photo-tools-header"><h2 id="photo-tools-title" data-photo-tools-title>Photo tools</h2><button type="button" class="quiet" data-photo-tools-close>Close</button></header>
              <div class="photo-tools-body">
                <div class="photo-tools-view" data-photo-tools-view="tools">
                  <section class="photo-tools-review" aria-label="Review"><h3>Review</h3><div class="photo-tools-actions"><button type="button" class="select-button" data-photo-tools-pick>Pick</button><button type="button" class="reject-button" data-photo-tools-reject>Reject</button><button type="button" class="quiet" data-photo-tools-clear>Clear flag</button><button type="button" class="quiet" data-photo-tools-undo disabled>Undo</button></div></section>
                  <div class="photo-tools-entries" data-photo-tools-entries role="group" aria-label="Photo tools"><div class="photo-tools-navigation" role="group" aria-label="Photo navigation"><button type="button" data-photo-tools-entry="previous">Previous</button><button type="button" data-photo-tools-entry="next">Next</button></div><button type="button" data-photo-tools-entry="rating">Rating</button><button type="button" data-photo-tools-entry="edit">Edit</button><button type="button" data-photo-tools-entry="albums">Albums</button><button type="button" data-photo-tools-entry="details">Capture Details / Preview Source</button><button type="button" data-photo-tools-entry="zoom">Preview Zoom</button><button type="button" data-photo-tools-entry="nearby">Nearby Photos</button><button type="button" data-photo-tools-entry="sources">Sources</button><p class="photo-tools-help">Gestures: swipe left for Next, right for Previous, up to advance Selection State, down to reverse. Keyboard alternatives: Arrow keys navigate; P Pick; X Reject; U Clear flag; 0–5 Rating.</p></div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-rating" data-photo-tools-view="rating" hidden>
                  <header class="photo-tools-view-header"><h3>Rating</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <fieldset class="rating-controls"><legend>Rating</legend><div data-ratings></div></fieldset>
                </div>
${PHOTO_EDITOR_TEMPLATE}
                <div class="photo-tools-view" id="photo-tools-view-albums" data-photo-tools-view="albums" hidden>
                  <header class="photo-tools-view-header"><h3>Albums</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="membership" data-membership aria-label="Album membership"><div class="membership-facts"><p class="membership-heading">Albums</p><p class="membership-status" data-membership-status role="status">Loading Albums…</p><ul class="membership-list" data-membership-list hidden></ul><p class="membership-message" data-membership-message role="alert" hidden></p><div class="membership-actions"><button type="button" class="quiet" data-membership-manage aria-expanded="false" aria-controls="membership-panel">Manage</button><button type="button" data-membership-retry hidden>Retry Albums</button></div></div><div class="membership-panel" id="membership-panel" data-membership-panel hidden><div class="membership-options" data-membership-options></div></div></div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-details" data-photo-tools-view="details" hidden>
                  <header class="photo-tools-view-header"><h3>Details</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="photo-tools-facts" data-metadata aria-label="Capture details"><strong>Capture Details</strong><dl><div><dt>Captured</dt><dd data-metadata-capture-time>—</dd></div><div><dt>Aperture</dt><dd data-metadata-aperture>—</dd></div><div><dt>ISO</dt><dd data-metadata-iso>—</dd></div><div><dt>Shutter</dt><dd data-metadata-shutter-speed>—</dd></div><div><dt>Focal Length</dt><dd data-metadata-focal-length>—</dd></div></dl></div>
                  <p class="photo-tools-preview"><span class="photo-tools-preview-label">Preview Source</span><span data-source>—</span><span class="photo-tools-detail-limit" data-detail-limit hidden>Limited by camera Preview resolution</span></p>
                  <div class="external-metadata" data-external-metadata aria-label="External Metadata"></div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-zoom" data-photo-tools-view="zoom" hidden>
                  <header class="photo-tools-view-header"><h3>Preview Zoom</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="photo-tools-zoom" data-zoom-controls role="group" aria-label="Preview zoom">
                    <button type="button" class="zoom-control" data-zoom-fit aria-pressed="true" aria-label="Fit Window">Fit Window</button>
                    <button type="button" class="zoom-control zoom-step" data-zoom-out aria-label="Zoom out">−</button>
                    <input class="zoom-slider" type="range" data-zoom-slider min="10" max="800" step="1" value="100" aria-label="Zoom percentage" />
                    <button type="button" class="zoom-control zoom-step" data-zoom-in aria-label="Zoom in">+</button>
                    <span class="zoom-level" data-zoom-level>—</span>
                    <button type="button" class="zoom-control" data-zoom-100 aria-label="Zoom to 100 percent">100%</button>
                  </div>
                </div>
                <div class="photo-tools-view" id="photo-tools-view-nearby" data-photo-tools-view="nearby" hidden>
                  <header class="photo-tools-view-header"><h3>Nearby Photos</h3><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="filmstrip-tools" data-filmstrip-tools></div>
                </div>
              </div>
            </div>
          </dialog>`;

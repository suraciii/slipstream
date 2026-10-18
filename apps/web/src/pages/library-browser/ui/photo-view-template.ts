import { PHOTO_TOOLS_TEMPLATE } from "./photo-tools-template.js";

export const PHOTO_VIEW_TEMPLATE = `
<section class="photo-view" data-review data-photo-view hidden tabindex="-1" aria-labelledby="photo-title">
  <header class="photo-header"><button type="button" class="quiet" data-back>Back to Grid</button><div class="photo-identity"><h2 id="photo-title" data-photo-title>Photo</h2><p class="photo-filename" data-photo-filename>—</p></div><div class="photo-header-actions"><button type="button" class="quiet photo-source-toggle" data-photo-source-toggle aria-controls="source-panel" aria-expanded="false" hidden>Sources</button><button type="button" class="quiet" data-photo-status-recovery aria-label="Status and Recovery">Status and Recovery</button><button type="button" class="quiet" data-photo-library>Library management</button><p class="photo-connection" data-photo-connection role="status" hidden></p><button type="button" class="quiet" data-retry-photo hidden>Retry</button></div></header>
  <section class="preview" data-preview aria-label="Photo Preview">
    <div class="swipe-feedback reject" data-reject-feedback role="status" aria-live="polite">Previous</div>
    <div class="image-stage" data-stage></div>
    <div class="swipe-feedback select" data-select-feedback role="status" aria-live="polite">Next</div>
    <div class="rating-wheel" data-rating-wheel hidden role="dialog" aria-label="Rating Wheel" aria-describedby="rating-wheel-instructions">
      <p class="rating-wheel-instructions" id="rating-wheel-instructions" data-rating-wheel-instructions>Move across a Rating and release to save.</p>
      <p class="rating-wheel-status" data-rating-wheel-status role="status" aria-live="polite"></p>
      <div class="rating-wheel-options" data-rating-wheel-options role="group" aria-label="Rating choices"></div>
    </div>
  </section>
  <div class="filmstrip-host" data-filmstrip-host><div class="filmstrip" data-filmstrip role="group" aria-label="Neighbor Photos" hidden></div></div>
  <footer class="photo-review-controls" aria-label="Photo review">
    <div class="photo-navigation" data-photo-navigation role="group" aria-label="Photo navigation"><button type="button" class="quiet" data-dock-previous disabled>Previous</button><p class="photo-position" data-position>0 / 0</p><button type="button" class="quiet" data-dock-next disabled>Next</button><button type="button" class="selection-badge" data-selection aria-controls="photo-tools-panel" aria-expanded="false" aria-label="Selection State: Unflagged">Unflagged</button><span data-rating>Rating: 0 stars (no Rating)</span></div>
    <div class="quick-action-dock" data-quick-action-dock role="group" aria-label="Photo actions">
      <button type="button" class="select-button" data-dock-select>Select</button>
      <button type="button" class="reject-button" data-dock-reject>Reject</button>
      <button type="button" class="quiet" data-dock-undo disabled>Undo</button>
      <button type="button" class="rating-dock-button" data-dock-rating disabled aria-controls="rating-choices" aria-expanded="false">Rating 0★</button>
      <button type="button" class="quiet" data-dock-more aria-controls="photo-tools-panel" aria-expanded="false">More</button>
    </div>
    <p class="status" data-status role="status" aria-live="polite" hidden></p>
  </footer>
  ${PHOTO_TOOLS_TEMPLATE}
</section>`;

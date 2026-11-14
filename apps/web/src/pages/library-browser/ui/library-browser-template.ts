import { PHOTO_VIEW_TEMPLATE } from "./photo-view-template.js";
import { RECOVERY_PANEL_TEMPLATE } from "./recovery-panel.js";
import { REMOVED_PANELS_TEMPLATE } from "./removed-panels.js";
export const APP_SHELL_TEMPLATE = `
<div class="app-shell">
  <header class="app-header"><div class="app-wordmark">Slipstream</div><p data-connection role="status">Connecting…</p></header>
  <section class="browser" data-browser>
    <div class="library-status-entry" data-library-status-entry hidden><button type="button" data-library-status-toggle aria-expanded="false" aria-controls="library-status-notice">Library status</button><div class="library-status-notice" id="library-status-notice" data-library-status-notice role="status" aria-live="polite" hidden></div></div>
    <dialog class="source-dialog" data-source-dialog>
      <nav class="source-panel" id="source-panel" data-library-screen aria-label="Library navigation">
        <header class="source-header"><h2 id="library-navigation-title">Library</h2><button type="button" class="quiet source-close" data-source-close>Close</button></header>
        <p data-summary-status role="status">Loading Library…</p>
        <p class="recovery-notice" data-recovery-notice hidden role="status"></p>
        <label class="source-filter"><span>Filter folders and albums</span><input type="search" data-source-filter autocomplete="off" spellcheck="false" /></label>
        <p class="source-filter-empty" data-source-filter-empty role="status" hidden>No matching folders or albums.</p>
        <div class="source-list" data-source-list></div>
        <footer class="source-footer"><button type="button" data-retry hidden>Retry connection</button><button type="button" class="quiet" data-access-sign-out>Sign out</button></footer>
        <p class="source-width-status" data-source-width-status role="status" aria-live="polite" hidden></p>
      </nav>
    </dialog>
    <div class="source-resizer" data-source-resizer role="separator" aria-label="Resize Library navigation" aria-orientation="vertical" aria-valuemin="240" aria-valuemax="360" aria-valuenow="280" tabindex="0"></div>
    <section class="grid-view" data-grid-view aria-labelledby="grid-title">
      <header class="grid-header"><div class="grid-header-row">
        <div class="grid-source"><span class="source-toggle-indicator" aria-hidden="true">▾</span><span class="grid-source-details"><span class="grid-heading"><h1 id="grid-title" data-grid-title>All Photos</h1><span class="grid-source-path" data-grid-source-path hidden></span></span><span class="grid-status-slot"><p id="grid-status" data-grid-status role="status"></p></span></span><button type="button" class="quiet source-toggle" data-source-toggle aria-controls="source-panel" aria-expanded="false" aria-labelledby="source-toggle-label grid-title grid-status"><span id="source-toggle-label" hidden>Library —</span></button></div>
        <p class="grid-connection" data-grid-connection role="status" hidden></p>
        <div class="grid-tools" data-grid-tools><button type="button" class="quiet" data-grid-view-options aria-label="View options">View options<span class="options-flag" data-view-options-flag hidden></span></button><button type="button" class="quiet" data-grid-select-mode aria-pressed="false">Select mode</button></div>
        <div class="grid-selection" data-grid-selection hidden><p class="grid-selection-count" data-batch-count></p><button type="button" class="quiet" data-grid-multi-done>Done</button></div>
      </div><p class="grid-summary" data-grid-summary role="status" aria-live="polite"></p></header>
      <div class="grid-viewport" data-grid-viewport tabindex="0" role="group" aria-label="Photo Library Grid"><div class="grid-canvas" data-grid-canvas></div><div class="grid-layer" data-grid-layer></div><div class="grid-empty" data-grid-empty hidden><p data-grid-empty-message role="status"></p><button type="button" data-grid-empty-action hidden>Check Library</button><button type="button" data-grid-empty-clear-filters hidden>Clear filters</button></div></div>
      <div class="grid-mobile-actions" data-grid-mobile-actions></div>
      <div class="grid-batch" data-grid-batch hidden><p class="grid-batch-retained" data-batch-retained hidden>Selection remains active</p><details class="grid-batch-result" data-grid-batch-result hidden><summary>Result</summary><div class="grid-batch-result-details"><p data-grid-batch-result-text></p><button type="button" class="quiet" data-grid-batch-compensate hidden>Remove added Photos</button></div></details><div class="grid-batch-actions" data-batch-actions role="group" aria-label="Batch actions"><button type="button" data-batch-pick>Pick</button><button type="button" data-batch-reject>Reject</button><button type="button" data-batch-album-add>Add to Album</button></div></div>
    </section>
${PHOTO_VIEW_TEMPLATE}
    <dialog class="options-dialog" data-view-options aria-labelledby="view-options-title">
      <div class="options-sheet">
        <header class="options-header"><h2 id="view-options-title">View options</h2><button type="button" class="quiet" data-view-options-close>Close</button></header>
        <div class="options-body">
          <div class="options-main-view" data-options-main-view>
          <p class="options-committed-summary" data-committed-filter-summary hidden></p>
          <p class="options-progress" data-grid-source-progress role="status"></p>
          <p class="options-progress" data-grid-visible-results role="status"></p>
          <section aria-labelledby="options-selection-title"><h3 id="options-selection-title">Selection</h3>
          <div class="options-field"><div role="group" aria-label="Selection State" data-selection-segments><button type="button" data-segment="all" aria-pressed="true">All</button><button type="button" data-segment="unflagged" aria-pressed="false">Unflagged</button><button type="button" data-segment="picked" aria-pressed="false">Picked</button><button type="button" data-segment="rejected" aria-pressed="false">Rejected</button></div></div>
          </section>
          <section aria-labelledby="options-display-title"><h3 id="options-display-title">Order and display</h3>
          <div class="options-field"><label for="view-sort-select">Source order</label><select id="view-sort-select" data-sort-select></select></div>
          <div class="options-field"><label for="view-size-select">Thumbnail size</label><select id="view-size-select" data-size-select></select></div>
          </section>
          <section aria-labelledby="options-filter-title"><h3 id="options-filter-title">Filter</h3>
          <fieldset class="options-filter-group" data-rating-group>
            <legend class="visually-hidden">Rating</legend>
            <button type="button" class="options-disclosure" data-rating-collapse aria-expanded="false" aria-controls="rating-fields">Rating</button>
            <div id="rating-fields" data-rating-fields hidden>
              <div class="options-field"><label for="view-rating-min-select">Minimum</label><select id="view-rating-min-select" data-rating-min-select aria-describedby="rating-help"></select></div>
              <div class="options-field"><label for="view-rating-max-select">Maximum</label><select id="view-rating-max-select" data-rating-max-select aria-describedby="rating-help rating-error"></select></div>
              <button type="button" class="quiet options-inline-action" data-rating-match disabled>Match minimum</button>
              <p class="options-help" id="rating-help">Minimum and maximum are inclusive, so the view matches every Rating from Minimum through Maximum. Match minimum sets both bounds to the Minimum for one exact Rating.</p>
            </div>
            <p class="options-error" id="rating-error" data-rating-error role="alert" hidden></p>
          </fieldset>
          <button type="button" class="quiet" data-reset-filters>Reset filters</button>
          <details class="options-advanced" data-advanced-filters>
            <summary>Advanced filters</summary>
            <div class="options-advanced-fields">
              <div class="options-field"><label for="view-kind-select">Original kind</label><select id="view-kind-select" data-kind-select></select></div>
              <div class="options-field"><label for="view-available-select">Original availability</label><select id="view-available-select" data-available-select></select></div>
              <fieldset class="options-filter-group" data-capture-time-group>
                <legend>Capture Time</legend>
                <div class="options-field"><label for="view-captured-from-date">From date</label><input id="view-captured-from-date" type="date" data-captured-from-date aria-describedby="capture-time-help capture-time-error"></div>
                <div class="options-field"><label for="view-captured-through-date">Through date</label><input id="view-captured-through-date" type="date" data-captured-through-date aria-describedby="capture-time-help capture-time-error"></div>
                <details class="options-exact-time" data-capture-exact>
                  <summary>Exact time</summary>
                  <div class="options-exact-fields">
                    <div class="options-field"><label for="view-captured-from">From (inclusive)</label><input id="view-captured-from" type="text" autocomplete="off" spellcheck="false" data-captured-from aria-describedby="capture-time-help capture-exact-help capture-time-error"></div>
                    <div class="options-field"><label for="view-captured-before">Before (exclusive)</label><input id="view-captured-before" type="text" autocomplete="off" spellcheck="false" data-captured-before aria-describedby="capture-time-help capture-exact-help capture-time-error"></div>
                    <p class="options-help" id="capture-exact-help">Camera-local time as YYYY-MM-DDTHH:MM:SS. No time zone.</p>
                  </div>
                </details>
                <p class="options-help" id="capture-time-help">Day range uses camera-local whole days. Through date is inclusive; use Exact time for second precision.</p>
                <p class="options-error" id="capture-time-error" data-capture-time-error role="alert" hidden></p>
              </fieldset>
            </div>
          </details>
          <p class="options-progress" data-draft-discard-notice hidden></p>
          <section class="options-browse-actions" data-browse-actions aria-labelledby="options-browse-title"><h3 id="options-browse-title">Browse actions</h3><button type="button" data-random-photo aria-label="Open a random Photo from the current source and filter">Open a random Photo</button><button type="button" data-album-resume hidden>Resume</button></section>
          <div class="options-source-entry"><button type="button" data-source-actions-open>Source actions</button></div>
          </div>
          <div class="options-subview" data-source-actions-view hidden>
            <header class="options-subview-header"><button type="button" class="quiet" data-source-actions-return>View options</button></header>
            <section class="options-source-actions" data-options-source-actions aria-labelledby="options-source-title"><h3 id="options-source-title">Source actions</h3>
            <div class="folder-album-controls" data-folder-album-controls hidden><label for="folder-album-select">Add Folder to</label><select id="folder-album-select" data-folder-album-select></select><button type="button" data-add-folder-to-album>Add Folder</button><p data-folder-album-status role="status" aria-live="polite"></p></div>
            <button type="button" data-refresh>Refresh Current Source</button><button type="button" class="quiet destructive-label" data-removal-open hidden>Remove rejected Photos</button><button type="button" class="quiet destructive-label" data-removed-open>Trash</button></section>
          </div>
        </div>
        <footer class="options-footer" data-options-footer><button type="button" data-view-options-apply>Apply</button><button type="button" class="quiet" data-view-options-cancel>Cancel</button></footer>
      </div>
    </dialog>
    <dialog class="album-dialog" data-album-form-dialog aria-labelledby="album-form-title">
      <div class="album-dialog-sheet" data-album-form-body></div>
    </dialog>
${RECOVERY_PANEL_TEMPLATE}
${REMOVED_PANELS_TEMPLATE}
  </section>
</div>`;

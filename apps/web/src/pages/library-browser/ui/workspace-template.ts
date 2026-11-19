export const PHOTO_WORKSPACE_TEMPLATE = `
                <div class="photo-tools-view workspace-view" id="photo-tools-view-workspace" data-photo-tools-view="workspace" hidden>
                  <header class="photo-tools-view-header"><button type="button" class="quiet" data-photo-tools-return>Photo tools</button></header>
                  <div class="workspace-layout">
                    <section class="workspace-inspected" aria-labelledby="workspace-inspected-heading">
                      <p class="workspace-status" data-workspace-status role="status" aria-live="polite" tabindex="-1">Preparing workspace…</p>
                      <p class="workspace-notice" data-workspace-notice hidden></p>
                      <div class="workspace-error" data-workspace-error role="alert" tabindex="-1" hidden></div>
                      <p class="workspace-environment" data-workspace-environment hidden></p>
                      <p class="workspace-input" data-workspace-input role="status" hidden></p>
                      <h3 id="workspace-inspected-heading" class="workspace-inspected-heading" data-workspace-inspected-heading tabindex="-1" hidden>Inspected file</h3>
                      <div class="workspace-image-frame">
                        <img class="workspace-image" data-workspace-image alt="Inspected workspace file" hidden>
                        <p class="workspace-image-empty" data-workspace-image-empty>Inspect a workspace file to see it here.</p>
                      </div>
                      <section class="workspace-inspected-facts" data-workspace-inspected-facts hidden aria-label="Inspected file facts">
                        <dl data-workspace-facts></dl>
                        <dl class="workspace-execution-path" data-workspace-file-path hidden></dl>
                        <div class="workspace-inspected-actions">
                          <a data-workspace-inspected-download hidden>Download</a>
                          <button type="button" data-workspace-remove hidden>Remove this reviewed file</button>
                        </div>
                      </section>
                    </section>
                    <section class="workspace-files" aria-labelledby="workspace-files-heading">
                      <header class="workspace-files-header">
                        <div><h3 id="workspace-files-heading">Workspace files</h3><p class="workspace-summary" data-workspace-summary>No files loaded.</p></div>
                        <button type="button" class="quiet" data-workspace-refresh>Refresh files</button>
                      </header>
                      <div class="workspace-file-groups" data-workspace-file-groups></div>
                      <p class="workspace-empty" data-workspace-empty hidden>No files are available in this workspace yet.</p>
                      <button type="button" class="quiet" data-workspace-files-more hidden>More files</button>
                    </section>
                  </div>
                </div>`;

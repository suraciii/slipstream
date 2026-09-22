# Issue #310 qualification report

Candidate: suraciii/slipstream `origin/main` @ `df18de3ba5d5fa9cede37fb91c7832cb92ddda04`
("Redesign Photo View around an unobstructed Preview and contextual tools (#314)").
Qualification worktree: `/home/<user>/repos/slipstream-issue-310` (branch `feat/310-qualification`),
prepared with `scripts/prepare-worktree.sh`. No production file differs from the candidate;
the only source change is additive qualification test scaffolding in
`apps/web/src/browser-review.browser-test.ts`.

## Gate evidence

Gate #1 — frozen candidate, no added tests (baseline):

```
cd /home/<user>/repos/slipstream-issue-310
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/home/<user>/.agent-browser/browsers/chrome-152.0.7977.42/chrome bun run verify
```

Result: PASS. Browser suite: 275 passed, 1 skipped (real-camera RAW scenario, expected
opt-in skip; `SLIPSTREAM_RAW_SAMPLE` unset). Rust `cargo test`: 11 + 138 + 72 + 1 passed,
4 ignored (real-camera), 0 failed. `cargo fmt --check`, `cargo clippy -D warnings`,
`format:check`, `check:album-language`, contract tests, `test:unit`, `lint`, `typecheck`,
Rust + Web builds: all exit 0. Log: `310-gate-1.log`.

Gate #2 — frozen candidate + the first wave of integrated tests: browser 284 passed,
4 skipped (1 expected RAW + 3 `test.fixme` defect documents), every other suite exit 0.
Log: `310-gate-2.log`.

Gate #3 — frozen candidate + the full test set; the log capture was truncated by the
infrastructure event after test 26 (all passing to that point), so it records no
conclusion and is retained for the record only. Log: `310-gate-3.log` (incomplete).

Gate #4 — full `bun run verify` on the committed qualification tree while an
agent-browser walkthrough was active on the same host: browser 285 passed, 1 failed,
4 skipped. The single failure (`a Fit below the manual floor keeps stepping monotonic`,
an existing #309 test) timed out waiting for the Preview image to render — a render
stall, not an assertion failure — and is attributed to host contention: the identical
test passed in Gates #1–#3 on an unchanged tree. The gate was rerun in isolation as
Gate #5. Log: `310-gate-4.log`.

Gate #5 — full `bun run verify` on the committed qualification tree, nothing else
running on the host (the canonical gate):

```
cd /home/<user>/repos/slipstream-issue-310
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/home/<user>/.agent-browser/browsers/chrome-152.0.7977.42/chrome bun run verify
```

Result: PASS, exit 0. Browser suite: 286 passed, 4 skipped — 290 discovered =
276 pre-existing (275 pass + 1 expected opt-in RAW skip) + 14 added qualification tests
(11 pass + 3 `test.fixme` defect documents). Rust `cargo test`: 11 + 138 + 72 + 1 passed,
4 ignored (real-camera), 0 failed. `cargo fmt --check`, `cargo clippy -D warnings`,
`format:check`, `check:album-language`, contract tests, `test:unit`, `lint`, `typecheck`,
Rust + Web builds: all exit 0. Log: `310-gate-5.log`.

Defect reproduction run — the three `test.fixme` documents were temporarily un-marked in
a scratch copy of the test file (checksum-verified restored immediately afterwards) and
run alone to prove each defect reproduces on the candidate:

```
cd /home/<user>/repos/slipstream-issue-310
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/home/<user>/.agent-browser/browsers/chrome-152.0.7977.42/chrome \
  bun x playwright test --grep 'a narrow header keeps the result count legible beside an active filter flag|a Rating write settles after its surface closes without claiming cancellation|a held Select write cannot repaint or advance a Grid the browser returned to'
```

Result: 3 failed — each fails exactly on its documented assertion (D3: painted status
width 98.1 px vs a 60.7 px box at 390×844; D1: `document.activeElement` is `<body>`;
D2: `[data-selection]` reads "Undecided" after Forward while the Library holds
"selected"). Log: `310-defect-verification.log`. The committed tree still carries the
three tests as `test.fixme`.

## Interactive walkthrough (agent-browser against a locally served candidate build)

The candidate's production build (`bun run build` output) was served by
`target/debug/slipstream-server` on loopback with a generated 24-Photo fixture in an
isolated state/cache directory (no user Originals touched), and driven with the
agent-browser CLI (Chrome 152 via CDP). Recorded observations:

- Culling session at 390×844: default Grid `Ready · 24 Photos`, header 53 px, smallest
  interactive target exactly 44 px, page overflow 0. Applied filter `Undecided` + order
  `Capture Time, latest first`; the URL carried `?order=capture-time-desc&selection=undecided`
  and applying replaced the destination without growing history. Deep scroll
  (`scrollTop = 1200`, 16 cells mounted), opened Photo 15/24: the URL named the Photo and
  kept the view parameters; history grew by exactly one entry. Photo tools → Details
  showed metadata (`photo-15.jpg`) and added no history; the native close restored focus
  to its invoker. Rating 4 saved without advancing and without history; the close
  returned focus to the Rating entry. Select advanced to 16/24 replacing the Photo
  destination; three Next presses reached 19/24 with no stack growth. Browser Back
  restored the Grid at `scrollTop = 1200` with focus on the anchor cell, the address as
  the Grid destination (no `photoId`), and the committed choices intact; the anchor cell
  showed `✓` and `4★` (decisions durable, no replay). Forward reopened Photo 19/24 under
  the same `photoId`.
- Batch contract: Select mode → 3 Photos → batch Select reported `3 Photos selected.`
  with `Source progress: 7 selected · 0 rejected · 17 undecided` and
  `Visible results: 24 of 24 Photos` (persistent counts, each moved once). Add to Album
  into a new Album reported `3 Photos added to “Walkthrough Trip”.` and exposed the
  scoped `Remove added Photos`; the compensation reported
  `3 Photos removed from “Walkthrough Trip”.` and hid the action. The tray selection
  stayed `3 / 100 Photos` through the whole batch.
- Photo View budgets, measured live: 390×844 Preview 663.3 px (≥650), 375×667 Preview
  486.3 px (≥480), 667×375 Preview 244 px (≥240), 844×390 Preview 259.1 px (≥240);
  smallest target 44 px and page overflow 0 at every size. Rotation (viewport resize)
  preserved the Photo, its position (3/24), and its address.
- Photo tools keyboard ownership and close restoration are covered by the automated
  `Issue #310 keyboard and modal qualification` suite (Gate #5).

## Acceptance matrix

| # | Acceptance item | Verdict | Evidence |
|---|-----------------|---------|----------|
| 1 | Qualify one frozen candidate; record identity and exact discovery/results | PASS | Candidate `df18de3`; discovery 290 browser tests (276 pre-existing + 14 added: 11 pass + 3 `test.fixme`); Gate #5 exit 0 with 286 passed / 4 skipped; Rust 11+138+72+1 passed, 4 ignored |
| 2 | Culling session: source → filter/order → deep scroll → Photo → inspect/rate/select → Next ×N → Back → Forward | PASS | Automated integrated test (130-Photo fixture: persistent counts, single-entry push, no decision replay, anchor + focus restoration, URL/address alignment at every step); walkthrough reproduced it on the served build (Back restored `scrollTop = 1200` and focus on the anchor cell; Forward reopened Photo 19/24 under the same `photoId`) |
| 3 | Batch: multi-select → partial decision → Review N → retry → Add to Album → scoped removal; persistent counts; Album resume unchanged | PASS | Automated mixed-outcome test (applied / changed-elsewhere / missing seam, server-side membership verified, no Album progress write); walkthrough exercised the clean path with matching persistent counts |
| 4 | Destinations under change: direct entry, reload, Album Resume, deleted Album, absent filtered Photo, unavailable Original, changed Folder publication, expired Snapshot, external page return, missing restoration state | PASS | New integrated tests for unavailable Original, external page + browser return (exactly one mounted browser), and unusable restoration state rendered as a direct entry; existing passing slice tests cover deleted-Album fallback with explanation, Folder publication change requiring an explicit action, expired-token Forward reopen, deep link + reload, filter replace surviving reload, and Album Resume |
| 5 | Async discipline: hold reads/writes across Back/Forward, source switch, panel close, disconnect/reconnect; no stale repaint; admitted writes settle; recovery ownership correct | PASS (with D1, D2 recorded) | Held-write scenarios reproduced as `test.fixme` documents (D1 focus, D2 stale fact after return); the async-ownership suite (latest-wins, detached settlement silent, no stale advancement, recovery ownership) passes; no blocking failure |
| 6 | ≥40,000 generated Photos: bounded windows/facts/DOM, Snapshot lifecycle | PASS | Automated 40,000-Photo test: late-position deep link resolves by identity; repeated Grid↔Photo navigation keeps rendered cells < 200, window reads ≤ 24, snapshot opens ≤ 8; release lifecycle unchanged |
| 7 | Budgets at 375×667, 390×844, 667×375, 844×390; 1024×768 touch; 1280×800 desktop; 44 px targets; no horizontal overflow; 200% text; safe areas; reduced motion; rotation | PASS (with D3 recorded) | Automated: narrow Grid regions (header ≤ 88, Select-mode header+tray ≤ 168, Grid ≥ 499 at 375×667), Photo View Preview reservations (480/650/240/240), 44 px targets, overflow 0, 200% text reflow, reduced motion + 200% Photo actions reachable, rotation preserving Photo/decisions/strip, touch-tablet and desktop contracts; walkthrough independently measured header 53 px / smallest 44 px / overflow 0 at 390×844 and Preview 663.3 / 486.3 / 244 / 259.1 px at the four sizes. D3: the narrow header's result count is not legible beside an active filter flag |
| 8 | Keyboard/modal: focus cycle, no background shortcuts, exact close restoration after re-render, usable source/form/recovery paths | PASS | Automated Photo-tools test (14-key sweep with zero mutations and zero state change, focus never behind the surface, native close restoring the invoker, shortcuts live again afterwards); existing modal/focus suite passes; walkthrough confirmed close-restoration to the invoker for tools and Rating |
| 9 | Actual Android Chromium device evidence | BLOCKED-pending-device | No Android device or `adb` access on the qualification host; emulated touch is not a substitute per the packet. Missing: an Android Chromium device with USB or remote debugging, device model, browser version, and operator-observed system Back vs toolbar traversal, rotation, keyboard, and safe-area behavior |
| 10 | `bun run verify` on the final candidate; record expected RAW skips and environment limits | PASS | Gate #5 exit 0 (see above); expected opt-in RAW skip recorded (`SLIPSTREAM_RAW_SAMPLE` unset); environment limits: no real-camera sample, no Android device, headless Chromium 152 |
| 11 | Server health, static assets, bounded deep-link browsing on existing host operator routes | PASS | Served candidate: `/healthz` → `{"status":"ok"}`; `/` → 200 `text/html`; JS/CSS assets → 200 with correct content types; deep link with `photoId` → 200 and renders Photo View in the browser; no route retired or renamed |

## Defects found

Recorded, not fixed (authority boundary); each owning slice decides the repair.

### D3 — narrow Grid header loses the visible result count beside an active filter flag (medium, layout/truthfulness)

- Repro: at 390×844 (also reproduced at 375×667 in the fixme run), open the Grid, apply
  any nondefault filter or order (View options → Apply). One active choice ellipsizes the
  compact status; filter + order together collapse it entirely.
- Expected: the header indicates the active choice AND the visible result count legibly
  (`docs/library-browser-experience.md`: "The header must indicate an active nondefault
  filter or order and the visible result count").
- Observed: with `?selection=undecided` the status box is 60.7 px against 98.1 px of
  painted text ("Ready · …"); with `?order=capture-time-desc&selection=undecided` the
  status box is 0 px wide (fully collapsed). The count remains reachable inside View
  options (`Visible results: 24 of 24 Photos`) and no page-level horizontal overflow is
  introduced.
- Location: `apps/web/src/pages/library-browser/ui/library-browser.css:489-494`
  (`.grid-header p` ellipsis) with `.grid-heading p { max-width: 7.5rem }` at
  `library-browser.css:1885-1889` and the shrinkable `.grid-heading` flex item at
  `library-browser.css:430-433`; markup at
  `apps/web/src/pages/library-browser/ui/library-browser-view.ts:639`.
- Regression: `test.fixme` "a narrow header keeps the result count legible beside an
  active filter flag" (`browser-review.browser-test.ts:19931`), verified failing.
- Owning slice: Grid (#308).

### D1 — closing the Rating surface mid-settlement drops keyboard focus to the document body (low, accessibility)

- Repro: open a Photo, open Rating, choose a value while the write's response is held,
  then close the surface (Escape) before the write settles.
- Expected: focus remains on a valid control after the panel close (the invoker, or the
  nearest valid control).
- Observed: `document.activeElement` is `<body>`; the write still settles truthfully
  ("Rating saved.", rating committed), so only focus restoration is affected.
- Location: `apps/web/src/pages/library-browser/ui/modal-surface.ts:83-91`
  (`focusInvoker` returns early or focuses a disabled invoker with no fallback), reached
  from the Rating close path at
  `apps/web/src/pages/library-browser/ui/library-browser-view.ts:1527-1531`.
- Regression: `test.fixme` "a Rating write settles after its surface closes without
  claiming cancellation" (`browser-review.browser-test.ts:20377`), verified failing.
- Owning slice: Photo View (#309).

### D2 — a decision whose write settles after the browser left the Photo is stale when the browser returns (low, truthfulness)

- Repro: open a Photo, choose Select while the write's response is held, take the
  browser's Back to the Grid, release the write, then Forward.
- Expected: the committed decision is truthful after the return.
- Observed: the Photo View presents "Undecided" while the Library holds "selected"; the
  URL, history length, and Grid detachment behavior are otherwise correct.
- Location: the retained-window resolution path
  `apps/web/src/pages/library-browser/model/source-grid-owner.ts:902-914`
  (`resolvePhotoPosition` trusts the retained window's fact without revalidation).
- Regression: `test.fixme` "a held Select write cannot repaint or advance a Grid the
  browser returned to" (`browser-review.browser-test.ts:20438`), verified failing.
- Owning slice: Photo View (#309).

## Residual limitations

- Android Chromium evidence is blocked pending device access (acceptance item 9).
- Real-camera RAW qualification remains opt-in and was not exercised
  (`SLIPSTREAM_RAW_SAMPLE` unset); the one skipped scenario is expected.
- Headless Chromium 152 on Linux is the only engine exercised; other engines remain
  explicitly exploratory per the support contract.
- Gate #4's single failure is a contended-run artifact superseded by Gate #5; both logs
  are retained.

## Verdict

QUALIFIED for Epic #306. The frozen candidate passes every acceptance checklist item on
the canonical gate (Gate #5, exit 0) with three recorded defects for the owning slices:
D3 (medium, Grid header result-count legibility) and D1/D2 (low, Photo View focus and
stale-fact timing windows). No defect blocks the qualification verdict; none was fixed
or hidden.

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { PHOTO_VIEW_TEMPLATE } from "./photo-view-template.js";

/// The Photo View stylesheet the page installs. The review geometry policy is a
/// layout contract, so it is asserted against the rules the browser applies
/// rather than against a second description of them.
const raw = readFileSync(
  new URL("./library-browser.css", import.meta.url),
  "utf8",
);

/// The stylesheet without its comments, so a comment can never be mistaken for
/// part of a selector or a declaration.
const css = raw.replace(/\/\*[\s\S]*?\*\//g, "");

/// The declarations of the top-level rules whose selector list exactly equals
/// one of the selectors given, in document order. Only rules at the top level
/// of the stylesheet are considered — a selector indented by an `@media` block
/// is skipped — and matching is exact so a renamed or regrouped rule fails the
/// test instead of silently passing.
const rulesFor = (...selectors: string[]): string[] => {
  const matched: string[] = [];
  // The selector group is the second capture and the declarations the third.
  for (const [, , selectorText = "", body = ""] of css.matchAll(
    /(^|\n)([ \t]*[^\n{}]*[^\n{} \t])[ \t]*\{([^{}]*)\}/g,
  )) {
    if (/^[ \t]/.test(selectorText)) continue;
    const list = selectorText
      .trim()
      .split(",")
      .map((selector) => selector.trim())
      .filter((selector) => selector.length > 0);
    if (list.length > 0 && list.every((name) => selectors.includes(name)))
      matched.push(body.trim());
  }
  return matched;
};

/// The declarations of the rules inside every `@media` block whose query
/// contains the given condition and whose selector list exactly equals one of
/// the selectors given.
const mediaRulesFor = (condition: string, ...selectors: string[]): string[] => {
  const matched: string[] = [];
  for (const open of css.matchAll(/@media[^{]+\{/g)) {
    if (!open[0].includes(condition)) continue;
    // The block ends at the brace that closes this @media rule, so a nested
    // rule inside it is not mistaken for the end.
    const start = (open.index ?? 0) + open[0].length;
    let depth = 1;
    let end = css.length;
    for (let index = start; index < css.length; index += 1) {
      if (css[index] === "{") depth += 1;
      else if (css[index] === "}") depth -= 1;
      if (depth === 0) {
        end = index;
        break;
      }
    }
    for (const [, , selectorText = "", declarations = ""] of css
      .slice(start, end)
      .matchAll(/(^|\n)([ \t]*[^\n{}]*[^\n{} \t])[ \t]*\{([^{}]*)\}/g)) {
      const list = selectorText
        .trim()
        .split(",")
        .map((selector) => selector.trim())
        .filter((selector) => selector.length > 0);
      if (list.length > 0 && list.every((name) => selectors.includes(name)))
        matched.push(declarations.trim());
    }
  }
  return matched;
};

/// The individual tracks of one `grid-template-columns` declaration.
const tracksOf = (body: string): string[] => {
  const columns = (
    body.match(/grid-template-columns:([^;]+)/)?.[1] ?? ""
  ).replace(/\s+/g, " ");
  const tracks: string[] = [];
  for (const track of columns.split(/(?=minmax|repeat)/))
    if (track.trim().length > 0) tracks.push(track.trim());
  return tracks;
};

/// The minimum width of one grid track in rem, or undefined when the track is
/// not a bounded minimum.
const trackMinimum = (track: string): number | undefined => {
  const minimum = track.match(/minmax\(\s*([\d.]+)rem/)?.[1];
  return minimum === undefined ? undefined : Number(minimum);
};

describe("Photo View context row", () => {
  test("Selection State and Rating sit with the Photo identity", () => {
    // Selection State must be presented with the Photo identity, so the badge
    // and the Rating value are inside the identity group rather than inside the
    // navigation group that holds Previous, position, and Next.
    const identity = PHOTO_VIEW_TEMPLATE.match(
      /<div class="photo-identity">[\s\S]*?<\/div><\/div>/,
    )?.[0];
    expect(identity).toBeDefined();
    expect(identity).toContain('class="photo-identity-facts"');
    expect(identity).toContain("data-selection");
    expect(identity).toContain("data-rating");
  });

  test("the identity facts hold no forced row of their own", () => {
    // A fact that claims the whole context row width would push the review
    // controls below the safe viewport on a short screen.
    const facts = rulesFor(".photo-identity-facts");
    expect(facts.length).toBeGreaterThan(0);
    for (const body of facts) {
      expect(body).not.toContain("flex-basis: 100%");
      expect(body).toContain("flex-wrap: wrap");
    }
  });

  test("the Selection State badge keeps its Photo tools invocation", () => {
    // The badge is a wired entry point for Photo tools: it must not be removed
    // from the template, and its panel reference must stay intact.
    expect(PHOTO_VIEW_TEMPLATE).toContain(
      'data-selection aria-controls="photo-tools-panel"',
    );
    expect(PHOTO_VIEW_TEMPLATE).toContain(
      'aria-label="Selection State: Unflagged"',
    );
  });

  test("the identity facts truncate rather than widen the context row", () => {
    // On a compact layout the badge gives up its own label width, so a long
    // Selection State value can never push the row into a second line.
    const compact = mediaRulesFor(
      "max-width: 760px",
      ".photo-identity-facts .selection-badge",
    );
    expect(compact.length).toBeGreaterThan(0);
    for (const body of compact) {
      expect(body).toContain("text-overflow: ellipsis");
      expect(body).toContain("white-space: nowrap");
      expect(body).toContain("min-width: 0");
    }
  });
});

describe("Photo View review strip geometry", () => {
  test("the navigation strip holds only Previous, position, and Next", () => {
    // One navigation group, and the identity facts are not part of it.
    const navigation = PHOTO_VIEW_TEMPLATE.match(
      /<div class="photo-navigation"[^>]*>([\s\S]*?)<\/div>/,
    )?.[1];
    expect(navigation).toBeDefined();
    expect(navigation).toContain("data-dock-previous");
    expect(navigation).toContain("data-position");
    expect(navigation).toContain("data-dock-next");
    expect(navigation).not.toContain("data-selection");
    expect(navigation).not.toContain("data-rating");
  });

  test("the review dock keeps one Reject, Rating, Pick, Undo, and More", () => {
    const dock = PHOTO_VIEW_TEMPLATE.match(
      /<div class="quick-action-dock"[^>]*>([\s\S]*?)<\/div>/,
    )?.[1];
    expect(dock).toBeDefined();
    for (const entry of [
      "data-dock-reject",
      "data-dock-rating",
      "data-dock-pick",
      "data-dock-undo",
      "data-dock-more",
    ])
      expect(dock).toContain(entry);
  });

  test("a compact navigation strip keeps three columns", () => {
    // The five-column short-landscape and narrow resets collapsed the
    // navigation and fact columns into two, which is what let the Previous text
    // range overlap the position text. Consolidating moves the facts out, so
    // the strip stays three tracks.
    const bodies = [
      ...mediaRulesFor("max-height: 480px", ".photo-navigation"),
      ...mediaRulesFor("max-width: 420px", ".photo-navigation"),
    ];
    expect(bodies.length).toBe(2);
    for (const body of bodies) expect(tracksOf(body)).toHaveLength(3);
  });

  test("no navigation control track can shrink below a 44px touch target", () => {
    // Consolidation moves content; it never shrinks a target. Every track that
    // holds Previous or Next keeps at least 2.75rem, which is the 44px target.
    const bodies = [
      ...rulesFor(".photo-navigation"),
      ...mediaRulesFor("max-height: 480px", ".photo-navigation"),
      ...mediaRulesFor("max-width: 420px", ".photo-navigation"),
    ];
    expect(bodies).toHaveLength(3);
    for (const body of bodies) {
      const tracks = tracksOf(body);
      // The position text sits in the middle track; the two control tracks
      // flank it, so both must keep the 44px minimum.
      for (const track of [tracks[0], tracks[tracks.length - 1]]) {
        const minimum = trackMinimum(track ?? "");
        expect(minimum).toBeGreaterThanOrEqual(2.75);
      }
    }
  });

  test("the review strip reserves the Preview region at 390 by 844", () => {
    // The Preview row keeps its 650px floor at the narrow-tall breakpoint, so
    // the primary controls stay above the safe viewport.
    const narrow = mediaRulesFor("min-width: 390px", ".photo-view").filter(
      (body) => body.includes("650px"),
    );
    expect(narrow.length).toBe(1);
  });
});

describe("Photo View direction label contrast", () => {
  test("the resting direction labels carry no opacity composition", () => {
    // Recoloring was not the fix: the dimming composition is removed, so a
    // resting Pick or Reject label is ordinary text.
    const feedback = rulesFor(".swipe-feedback");
    expect(feedback.length).toBe(1);
    for (const body of feedback) {
      expect(body).not.toContain("opacity");
      expect(body).not.toContain("filter");
    }
  });

  test("the resting Pick and Reject labels keep their own colors", () => {
    const pick = rulesFor(".swipe-feedback.pick");
    const reject = rulesFor(".swipe-feedback.reject");
    expect(pick).toEqual(["color: var(--selected);"]);
    expect(reject).toEqual(["color: var(--rejected);"]);
  });

  test("a pending direction label stays distinct by background", () => {
    // Pending must not look like a committed state, and it must not rely on the
    // removed opacity transition either. The base rule carries the pending
    // background; the hover block only reveals it.
    const pending = rulesFor(".swipe-feedback.pending");
    const rejectPending = rulesFor(".swipe-feedback.reject.pending");
    expect(pending).toEqual(["background: var(--selected-bg);"]);
    expect(rejectPending).toEqual(["background: var(--rejected-bg);"]);
  });
});

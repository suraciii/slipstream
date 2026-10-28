import { describe, expect, test } from "bun:test";
import { APP_SHELL_TEMPLATE } from "./library-browser-view.js";
import { PHOTO_VIEW_TEMPLATE } from "./photo-view-template.js";
import { RECOVERY_PANEL_TEMPLATE } from "./recovery-panel.js";
import { REMOVED_PANELS_TEMPLATE } from "./removed-panels.js";

/// Extracts one element's full tag from the template, matching the outermost
/// element whose class list contains every name given. The shell template is
/// one long line, so this reads the markup the Browser installs without a DOM
/// parser, which the unit runtime does not provide.
const elementWith = (
  markup: string,
  tagName: string,
  ...classNames: string[]
): string => {
  const classPattern = classNames.map((name) => `\\b${name}\\b`).join("[^>]*");
  const pattern = new RegExp(
    `<${tagName}(?=[^>]*class="[^"]*${classPattern}")[^>]*>`,
  );
  const match = markup.match(pattern);
  expect(match).not.toBeNull();
  return match?.[0] ?? "";
};

const attributeOf = (tag: string, name: string): string | null =>
  tag.match(new RegExp(`\\s${name}="([^"]*)"`))?.[1] ?? null;

/// The substring between the opening tag found above and its matching close,
/// bounded by the first close tag of the same name.
const innerOf = (markup: string, openTag: string, tagName: string): string => {
  const start = markup.indexOf(openTag);
  expect(start).toBeGreaterThanOrEqual(0);
  const rest = markup.slice(start + openTag.length);
  const end = rest.indexOf(`</${tagName}>`);
  expect(end).toBeGreaterThanOrEqual(0);
  return rest.slice(0, end);
};

describe("application shell landmarks", () => {
  test("the shell owns no main landmark of its own", () => {
    // The application-level main landmark is supplied by index.html, so the
    // shell must nest none of its own.
    expect(APP_SHELL_TEMPLATE).not.toContain("<main");
  });

  test("every id the shell references exists in the shell", () => {
    // Nested templates render their own ids into the shell at install time, and
    // one dialog heading is created programmatically by its own module, so a
    // reference into one of those is checked against that source rather than
    // the shell string alone.
    const programmatic = [
      PHOTO_VIEW_TEMPLATE,
      RECOVERY_PANEL_TEMPLATE,
      REMOVED_PANELS_TEMPLATE,
      // album-form.ts sets this id on the heading it builds.
      'id="album-form-title"',
    ].join("\n");
    const installed = `${APP_SHELL_TEMPLATE}\n${programmatic}`;
    const ids = new Set<string>();
    for (const match of APP_SHELL_TEMPLATE.matchAll(
      /(?:aria-labelledby|aria-controls|for)="([^"]*)"/g,
    )) {
      for (const id of (match[1] ?? "").split(" ")) if (id !== "") ids.add(id);
    }
    expect(ids.size).toBeGreaterThan(0);
    for (const id of ids) {
      expect(installed).toContain(`id="${id}"`);
    }
  });
});

describe("grid heading ownership", () => {
  test("the grid view owns exactly one h1", () => {
    const gridView = innerOf(
      APP_SHELL_TEMPLATE,
      elementWith(APP_SHELL_TEMPLATE, "section", "grid-view"),
      "section",
    );
    expect(gridView.match(/<h1[\s>]/g)?.length).toBe(1);
  });

  test("the photo view owns exactly one h1", () => {
    expect(PHOTO_VIEW_TEMPLATE.match(/<h1[\s>]/g)?.length).toBe(1);
  });

  test("the grid heading is the disclosure's accessible name source", () => {
    const toggle = elementWith(APP_SHELL_TEMPLATE, "button", "source-toggle");
    expect(attributeOf(toggle, "aria-labelledby")).toContain("grid-title");
    const heading = APP_SHELL_TEMPLATE.match(/<h1[^>]*id="grid-title"[^>]*>/);
    expect(heading).not.toBeNull();
    // The heading must sit inside the disclosure, so the visible label and the
    // announced name are one element and cannot drift apart.
    const toggleEnd = APP_SHELL_TEMPLATE.indexOf(
      "</button>",
      APP_SHELL_TEMPLATE.indexOf(toggle),
    );
    expect(APP_SHELL_TEMPLATE.indexOf(heading?.[0] ?? "")).toBeLessThan(
      toggleEnd,
    );
  });

  test("the sources panel heading is not the view heading", () => {
    const heading = APP_SHELL_TEMPLATE.match(
      /<h2[^>]*id="sources-title"[^>]*>/,
    );
    expect(heading).not.toBeNull();
  });
});

describe("grid header disclosure policy", () => {
  test("the source name and the result count share one disclosure", () => {
    const toggle = elementWith(APP_SHELL_TEMPLATE, "button", "source-toggle");
    const inner = innerOf(APP_SHELL_TEMPLATE, toggle, "button");
    const labels = attributeOf(toggle, "aria-labelledby")?.split(" ") ?? [];
    expect(labels).toContain("grid-title");
    expect(labels).toContain("grid-status");
    // Both referenced elements are inside the one control.
    expect(inner).toContain('id="grid-title"');
    expect(inner).toContain("data-grid-status");
  });

  test("the count is not inside the heading", () => {
    const headingInner = innerOf(
      APP_SHELL_TEMPLATE,
      APP_SHELL_TEMPLATE.match(/<h1[^>]*id="grid-title"[^>]*>/)?.[0] ?? "",
      "h1",
    );
    expect(headingInner).not.toContain("data-grid-status");
  });

  test("view options and select mode keep complete labels", () => {
    const viewOptions =
      APP_SHELL_TEMPLATE.match(
        /<button[^>]*data-grid-view-options[^>]*>[^<]*/,
      )?.[0] ?? "";
    const selectMode =
      APP_SHELL_TEMPLATE.match(
        /<button[^>]*data-grid-select-mode[^>]*>[^<]*/,
      )?.[0] ?? "";
    expect(viewOptions).toContain(">View options");
    expect(viewOptions).toContain('aria-label="View options"');
    expect(selectMode).toContain(">Select mode");
    // The visible text is the accessible name, so neither label is shortened.
    expect(attributeOf(selectMode, "aria-label")).toBeNull();
  });

  test("neither tool control is inside the disclosure", () => {
    const toggle = elementWith(APP_SHELL_TEMPLATE, "button", "source-toggle");
    const inner = innerOf(APP_SHELL_TEMPLATE, toggle, "button");
    expect(inner).not.toContain("data-grid-view-options");
    expect(inner).not.toContain("data-grid-select-mode");
  });
});

describe("source separator value semantics", () => {
  test("the separator exposes a value, its bounds, and a name", () => {
    const resizer = elementWith(APP_SHELL_TEMPLATE, "div", "source-resizer");
    expect(attributeOf(resizer, "role")).toBe("separator");
    expect(attributeOf(resizer, "aria-orientation")).toBe("vertical");
    expect(attributeOf(resizer, "aria-label")).toBeTruthy();
    const now = Number(attributeOf(resizer, "aria-valuenow"));
    const min = Number(attributeOf(resizer, "aria-valuemin"));
    const max = Number(attributeOf(resizer, "aria-valuemax"));
    expect(Number.isFinite(now)).toBe(true);
    expect(Number.isFinite(min)).toBe(true);
    expect(Number.isFinite(max)).toBe(true);
    expect(min).toBeLessThan(max);
    expect(now).toBeGreaterThanOrEqual(min);
    expect(now).toBeLessThanOrEqual(max);
  });

  test("the separator is keyboard reachable", () => {
    const resizer = elementWith(APP_SHELL_TEMPLATE, "div", "source-resizer");
    expect(attributeOf(resizer, "tabindex")).toBe("0");
  });

  test("the resize announcement region is a live region", () => {
    const status = APP_SHELL_TEMPLATE.match(
      /<p[^>]*data-source-width-status[^>]*>/,
    )?.[0];
    expect(status).not.toBeNull();
    expect(attributeOf(status ?? "", "role")).toBe("status");
    expect(attributeOf(status ?? "", "aria-live")).toBe("polite");
  });
});

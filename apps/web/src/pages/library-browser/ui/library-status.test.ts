import { describe, expect, test } from "bun:test";
import { createApplicationOwner } from "../model/application-owner.js";
import { createApplicationPresentationController } from "../model/application-presentation-controller.js";
import { RecoveryGate } from "../model/async-ownership.js";
import type { LibraryBrowserIntent } from "./library-browser-view-contract.js";
import { createSummaryPresenter } from "./library-status.js";

// The unit runtime has no DOM. These elements implement the DOM operations
// used by the summary presenter so the tests exercise its event handlers.
class ElementStub {
  hidden = false;
  disabled = false;
  type = "";
  className = "";
  private children: Array<ElementStub | string> = [];
  private attributes = new Map<string, string>();
  private listeners = new Map<string, Array<() => void>>();
  readonly selectors = new Map<string, ElementStub>();

  get textContent(): string {
    return this.children
      .map((child) => (typeof child === "string" ? child : child.textContent))
      .join("");
  }

  set textContent(value: string) {
    this.children = [value];
  }

  replaceChildren(...children: Array<ElementStub | string>): void {
    this.children = children;
  }

  append(...children: Array<ElementStub | string>): void {
    this.children.push(...children);
  }

  querySelector(selector: string): ElementStub | null {
    if (selector === "button")
      return (
        this.children.find(
          (child): child is ElementStub =>
            child instanceof ElementStub && child.type === "button",
        ) ?? null
      );
    return this.selectors.get(selector) ?? null;
  }

  setAttribute(name: string, value: string): void {
    this.attributes.set(name, value);
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  addEventListener(type: string, listener: () => void): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  click(): void {
    for (const listener of this.listeners.get("click") ?? []) listener();
  }
}

const element = (stub: ElementStub): HTMLElement =>
  stub as unknown as HTMLElement;
const button = (stub: ElementStub): HTMLButtonElement =>
  stub as unknown as HTMLButtonElement;

const fixture = () => {
  const previousDocument = Object.getOwnPropertyDescriptor(
    globalThis,
    "document",
  );
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      createTextNode: (text: string) => text,
      createElement: () => new ElementStub(),
    },
  });
  const root = new ElementStub();
  const entry = new ElementStub();
  const toggle = new ElementStub();
  const notice = new ElementStub();
  const summary = new ElementStub();
  const gridSummary = new ElementStub();
  const gridEmptyAction = new ElementStub();
  entry.hidden = true;
  notice.hidden = true;
  toggle.setAttribute("aria-expanded", "false");
  root.selectors.set("[data-library-status-entry]", entry);
  root.selectors.set("[data-library-status-toggle]", toggle);
  root.selectors.set("[data-library-status-notice]", notice);
  const intents: LibraryBrowserIntent[] = [];
  const present = createSummaryPresenter(
    element(root),
    element(summary),
    element(gridSummary),
    button(gridEmptyAction),
    (intent) => intents.push(intent),
  );
  return {
    entry,
    toggle,
    notice,
    summary,
    gridSummary,
    gridEmptyAction,
    intents,
    present,
    restore: () => {
      if (previousDocument)
        Object.defineProperty(globalThis, "document", previousDocument);
      else Reflect.deleteProperty(globalThis, "document");
    },
  };
};

describe("compact Library status", () => {
  test("normal and background summaries keep the entry hidden", () => {
    const ui = fixture();
    try {
      ui.present("Library ready", undefined, "idle");
      expect(ui.entry.hidden).toBe(true);
      expect(ui.notice.hidden).toBe(true);
      expect(ui.toggle.getAttribute("aria-expanded")).toBe("false");
      expect(ui.summary.textContent).toBe("Library ready");
      expect(ui.gridSummary.textContent).toBe("");

      ui.present("Applying Library updates…", undefined, "active");
      expect(ui.entry.hidden).toBe(true);
      expect(ui.gridSummary.textContent).toBe("Applying Library updates…");
      expect(ui.gridEmptyAction.disabled).toBe(true);
    } finally {
      ui.restore();
    }
  });

  test("an owning failure exposes a disclosure with text and Retry action", () => {
    const ui = fixture();
    try {
      ui.present(
        "Library check failed; the last complete Library remains available.",
        { kind: "retry-library-check", presentationId: 42 },
        "failed",
      );
      expect(ui.entry.hidden).toBe(false);
      expect(ui.notice.hidden).toBe(true);
      expect(ui.toggle.getAttribute("aria-expanded")).toBe("false");

      ui.toggle.click();
      expect(ui.notice.hidden).toBe(false);
      expect(ui.toggle.getAttribute("aria-expanded")).toBe("true");
      expect(ui.notice.textContent).toContain(
        "Library check failed; the last complete Library remains available.",
      );
      const retry = ui.notice.querySelector("button");
      expect(retry?.textContent).toBe("Retry Library Check");
      retry?.click();
      expect(ui.intents).toEqual([
        { kind: "summary-action", presentationId: 42 },
      ]);

      ui.toggle.click();
      expect(ui.notice.hidden).toBe(true);
      expect(ui.toggle.getAttribute("aria-expanded")).toBe("false");
    } finally {
      ui.restore();
    }
  });

  test("completion presents and dispatches Refresh, then clears when ownership ends", () => {
    const ui = fixture();
    try {
      ui.present(
        "This view still shows the earlier result.",
        { kind: "refresh-current-source", presentationId: 73 },
        "complete",
      );
      expect(ui.entry.hidden).toBe(false);
      ui.toggle.click();
      expect(ui.notice.hidden).toBe(false);
      expect(ui.notice.textContent).toContain(
        "This view still shows the earlier result.",
      );
      const refresh = ui.notice.querySelector("button");
      expect(refresh?.textContent).toBe("Refresh Current Source");
      refresh?.click();
      expect(ui.intents).toEqual([
        { kind: "summary-action", presentationId: 73 },
      ]);

      ui.present("Library ready", undefined, "idle");
      expect(ui.entry.hidden).toBe(true);
      expect(ui.notice.hidden).toBe(true);
      expect(ui.notice.textContent).toBe("");
      expect(ui.toggle.getAttribute("aria-expanded")).toBe("false");
      expect(ui.gridEmptyAction.disabled).toBe(false);
    } finally {
      ui.restore();
    }
  });

  test("background overview progress cannot erase an open owning notice", async () => {
    const ui = fixture();
    const scheduled: Array<{
      active: boolean;
      run: () => void | Promise<void>;
    }> = [];
    let overviewRequests = 0;
    const publication = "publication-1";
    const owner = createApplicationOwner(
      (input) => {
        if (input === "/api/overview") {
          overviewRequests += 1;
          return Promise.resolve(
            new Response(
              JSON.stringify({
                published: true,
                publication,
                photoCount: 0,
                albums: [],
                scan: {
                  state: overviewRequests === 1 ? "idle" : "applying",
                  publication,
                },
              }),
            ),
          );
        }
        return Promise.resolve(
          new Response(JSON.stringify({ state: "failed", publication })),
        );
      },
      {
        schedule: (_delay, run) => {
          const task = { active: true, run };
          scheduled.push(task);
          return () => {
            task.active = false;
          };
        },
        emit: (event) => {
          if (event.kind === "summary")
            controller.presentSummary(event.summary);
        },
      },
    );
    const controller = createApplicationPresentationController(
      owner,
      new RecoveryGate(),
      {
        isAlive: () => true,
        isConnectionEstablished: () => true,
        presentSummary: (text, action, state) =>
          ui.present(
            text,
            action
              ? {
                  kind: action.action.kind,
                  presentationId: action.presentationId,
                }
              : undefined,
            state,
          ),
        syncConnection: () => {},
        setConnected: () => {},
      },
    );
    try {
      await owner.refreshOverview();
      const poll = scheduled.find((task) => task.active);
      if (!poll) throw new Error("expected Library status poll");
      poll.active = false;
      await poll.run();
      expect(ui.entry.hidden).toBe(false);
      ui.toggle.click();
      const before = ui.notice.textContent;
      const retry = ui.notice.querySelector("button");
      expect(before).toContain("Library check failed");
      expect(retry?.textContent).toBe("Retry Library Check");

      expect(await owner.refreshOverview()).toBe(true);
      expect(overviewRequests).toBe(2);
      expect(owner.overview?.scan.state).toBe("applying");
      expect(ui.entry.hidden).toBe(false);
      expect(ui.notice.hidden).toBe(false);
      expect(ui.toggle.getAttribute("aria-expanded")).toBe("true");
      expect(ui.notice.textContent).toBe(before);
      expect(ui.notice.querySelector("button")).toBe(retry);
      retry?.click();
      expect(ui.intents).toHaveLength(1);
      expect(ui.intents[0]?.kind).toBe("summary-action");
    } finally {
      controller.dispose();
      owner.dispose();
      ui.restore();
    }
  });
});

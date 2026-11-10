import { describe, expect, test } from "bun:test";
import {
  installLaunchTargetHandler,
  type LaunchTargetWindow,
} from "./launch-target.js";

function harness(href: string) {
  const navigations: string[] = [];
  let consumer:
    | ((parameters: Readonly<{ targetURL?: string }>) => void)
    | undefined;
  const target: LaunchTargetWindow = {
    location: {
      href,
      origin: new URL(href).origin,
      assign: (url) => navigations.push(url),
    },
    launchQueue: {
      setConsumer: (next) => {
        consumer = next;
      },
    },
  };
  installLaunchTargetHandler(target, (url) => navigations.push(url));
  return {
    navigations,
    launch: (targetURL?: string) =>
      consumer?.(targetURL === undefined ? {} : { targetURL }),
  };
}

describe("installed application launch target", () => {
  test("ignores an ordinary start-URL relaunch", () => {
    const app = harness("https://photos.example.test/?source=album");
    app.launch("https://photos.example.test/");
    expect(app.navigations).toEqual([]);
  });

  test("navigates to a root-relative shortcut target", () => {
    const app = harness("https://photos.example.test/?source=album");
    app.launch("/?selection=picked");
    expect(app.navigations).toEqual([
      "https://photos.example.test/?selection=picked",
    ]);
  });

  test("navigates to an in-scope deep link", () => {
    const app = harness("https://photos.example.test/");
    app.launch("https://photos.example.test/?source=album&albumId=abc");
    expect(app.navigations).toEqual([
      "https://photos.example.test/?source=album&albumId=abc",
    ]);
  });

  test("ignores a target outside the application origin", () => {
    const app = harness("https://photos.example.test/");
    app.launch("https://elsewhere.example.test/?selection=picked");
    expect(app.navigations).toEqual([]);
  });

  test("ignores a target that is already current", () => {
    const app = harness("https://photos.example.test/?selection=picked");
    app.launch("https://photos.example.test/?selection=picked");
    expect(app.navigations).toEqual([]);
  });

  test("does nothing when the Launch Handler API is unavailable", () => {
    expect(() =>
      installLaunchTargetHandler({
        location: {
          href: "https://photos.example.test/",
          origin: "https://photos.example.test",
          assign: () => {},
        },
      }),
    ).not.toThrow();
  });
});

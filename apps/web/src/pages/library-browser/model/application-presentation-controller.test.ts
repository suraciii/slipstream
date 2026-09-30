import { describe, expect, test } from "bun:test";
import {
  createApplicationPresentationController,
  type ApplicationPresentationHooks,
} from "./application-presentation-controller.js";
import type {
  ApplicationOwner,
  ApplicationSummaryAction,
} from "./application-owner.js";
import { RecoveryGate } from "./async-ownership.js";

const action = (
  kind: ApplicationSummaryAction["kind"],
): ApplicationSummaryAction => ({ kind }) as ApplicationSummaryAction;

function fakeApplication(): ApplicationOwner {
  return {
    activateSummaryAction: (value) =>
      value.kind === "refresh-current-source"
        ? { kind: "refresh-current-source" }
        : undefined,
  } as ApplicationOwner;
}

describe("ApplicationPresentationController", () => {
  test("rejects a stale Summary action after newer presentation", () => {
    const rendered: number[] = [];
    const hooks: ApplicationPresentationHooks = {
      isAlive: () => true,
      isConnectionEstablished: () => true,
      presentSummary: (_text, summary) => {
        if (summary) rendered.push(summary.presentationId);
      },
      setConnected: () => {},
      syncConnection: () => {},
    };
    const controller = createApplicationPresentationController(
      fakeApplication(),
      new RecoveryGate(),
      hooks,
    );
    controller.presentSummary({
      text: "first",
      action: action("refresh-current-source"),
    });
    const first = rendered[0]!;
    controller.presentSummary({ text: "newer" });
    expect(controller.activateAction(first)).toBeUndefined();
  });

  test("does not repeat transport transitions", () => {
    let connected = 0;
    let synced = 0;
    const hooks: ApplicationPresentationHooks = {
      isAlive: () => true,
      isConnectionEstablished: () => connected > 0,
      presentSummary: () => {},
      setConnected: () => {
        connected += 1;
      },
      syncConnection: () => {
        synced += 1;
      },
    };
    const gate = new RecoveryGate();
    const controller = createApplicationPresentationController(
      fakeApplication(),
      gate,
      hooks,
    );
    controller.coordinate({ kind: "transport-lost" });
    expect(gate.transportReachable).toBe(false);
    controller.coordinate({ kind: "transport-lost" });
    controller.coordinate({ kind: "mark-reachable" });
    controller.coordinate({ kind: "mark-reachable" });
    expect(gate.transportReachable).toBe(true);
    expect(synced).toBe(1);
    expect(connected).toBe(1);
  });
});

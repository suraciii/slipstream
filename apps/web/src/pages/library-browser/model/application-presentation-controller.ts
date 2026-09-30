import type {
  ApplicationCoordination,
  ApplicationOwner,
  ApplicationPresentation,
  ApplicationRecovery,
  ApplicationSummary,
  ApplicationSummaryAction,
} from "./application-owner.js";
import type { RecoveryClaim, RecoveryGate } from "./async-ownership.js";

export type SummaryActionPresentation = Readonly<{
  presentationId: number;
  action: ApplicationSummaryAction;
}>;

export type ApplicationPresentationHooks = Readonly<{
  isAlive: () => boolean;
  isConnectionEstablished: () => boolean;
  presentSummary: (
    text: string,
    action: SummaryActionPresentation | undefined,
    state: ApplicationSummary["libraryCheckState"],
  ) => void;
  syncConnection: () => void;
  setConnected: () => void;
}>;

export interface ApplicationPresentationController {
  readonly scanPhase: string;
  presentSummary(
    summary: Extract<ApplicationPresentation, { kind: "summary" }>["summary"],
  ): void;
  activateAction(
    presentationId: number,
  ): Readonly<{ kind: "refresh-current-source" }> | undefined;
  coordinate(coordination: ApplicationCoordination): void;
  dispose(): void;
}

export function createApplicationPresentationController(
  application: ApplicationOwner,
  recoveryGate: RecoveryGate,
  hooks: ApplicationPresentationHooks,
): ApplicationPresentationController {
  const recoveries = new Map<ApplicationRecovery, RecoveryClaim>();
  let nextPresentationId = 0;
  let currentAction: SummaryActionPresentation | undefined;
  let scanPhase = "";
  let closed = false;

  const presentSummary = (
    summary: Extract<ApplicationPresentation, { kind: "summary" }>["summary"],
  ): void => {
    if (closed || !hooks.isAlive()) return;
    const presentationId = ++nextPresentationId;
    currentAction = summary.action
      ? { presentationId, action: summary.action }
      : undefined;
    scanPhase = summary.libraryCheckState === "active" ? summary.text : "";
    hooks.presentSummary(
      summary.text,
      currentAction,
      summary.libraryCheckState,
    );
  };
  const activateAction = (
    presentationId: number,
  ): Readonly<{ kind: "refresh-current-source" }> | undefined => {
    if (
      closed ||
      !currentAction ||
      currentAction.presentationId !== presentationId
    )
      return undefined;
    return application.activateSummaryAction(currentAction.action);
  };
  const coordinate = (coordination: ApplicationCoordination): void => {
    if (closed || !hooks.isAlive()) return;
    if (coordination.kind === "mark-reachable") {
      if (hooks.isConnectionEstablished() && recoveryGate.transportReachable)
        return;
      recoveryGate.markReachable();
      hooks.setConnected();
      return;
    }
    if (coordination.kind === "transport-lost") {
      if (!recoveryGate.transportReachable) return;
      recoveryGate.markTransportLost();
      hooks.syncConnection();
      return;
    }
    if (coordination.kind === "fail-application-recovery") {
      let claim = recoveries.get(coordination.recovery);
      if (!claim) {
        claim = recoveryGate.issue(
          coordination.slot,
          coordination.slot === "overview-reload" ? "overview" : "library",
        );
        recoveries.set(coordination.recovery, claim);
      }
      if (!recoveryGate.fail(claim, { transportLost: true }))
        recoveryGate.discard(claim);
      hooks.syncConnection();
      return;
    }
    if (coordination.kind === "recover") {
      const claim = recoveries.get(coordination.recovery);
      if (claim) recoveryGate.recover(claim);
      recoveries.delete(coordination.recovery);
      hooks.syncConnection();
    }
  };
  return {
    get scanPhase() {
      return scanPhase;
    },
    presentSummary,
    activateAction,
    coordinate,
    dispose() {
      closed = true;
      recoveries.clear();
      currentAction = undefined;
      scanPhase = "";
    },
  };
}

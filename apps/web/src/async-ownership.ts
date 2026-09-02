export type TaskKey = string;

export interface TaskLease {
  readonly key: TaskKey;
  readonly signal: AbortSignal | undefined;
  isCurrent(): boolean;
  onCleanup(cleanup: () => void): void;
  finish(): void;
}

export interface OrderedTaskLease extends TaskLease {
  readonly sequence: number;
  readonly floor: number;
  commit(currentFloor: number): boolean;
}

type TaskRecord = {
  key: TaskKey;
  active: boolean;
  controller: AbortController | undefined;
  cleanups: Array<() => void>;
  cancellations: Array<() => void>;
};

type OrderedState = {
  next: number;
  committedFloor: number | undefined;
  committedSequence: number;
};

type SharedTaskRecord = TaskRecord & {
  promise: Promise<unknown>;
};

export interface SharedTask<T> {
  readonly promise: Promise<T>;
  readonly signal: AbortSignal | undefined;
  readonly started: boolean;
  isCurrent(): boolean;
}

export class TaskScope {
  readonly #latest = new Map<TaskKey, TaskRecord>();
  readonly #ordered = new Map<TaskKey, OrderedState>();
  readonly #shared = new Map<TaskKey, SharedTaskRecord>();
  readonly #records = new Set<TaskRecord>();
  #halted = false;

  beginLatest(key: TaskKey, options: { abortTransport: boolean }): TaskLease {
    this.#assertOpen();
    const prior = this.#latest.get(key);
    if (prior) this.#cancel(prior);
    const record: TaskRecord = {
      key,
      active: true,
      controller: options.abortTransport ? new AbortController() : undefined,
      cleanups: [],
      cancellations: [],
    };
    this.#latest.set(key, record);
    this.#records.add(record);
    return this.#lease(record, () => this.#latest.get(key) === record);
  }

  current(key: TaskKey): TaskLease | undefined {
    const record = this.#latest.get(key);
    if (!record?.active) return undefined;
    return this.#lease(record, () => this.#latest.get(key) === record);
  }

  beginOrdered(key: TaskKey, floor: number): OrderedTaskLease {
    this.#assertOpen();
    const state = this.#ordered.get(key) ?? {
      next: 0,
      committedFloor: undefined,
      committedSequence: 0,
    };
    this.#ordered.set(key, state);
    const sequence = (state.next += 1);
    const record: TaskRecord = {
      key,
      active: true,
      controller: undefined,
      cleanups: [],
      cancellations: [],
    };
    this.#records.add(record);
    const lease = this.#lease(record, () => record.active);
    return {
      ...lease,
      sequence,
      floor,
      commit: (currentFloor: number) => {
        if (!lease.isCurrent() || currentFloor !== floor) return false;
        if (
          state.committedFloor !== undefined &&
          (floor < state.committedFloor ||
            (floor === state.committedFloor &&
              sequence <= state.committedSequence))
        )
          return false;
        state.committedFloor = floor;
        state.committedSequence = sequence;
        return true;
      },
    };
  }

  joinOrStart<T>(
    key: TaskKey,
    options: {
      abortTransport: boolean;
      cleanup?: () => void;
      onCancel: () => T;
    },
    start: (signal: AbortSignal | undefined) => Promise<T>,
  ): SharedTask<T> {
    this.#assertOpen();
    const existing = this.#shared.get(key);
    if (existing?.active) {
      return {
        promise: existing.promise as Promise<T>,
        signal: existing.controller?.signal,
        started: false,
        isCurrent: () =>
          !this.#halted &&
          existing.active &&
          this.#shared.get(key) === existing,
      };
    }
    const prior = this.#latest.get(key);
    if (prior) this.#cancel(prior);
    const record: SharedTaskRecord = {
      key,
      active: true,
      controller: options.abortTransport ? new AbortController() : undefined,
      cleanups: options.cleanup ? [options.cleanup] : [],
      cancellations: [],
      promise: Promise.resolve(undefined),
    };
    this.#latest.set(key, record);
    this.#shared.set(key, record);
    this.#records.add(record);
    const lease = this.#lease(record, () => this.#shared.get(key) === record);
    const cancelled = new Promise<T>((resolve) => {
      record.cancellations.push(() => resolve(options.onCancel()));
    });
    const operation = Promise.resolve().then(() =>
      start(record.controller?.signal),
    );
    record.promise = Promise.race([operation, cancelled]).finally(() =>
      lease.finish(),
    );
    return {
      promise: record.promise as Promise<T>,
      signal: record.controller?.signal,
      started: true,
      isCurrent: () => lease.isCurrent(),
    };
  }

  halt(): void {
    if (this.#halted) return;
    this.#halted = true;
    for (const record of [...this.#records]) this.#cancel(record);
    this.#latest.clear();
  }

  get halted(): boolean {
    return this.#halted;
  }

  #lease(record: TaskRecord, ownsKey: () => boolean): TaskLease {
    return {
      key: record.key,
      signal: record.controller?.signal,
      isCurrent: () => !this.#halted && record.active && ownsKey(),
      onCleanup: (cleanup) => {
        if (record.active && !this.#halted) record.cleanups.push(cleanup);
        else cleanup();
      },
      finish: () => this.#finish(record),
    };
  }

  #assertOpen(): void {
    if (this.#halted) throw new Error("Task scope is halted");
  }

  #finish(record: TaskRecord): void {
    if (!record.active) return;
    record.active = false;
    if (this.#latest.get(record.key) === record)
      this.#latest.delete(record.key);
    if (this.#shared.get(record.key) === record)
      this.#shared.delete(record.key);
    this.#records.delete(record);
    record.cancellations.splice(0);
    this.#cleanup(record);
  }

  #cancel(record: TaskRecord): void {
    if (!record.active) return;
    record.controller?.abort();
    const cancellations = record.cancellations.splice(0);
    for (const cancel of cancellations) cancel();
    this.#finish(record);
  }

  #cleanup(record: TaskRecord): void {
    const cleanups = record.cleanups.splice(0);
    for (const cleanup of cleanups) cleanup();
  }
}

export interface SettlementHandle {
  readonly sequence: number;
  readonly admissionKey: string | undefined;
  isNewest(): boolean;
  ownsSurface(): boolean;
  canPresent(): boolean;
  finish(): void;
}

type SettlementRecord = {
  sequence: number;
  admissionKey: string | undefined;
  ownsSurface: () => boolean;
  active: boolean;
};

export class SettlementFamily {
  #next = 0;
  #newest = 0;
  #closed = false;
  readonly #admissions = new Map<string, SettlementRecord>();

  begin(options: {
    admissionKey?: string;
    ownsSurface?: () => boolean;
  }): SettlementHandle | undefined {
    if (this.#closed) return undefined;
    if (
      options.admissionKey !== undefined &&
      this.#admissions.has(options.admissionKey)
    )
      return undefined;
    const record: SettlementRecord = {
      sequence: (this.#next += 1),
      admissionKey: options.admissionKey,
      ownsSurface: options.ownsSurface ?? (() => true),
      active: true,
    };
    this.#newest = record.sequence;
    if (record.admissionKey !== undefined)
      this.#admissions.set(record.admissionKey, record);
    return {
      sequence: record.sequence,
      admissionKey: record.admissionKey,
      isNewest: () => record.active && record.sequence === this.#newest,
      ownsSurface: () => record.active && !this.#closed && record.ownsSurface(),
      canPresent: () =>
        record.active &&
        !this.#closed &&
        record.sequence === this.#newest &&
        record.ownsSurface(),
      finish: () => {
        if (!record.active) return;
        record.active = false;
        if (
          record.admissionKey !== undefined &&
          this.#admissions.get(record.admissionKey) === record
        )
          this.#admissions.delete(record.admissionKey);
      },
    };
  }

  isAdmitted(admissionKey: string): boolean {
    return this.#admissions.has(admissionKey);
  }

  closePresentation(): void {
    this.#closed = true;
  }
}

export type NoticePriority = number;
export type NoticeResult = "applied" | "blocked" | "stale";

export interface NoticeHandle {
  readonly ticket: number;
  readonly kind: string;
  readonly key: string;
  readonly priority: NoticePriority;
}

export interface BackgroundEpoch {
  readonly ticket: number;
}

export interface NoticeUpdate<T> {
  readonly result: NoticeResult;
  readonly visible?: T | null;
}

type NoticeRecord<T> = {
  handle: NoticeHandle;
  payload: T;
  fallback: boolean;
};

export class SummaryNoticeChannel<T> {
  readonly #handles = new Set<NoticeHandle>();
  readonly #epochs = new Set<BackgroundEpoch>();
  readonly #pending = new Map<string, NoticeRecord<T>>();
  #next = 0;
  #barrier = 0;
  #lastBackground = 0;
  #active: NoticeRecord<T> | undefined;
  #closed = false;

  issue(kind: string, key: string, priority: NoticePriority): NoticeHandle {
    const handle = Object.freeze({
      ticket: (this.#next += 1),
      kind,
      key,
      priority,
    });
    if (!this.#closed) this.#handles.add(handle);
    return handle;
  }

  backgroundEpoch(): BackgroundEpoch {
    const epoch = Object.freeze({ ticket: (this.#next += 1) });
    if (!this.#closed) this.#epochs.add(epoch);
    return epoch;
  }

  present(
    handle: NoticeHandle,
    payload: T,
    options: { fallback?: boolean } = {},
  ): NoticeUpdate<T> {
    if (!this.#valid(handle)) return { result: "stale" };
    const record = { handle, payload, fallback: options.fallback ?? false };
    if (!this.#active) {
      this.#active = record;
      return { result: "applied", visible: payload };
    }
    if (this.#same(this.#active.handle, handle)) {
      this.#active = record;
      return { result: "applied", visible: payload };
    }
    if (this.#wins(handle, this.#active.handle)) {
      this.#rememberPending(this.#active);
      this.#active = record;
      return { result: "applied", visible: payload };
    }
    this.#rememberPending(record);
    return { result: "blocked" };
  }

  release(handle: NoticeHandle): NoticeUpdate<T> {
    if (!this.#valid(handle)) return { result: "stale" };
    if (this.#active && this.#same(this.#active.handle, handle)) {
      this.#handles.delete(handle);
      this.#active = undefined;
      return this.#revealPending();
    }
    const pending = this.#pending.get(handle.kind);
    if (pending && this.#same(pending.handle, handle)) {
      this.#pending.delete(handle.kind);
      this.#handles.delete(handle);
      return { result: "applied" };
    }
    this.#handles.delete(handle);
    return { result: "applied" };
  }

  releaseKind(kind: string, authorizer: NoticeHandle): NoticeUpdate<T> {
    if (!this.#valid(authorizer) || authorizer.kind !== kind)
      return { result: "stale" };
    const pending = this.#pending.get(kind);
    if (pending && pending.handle.ticket <= authorizer.ticket) {
      this.#handles.delete(pending.handle);
      this.#pending.delete(kind);
    }
    if (
      this.#active?.handle.kind === kind &&
      this.#active.handle.ticket <= authorizer.ticket
    ) {
      this.#handles.delete(this.#active.handle);
      this.#handles.delete(authorizer);
      this.#active = undefined;
      return this.#revealPending();
    }
    this.#handles.delete(authorizer);
    return { result: "applied" };
  }

  beginBarrier(
    kind: string,
    key: string,
    priority: NoticePriority,
    payload: T,
  ): { handle: NoticeHandle; update: NoticeUpdate<T> } {
    const handle = this.issue(kind, key, priority);
    this.#barrier = handle.ticket;
    for (const issued of [...this.#handles])
      if (issued.ticket < this.#barrier) this.#handles.delete(issued);
    for (const epoch of [...this.#epochs])
      if (epoch.ticket < this.#barrier) this.#epochs.delete(epoch);
    this.#active = undefined;
    this.#pending.clear();
    const update = this.present(handle, payload);
    return { handle, update };
  }

  discardBackground(epoch: BackgroundEpoch): NoticeResult {
    if (this.#closed || !this.#epochs.delete(epoch)) return "stale";
    return epoch.ticket < this.#barrier ? "stale" : "applied";
  }

  presentBackground(epoch: BackgroundEpoch, payload: T): NoticeUpdate<T> {
    if (this.#closed || !this.#epochs.has(epoch)) return { result: "stale" };
    this.#epochs.delete(epoch);
    if (
      epoch.ticket < this.#barrier ||
      epoch.ticket < this.#lastBackground ||
      this.#active
    )
      return { result: "stale" };
    this.#lastBackground = epoch.ticket;
    return { result: "applied", visible: payload };
  }

  close(): void {
    this.#closed = true;
    this.#handles.clear();
    this.#epochs.clear();
    this.#pending.clear();
    this.#active = undefined;
  }

  get activeHandle(): NoticeHandle | undefined {
    return this.#active?.handle;
  }

  get reloadBarrierTicket(): number {
    return this.#barrier;
  }

  #valid(handle: NoticeHandle): boolean {
    return (
      !this.#closed &&
      this.#handles.has(handle) &&
      handle.ticket >= this.#barrier
    );
  }

  #same(left: NoticeHandle, right: NoticeHandle): boolean {
    return left === right;
  }

  #wins(candidate: NoticeHandle, current: NoticeHandle): boolean {
    return (
      candidate.priority > current.priority ||
      (candidate.priority === current.priority &&
        candidate.ticket > current.ticket)
    );
  }

  #rememberPending(record: NoticeRecord<T>): void {
    if (!record.fallback || !this.#valid(record.handle)) return;
    const current = this.#pending.get(record.handle.kind);
    if (!current || record.handle.ticket > current.handle.ticket)
      this.#pending.set(record.handle.kind, record);
  }

  #revealPending(): NoticeUpdate<T> {
    const next = [...this.#pending.values()]
      .filter((record) => this.#valid(record.handle))
      .sort(
        (left, right) =>
          right.handle.priority - left.handle.priority ||
          right.handle.ticket - left.handle.ticket,
      )[0];
    if (!next) return { result: "applied", visible: null };
    this.#pending.delete(next.handle.kind);
    this.#active = next;
    return { result: "applied", visible: next.payload };
  }
}

export type RecoveryScope = "source" | "photo";

export interface RecoveryOwner {
  readonly scope: RecoveryScope;
  readonly generation: string;
}

export interface RecoveryClaim {
  readonly ticket: number;
  readonly kind: string;
  readonly key: string;
  readonly owner: RecoveryOwner | undefined;
  readonly blocking: boolean;
}

export interface RecoveryTransition {
  readonly ticket: number;
  readonly scope: RecoveryScope;
  readonly next: string;
}

type TransitionState = {
  predecessors: Set<string>;
};

export class RecoveryGate {
  readonly #issued = new Set<RecoveryClaim>();
  readonly #active = new Set<RecoveryClaim>();
  readonly #owners = new Map<RecoveryScope, string>();
  readonly #transitions = new Map<RecoveryTransition, TransitionState>();
  readonly #claimTransitions = new Map<RecoveryClaim, RecoveryTransition>();
  #next = 0;
  #reachable = true;
  #closed = false;

  setOwner(scope: RecoveryScope, generation: string): void {
    if (this.#closed) return;
    this.#owners.set(scope, generation);
  }

  issue(
    kind: string,
    key: string,
    options: {
      owner?: RecoveryOwner;
      blocking?: boolean;
      transition?: RecoveryTransition;
    } = {},
  ): RecoveryClaim {
    if (
      options.transition &&
      (!this.#transitionCurrent(options.transition) ||
        options.owner?.scope !== options.transition.scope ||
        options.owner.generation !== options.transition.next)
    )
      throw new Error(
        "Recovery claim does not belong to the current transition",
      );
    const claim = Object.freeze({
      ticket: (this.#next += 1),
      kind,
      key,
      owner: options.owner,
      blocking: options.blocking ?? true,
    });
    if (!this.#closed) {
      this.#issued.add(claim);
      if (options.transition)
        this.#claimTransitions.set(claim, options.transition);
    }
    return claim;
  }

  fail(
    claim: RecoveryClaim,
    options: { transportLost?: boolean } = {},
  ): boolean {
    if (!this.#canAffectCurrent(claim)) return false;
    this.#active.add(claim);
    if (options.transportLost) this.#reachable = false;
    return true;
  }

  recover(claim: RecoveryClaim): boolean {
    if (!this.#canAffectCurrent(claim) || !this.#active.has(claim))
      return false;
    this.#retireClaim(claim);
    this.#reachable = true;
    return true;
  }

  discard(claim: RecoveryClaim): boolean {
    if (this.#closed || !this.#issued.has(claim) || this.#active.has(claim))
      return false;
    this.#retireClaim(claim);
    return true;
  }

  markReachable(): void {
    if (!this.#closed) this.#reachable = true;
  }

  beginTransition(
    scope: RecoveryScope,
    nextGeneration: string,
  ): RecoveryTransition {
    if (this.#closed) throw new Error("Recovery gate is closed");
    const predecessors = new Set<string>();
    for (const [transition, state] of this.#transitions)
      if (transition.scope === scope)
        for (const generation of state.predecessors)
          predecessors.add(generation);
    const prior = this.#owners.get(scope);
    if (prior !== undefined) predecessors.add(prior);
    const transition = Object.freeze({
      ticket: (this.#next += 1),
      scope,
      next: nextGeneration,
    });
    this.#transitions.set(transition, { predecessors });
    this.#owners.set(scope, nextGeneration);
    return transition;
  }

  succeedTransition(transition: RecoveryTransition): boolean {
    if (!this.#transitionCurrent(transition)) return false;
    this.#settleTransition(transition);
    this.#reachable = true;
    return true;
  }

  failTransition(
    transition: RecoveryTransition,
    replacement: RecoveryClaim,
    options: { transportLost?: boolean } = {},
  ): boolean {
    if (
      !this.#transitionCurrent(transition) ||
      !this.#issued.has(replacement) ||
      this.#claimTransitions.get(replacement) !== transition ||
      replacement.owner?.scope !== transition.scope ||
      replacement.owner.generation !== transition.next
    )
      return false;
    this.#settleTransition(transition, replacement);
    this.#active.add(replacement);
    if (options.transportLost) this.#reachable = false;
    return true;
  }

  close(): void {
    this.#closed = true;
    this.#issued.clear();
    this.#active.clear();
    this.#owners.clear();
    this.#transitions.clear();
    this.#claimTransitions.clear();
  }

  get transportReachable(): boolean {
    return this.#reachable;
  }

  get decisionReady(): boolean {
    return (
      !this.#closed &&
      this.#reachable &&
      ![...this.#active].some((claim) => claim.blocking)
    );
  }

  isActive(claim: RecoveryClaim): boolean {
    return this.#active.has(claim);
  }

  #canAffectCurrent(claim: RecoveryClaim): boolean {
    if (this.#closed || !this.#issued.has(claim)) return false;
    if (!claim.owner) return true;
    return this.#owners.get(claim.owner.scope) === claim.owner.generation;
  }

  #transitionCurrent(transition: RecoveryTransition): boolean {
    return (
      !this.#closed &&
      this.#transitions.has(transition) &&
      this.#owners.get(transition.scope) === transition.next
    );
  }

  #settleTransition(
    transition: RecoveryTransition,
    keep?: RecoveryClaim,
  ): void {
    const state = this.#transitions.get(transition)!;
    for (const claim of [...this.#issued]) {
      const predecessor =
        claim.owner?.scope === transition.scope &&
        state.predecessors.has(claim.owner.generation);
      const establishment = this.#claimTransitions.get(claim) === transition;
      if (claim !== keep && (predecessor || establishment))
        this.#retireClaim(claim);
    }
    for (const issued of [...this.#transitions.keys()])
      if (issued.scope === transition.scope) this.#transitions.delete(issued);
    for (const [claim, owner] of [...this.#claimTransitions])
      if (owner.scope === transition.scope && claim !== keep)
        this.#claimTransitions.delete(claim);
  }

  #retireClaim(claim: RecoveryClaim): void {
    this.#active.delete(claim);
    this.#issued.delete(claim);
    this.#claimTransitions.delete(claim);
  }
}

# Web Async Ownership

The Web client runs many asynchronous operations whose settlements race the
user: two Album mutations overlap, an overview response lands after a newer
one, a Folder window arrives after a rescan. Every such operation must answer
one question when it settles: _may I still write what I know?_ Until now each
feature answered it with a private mechanism (status epochs, action sequence
numbers, notice markers, commit counters, in-flight key sets). The rules were
similar but not identical, and every new asynchronous feature risked inventing
another one.

## Design Drivers

- Settlement correctness is a product requirement, not polish: a late
  response must never revert newer Album state, overwrite a newer status, or
  disconnect a UI a newer action already restored. Twelve P1 findings across
  #97's four review rounds all lived on this family.
- Admitted persistence is never aborted: once a mutation request is sent, the
  server may commit it; aborting the fetch blinds the client without rolling
  the server back. Photo and Album persistence keeps this existing contract.
- Publication and snapshot coherence forbid eventual consistency at the view
  layer: the browser loads one coherent generation and never reinterprets a
  stale Folder open silently.
- The client is a single dependency-free script. Introducing a framework or a
  state-management library for this problem exceeds it.

## Model

Every asynchronous operation belongs to exactly one **family**, and every
family is one of two kinds.

A **read family** produces presentation data only: overview refreshes, File
Location windows, Browse snapshots and windows, previews, thumbnails. A read
family owns a **scope**. Starting a new operation in a scope halts the
previous one: its `fetch` is aborted with an `AbortSignal`, its settlement
code never runs, and its cleanup runs once. A late response from a halted
operation is physically absent, not logically filtered. When the UI thing
that owns a scope goes away (source switched, photo closed, panel reset), the
scope halts with it.

A **write family** mutates persisted state: Album create/rename/delete,
membership add/remove, photo selection/rating, saved-position writes. A write
operation, once sent, always runs to settlement. Within a family the newest
operation owns settlement: a superseded write presents no success notice (the
refreshed shared data is the confirmation), and a superseded failure surfaces
in the Library summary unless a newer write already reported there. A write
never aborts, and a write's failure never changes global connectivity unless
it is still the newest operation of its family.

**Shared data** (the overview: photo count, scan state, Album summaries)
converges to the server in commit order. A refresh response applies only if
it is newer than the last committed refresh; a failed refresh never discards
an older success. Cancellation cannot replace this ordering — responses
arrive out of order even when nothing is cancelled — so the ordering rule
stands on its own.

## Concrete Rules

Each rule is stated as the behavior an observer can check.

- Opening a source halts the read families of the previous source. A Folder
  window response from the old source never renders into the new one.
- Expanding the same Folder parent twice keeps only the newest request; the
  first response is discarded because the request was aborted.
- A slow overview response that resolves after a newer refresh never reverts
  Album names, counts, headings, or the retry source.
- The Library summary keeps a write-failure notice until a newer write in the
  same family settles or the user deliberately reloads; background status
  writes do not erase it. Actionable recovery messages (Folder rescan
  notices, failed-range retry controls) retake the channel because they ask
  the user to act now.
- A superseded write's transport failure reports as a notice but leaves the
  connection state owned by the newest operation.
- An in-flight write disables its initiating control across re-rendings, and
  re-invoking the same write (same Album and Photo) while in flight is a
  no-op; a write against a different Album is independent.
- A removed member stays non-removable within the open snapshot, and becomes
  removable again after re-adding to that Album or reopening the source.
- The Folder/source generation family (browse-token lifecycles, publication
  rebinding) is separate cancellation semantics, not ownership: it keeps its
  existing rules unchanged.

## Options Considered

Adopt a query-cache library (TanStack Query and peers). Rejected: the
client's protocol is bespoke (immutable browse tokens, publication-coherent
generations, bounded windows) and the library explicitly does not solve
mutation races, which is where our complexity lives.

Adopt a full state machine (XState and peers). Rejected: one client file does
not need an actor runtime; the rules above fit a scope primitive plus one
settlement rule.

Keep the per-feature coordinators. Rejected: they encode the same decision
five ways; each future asynchronous feature would grow a sixth.

## Scope Module Contract

The implementation is a dependency-free module exposing the industry pattern
(Effection scopes, Swift `task(id:)`): a scope holds child operations; halting
cancels children via `AbortSignal`, runs their cleanup exactly once, and
rejects settlements that already finished. Reads take a scope and a key;
writes take a family key and return a settlement handle whose `present` and
`announce` enforce the rules above. The module owns no UI knowledge; surfaces
and summaries are passed in.

Migration replaces one coordinator at a time with the scope or settlement
rule it approximates, with the full browser suite green after each step, and
removes each retired mechanism in the same change.

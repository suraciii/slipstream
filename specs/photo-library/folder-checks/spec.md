# Folder Freshness

A large Library MUST stay browsable when external changes require substantial content verification.

Folder freshness is local: a Folder with unresolved file identity work reports **Pending**. Other Folders remain ordinary.

## Automatic Library Check

An automatic Library Check MUST discover supported Original Files, inspect known Capture Time facts, and publish safe changes.

A discovered file is a [Pending Candidate](../../../CONTEXT.md#library) when its kind and size could match a remembered Content Fingerprint for an unavailable or changed Original.

The check MUST establish Pending Candidates without reading their content.

An automatically started check MUST verify Pending Candidates only within the Automatic Verification Budget.

When the candidate work exceeds that budget, the check MUST defer every candidate to a [Folder Check](../../../CONTEXT.md#library).

A check within budget MUST verify all candidates before publication.

A Pending Candidate MUST NOT become a new Photo before its Folder Check.

A changed Original that becomes a Pending Candidate MUST keep its last published facts and availability.

A missing Original whose candidate is deferred MUST remain unavailable.

A new supported file that cannot match any remembered Content Fingerprint MUST publish normally.

A Folder that contains only deferred candidates MUST appear as a Pending Folder even when it has no published Photo.

## Pending Folder Presentation

A normal Folder MUST NOT show pending UI.

The Folder tree MUST mark a Pending Folder and its ancestor Folders with a small local marker.

The marker MUST NOT use a page-wide banner, dialog, or Library-wide warning.

Opening a Pending Folder MUST show one local strip above its Grid.

The strip MUST name the number of files that need checking and offer **Check folder**.

The Pending Folder MUST remain browsable with its last published Photos.

Published safe additions in a Pending Folder remain visible.

Pending candidates and deferred changed files MUST NOT appear as current Photos until verified.

Leaving a Pending Folder MUST NOT lose its pending state.

## Folder Check

**Check folder** starts one Folder Check for the open Original Folder subtree.

A Folder Check MUST verify every Pending Candidate whose content could affect the selected Folder's candidates.

That scope includes candidates outside the Folder when their kind and size could make an automatic match ambiguous.

A Folder Check MUST NOT verify candidates that cannot affect the selected Folder.

A Folder Check MUST report progress in the selected Folder with the existing measured phase and counts.

A Folder Check MUST publish one atomic replacement of the Published Library.

A Folder Check MUST resolve exact Location Recovery under the existing rules.

A verified candidate that does not identify one unique moved Original MUST follow the existing manual review rules.

A Folder Check MUST clear pending state for resolved candidates only.

Candidates that remain deferred, unreadable, or unrelated stay Pending.

Other Folders MUST retain their Photos and pending state.

Only one Library check or Folder Check MAY run at a time.

A second request MUST NOT start overlapping work.

## Full Library Check

An explicit **Check Library** MUST verify every Pending Candidate without the Automatic Verification Budget.

The automatic startup check uses the Automatic Verification Budget.

An explicit full check and a Folder Check share one Library Check lifecycle and one Published Library authority.

## Failure

A failed folder discovery MUST retain the prior Published Library.

A candidate read failure MUST leave that candidate Pending and MUST NOT overwrite its remembered facts.

A scan-wide failure MUST NOT clear any pending state.

A successful Folder Check MUST NOT clear pending state from a different Folder unless its candidates were resolved.

## Acceptance

- A large external move publishes only safe changes, leaves no duplicate Photo, and marks only affected Folders Pending.
- Checking one Pending Folder resolves its verifiable files, reports local progress, and leaves unrelated Folders untouched.
- A small candidate set verifies automatically and leaves no pending UI.

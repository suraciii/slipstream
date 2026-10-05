# Photo Editing Model

A Photographer or Agent needs to adjust one Photo, inspect the current result, and request an output that another service can consume. The model must make that path direct: one current editing state, guarded changes, bounded Preview, and an immutable Export output.

## Model

**Edit State** is the one current confirmed editing state for a Photo. It contains the selected Processing Engine, its product Controls, the bound input identity, and the current edit revision.

An Edit State is the object used by the Web and the Agent. A successful control change replaces the current state atomically and creates a new revision. The state is independent of Selection State, Rating, Album membership, and camera Preview.

A **Processing Engine** is a service capability such as darktable. An **Engine Module** is an addressable operation inside that engine. A **Control** is a product-defined value owned by an Engine Module, with a defined meaning, validation, and reset behavior.

A **Processing Artifact** is the immutable result of a completed Export. It is the handoff object between services. A downstream service may consume the Artifact as input and owns its own Edit State. Slipstream does not create or manage a cross-service editing pipeline.

Edit Recipe, Processing Step, and native parameter tree are implementation or compatibility terms. They are not additional user or Agent objects. A saved internal snapshot may be used to persist and replay the current Edit State, but it must not become a second daily editing model.

## Behavior

The normal editing flow is:

1. Read the Photo and current Edit State.
2. Change one admitted Control with the observed edit revision.
3. Read the confirmed state after the mutation.
4. Request an Edit Preview when a bounded rendition is needed.
5. Export the confirmed state when an output is requested.
6. Inspect and download the resulting Processing Artifact.

The Agent does not construct a complete engine parameter tree, access a host path, operate an engine catalog, or call a private engine process.

A Preview reads the current Edit State and is marked stale after a later accepted edit. Preview bytes are display renditions and cannot become the input of another service.

An Export captures the confirmed Edit State, input identity, Engine, concrete controls, bundle/schema identity, output contract, and request identity. A successful Export publishes one immutable Processing Artifact. It does not change the Original File or create a new Photo.

A downstream service starts its own Edit State from the explicitly supplied Artifact. It does not read or modify the upstream Photo's Edit State. An upstream edit does not silently retarget an existing downstream Artifact.

## Failure behavior

A missing or unqualified Control, changed source, stale edit revision, incompatible Artifact, unavailable Engine, or failed Export is explicit. The confirmed Edit State and earlier Artifacts remain unchanged.

A lost mutation or Export response is reconciled with the same request identity. The client does not infer success or start a second operation from the absence of a response.

The service never silently substitutes Camera Preview, another Engine, a different Artifact, or a default value for a refused operation.

## Scope

The current product does not add Preset, persistent editing History, Snapshot, virtual copy, batch edit, or a user-visible pipeline. Preset may be added later only for a demonstrated need to reuse Controls across Photos.

The current product does not expose processing-recipe as a primary command. Existing storage or compatibility routes may retain the internal snapshot format during migration, but new product guidance uses Edit State, Control, Processing Engine, Edit Preview, Export, and Processing Artifact.

## Terminology

Use:

- Edit State for the current saved editing state of one Photo.
- Control for a product-defined editable value.
- Processing Engine for a service capability.
- Edit Preview for a bounded rendition of the current state.
- Export for the explicit materialization operation.
- Processing Artifact for an immutable, provenance-bearing output used in service handoff.

Avoid:

- Edit Recipe as a second current-edit object.
- Processing Step as a user-visible workflow stage.
- Recipe as a synonym for Preset, History, Snapshot, or Artifact.
- Latest result as an input identity.

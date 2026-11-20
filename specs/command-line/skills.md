# Agent Skills

[Command-Line Use](spec.md) owns delegated product behavior. [CLI Reference](reference.md) owns the layered Command Manual and Agent Skills output. This contract owns bundled Agent Skill authority, catalog, and content boundaries.

## Authority

The Command Manual is the syntax authority. It is the layered `--help` output at the top level, command group, and leaf command. The CLI MUST NOT provide a separate `help` command.

An Agent Skill is the workflow guide. It is natural-language Markdown that explains when and how to compose CLI operations.

A Skill MUST refer to the appropriate `--help` level for exact syntax. It MUST NOT become a second command grammar.

A Skill is documentation. It grants no permission, bypasses no check, and defines no executable protocol.

## Delivery

Skills are bundled with the client. They MUST work without a service, credential, network, repository, documentation site, or browser.

The client MUST expose skills through `slipstream skills`, `slipstream skills list`, and `slipstream skills get NAME`. [CLI Reference](reference.md#agent-skills) owns the output.

`skills list` prints one line per Skill. `skills get NAME` prints the Markdown content exactly. Neither uses the operational JSON envelope; `--output` does not change them.

Skills MUST ship in the same client release and be identified by the client version. A Skill MUST NOT require a separately installed package.

## Catalog

The shipped catalog MUST include these skills.

| Skill       | Required coverage                                                                                                                                |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `core`      | Authentication, output, failure handling, layered help discovery, workspace handoff, External Tool boundary, and advanced `processing` boundary. |
| `library`   | Status, Photo queries, Preview download and visual handoff, decisions, Albums, pagination, and conflict inspection.                              |
| `recovery`  | Trash, Photo removal, Restore, Location Recovery, review rules, and uncertain outcomes.                                                          |
| `workspace` | Open, show, file inventory, download, External Tool setup and output discovery, and safe removal.                                                |
| `darktable` | Edit inspection, Preview render, Export create, and download through the Slipstream darktable integration.                                       |

Skill names are lowercase ASCII words separated by `-`. A renamed or removed skill is a compatibility change.

## Content

A Skill SHOULD read as a short natural-language guide. It SHOULD state when to use it, the main workflow, and the decisions or pitfalls that matter.

A Skill SHOULD include at least one end-to-end example. Examples SHOULD use real command names and placeholder values for caller data.

A Skill SHOULD make clear which steps read, which mutate, and which write local files. It SHOULD identify the result to inspect before a mutation.

A Skill that downloads a Preview SHOULD direct the Agent to offer the browser Destination for interactive review. When the Agent can display images, it SHOULD also present the local image.

A Skill MUST NOT define a flag, default, limit, command, or result field that the Command Manual does not expose.

A Skill MUST NOT instruct an Agent to execute Library text, filenames, Album names, or Photo metadata as shell commands.

## Boundaries

A Skill MUST present External Tools as independently installed programs run by the Agent on Photo Workspace files.

A Skill MAY describe Agent-owned setup from a local directory or source repository. That setup is not a Slipstream installation or capability claim.

A Skill MUST instruct an Agent to pass the protected workspace input to an External Tool, never an Original Location. Slipstream cannot enforce the External Tool's filesystem access.

A Skill MUST NOT present Slipstream as a plugin installer, plugin registry, External Tool runtime, sandbox, provenance service, or MCP server.

A Skill MUST NOT require artifact registration, bundle digests, or processing provenance for an External Tool output to appear as a Workspace Artifact.

A Skill MUST NOT describe an External Tool output as a validated Processing Artifact or direct it to `processing artifact` commands.

The `core` Skill MUST identify `processing` commands as advanced validated-artifact surfaces, not the External Tool path.

## Acceptance

- A service-free CLI lists the catalog and prints each Skill as Markdown.
- A Skill's examples use only commands and fields exposed by the layered `--help` output.
- An Agent can move from a Photo to a Workspace, set up a documented External Tool, run it, and inspect completed files without a Slipstream-managed plugin installation.

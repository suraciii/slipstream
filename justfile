# Low-cost checks: no native build, browser runtime, or engine image.
ci-fast:
    bun run check:static
    bun run --cwd apps/web test:unit
    bun run test:license
    bun run test:cli-package
    bun run test:metadata
    bun run test:container-input
    bun run test:docker-context
    bun run test:compose-storage
    bun run test:workflow
    bun run test:worktree-preflight
    bun run test:raw-contract
    bun run test:development-runner
    bun run test:processing-tools

# Complete repository gate, including every fast check.
ci-full: ci-fast
    bun run test:rust
    bun run test:raw-discovery
    bun run build
    bun run test:browser:prebuilt

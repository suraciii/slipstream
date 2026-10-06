use serde_json::Value;

/// The second peer module: standalone SpektraFilm, not a darktable
/// image-operation module.
pub const SPEKTRAFILM_MODULE: &str = "spektrafilm";
/// The standalone SpektraFilm implementation identity for this boundary.
pub const SPEKTRAFILM_IMPLEMENTATION: &str = "spektrafilm-rs";
/// The checked-in fork revision accepted by bundle verification.
pub const SPEKTRAFILM_FORK_COMMIT: &str = "e449ddba9bd137d8769388d1d1954736b56f2db3";
/// The pinned standalone SpektraFilm adapter identity of this contract
/// revision.
pub const SPEKTRAFILM_ADAPTER_VERSION: &str = "spektrafilm-rs-adapter-1";
/// The SpektraFilm-owned parameter schema version this boundary admits.
pub const SPEKTRAFILM_PARAMETER_VERSION: &str = "spektrafilm-rs-params-1";
/// The previous saved-tree version remains readable for historical Recipes
/// and Artifacts, but is not emitted by discovery or new executions.
pub const SPEKTRAFILM_LEGACY_PARAMETER_VERSION: &str = "spektrafilm-params-1";
/// The pinned fixed-recipe identity of the standalone SpektraFilm adapter,
/// the shared `FILM_RECIPE_SHA256` of `tools/development/film_identity.py`.
/// The pinned runtime re-verifies the forwarded tree against this recipe;
/// exact bundle verification separately pins source, packages, and runtime bytes.
pub const SPEKTRAFILM_RECIPE_SHA256: &str =
    "8efdd28d3a49fea7e71835dea82dbc416d9f95e4ae4216ae5fb7535087ec5cf8";
/// The pinned handoff profile of the standalone SpektraFilm input — the
/// shared `INPUT_ICC_SHA256` of `film_identity.py`, which is the byte
/// identity of the darktable peer's Development TIFF output profile.
pub const SPEKTRAFILM_INPUT_ICC_SHA256: &str =
    "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe";
/// The pinned sRGB profile bytes of the standalone SpektraFilm Finished
/// JPEG — the shared `OUTPUT_ICC_SHA256` of `film_identity.py`.
pub const SPEKTRAFILM_OUTPUT_ICC_SHA256: &str =
    "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21";
/// The complete default parameter tree of the pinned fixed recipe: the
/// exact runtime groups and Finished JPEG output the pinned runtime's own
/// `--emit-default-parameters` writes into its bundle
/// (`parameters-default.json`, digest-pinned by the bundle manifest). The
/// fixed runtime executes only this recipe, so the host pins the tree
/// beside the recipe identity above: startup verification refuses a
/// bundle whose emitted defaults differ, and admission compares every
/// forwarded group against these values.
pub const SPEKTRAFILM_RECIPE_TREE: &str = include_str!("../spektrafilm-recipe-tree.json");

/// The complete default parameter tree of the pinned fixed recipe, parsed
/// once from [`SPEKTRAFILM_RECIPE_TREE`]. The standalone runtime executes
/// only this tree: discovery publishes it as the module's own defaults,
/// and admission compares every forwarded group against it.
pub fn spektrafilm_default_tree() -> Value {
    static PINNED: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
        serde_json::from_str(SPEKTRAFILM_RECIPE_TREE).expect("the pinned recipe tree parses")
    });
    PINNED.clone()
}

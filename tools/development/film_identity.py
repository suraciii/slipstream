"""Shared accepted identity for the qualification and Film adapter paths."""

# Exact source, package, and runtime bytes are independently pinned by
# bundle.py and the adapter bundle manifest. The recipe covers all numerical
# parameters, stock names, and seed; only build provenance and the bounded
# workspace allocation are excluded because neither defines the saved look.
import hashlib
import json

FILM_RECIPE_SHA256 = "8efdd28d3a49fea7e71835dea82dbc416d9f95e4ae4216ae5fb7535087ec5cf8"
INPUT_ICC_SHA256 = "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe"
OUTPUT_ICC_SHA256 = "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21"
FINISHED_JPEG_QUALITY = 85
PROCEDURE = "film-once-empty-cache-v1"


def recipe_digest(manifest):
    """Hash the complete numerical recipe, retaining unknown recipe fields."""
    recipe = {
        key: value for key, value in manifest.items()
        if key not in {"processing_bundle", "gamut_workspace_allowance_bytes"}
    }
    encoded = json.dumps(recipe, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()

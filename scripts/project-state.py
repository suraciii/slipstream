#!/usr/bin/env python3
"""Project persisted Slipstream state into the expected-state snapshot schema.

This is the only supported way to build the `SLIPSTREAM_EXPECTED_STATE_SNAPSHOT`
file used by `scripts/verify-production.sh`. It reads the owned SQLite state
read-only (`mode=ro&immutable=1`), never writes, and fails closed on sidecar
files, schema drift, or unreadable rows. The output mirrors the bounded HTTP
wire exactly (`PhotoSummary` objects in the All Photos order and Photo Set
membership with saved position), so production acceptance compares the offline
projection against live bounded traversal over the same facts.

Usage:
    python3 scripts/project-state.py /data/slipstream/state/library.sqlite \
        > expected-state.json
"""

import json
import sqlite3
import sys
from pathlib import Path

REQUIRED_TABLES = (
    "library_metadata",
    "original_files",
    "photos",
    "photo_sets",
    "photo_set_members",
    "review_progress",
)
REQUIRED_COLUMNS = {
    "original_files": ("id", "kind", "available"),
    "photos": (
        "id",
        "raw_original_id",
        "jpeg_original_id",
        "ambiguous",
        "available",
        "preview_state",
        "preview_source",
        "preview_width",
        "preview_height",
        "sort_path",
        "selection_state",
        "rating",
    ),
    "photo_sets": ("id", "name", "created_at"),
    "photo_set_members": ("photo_set_id", "photo_id", "position"),
    "review_progress": ("photo_set_id", "photo_id"),
}

# Identical ordering to the Published Library snapshot loader: Capture Time
# order first (raw fact outranks JPEG), then sort path, then id, with Photos
# that lack any Capture Time fact last. COLLATE BINARY is byte order; UTF-8
# bytes preserve code point order, so ordering by the UTF-8 encoding matches.
PHOTOS_QUERY = """
SELECT p.id,p.raw_original_id,p.jpeg_original_id,p.ambiguous,p.available,
       p.preview_state,p.preview_source,p.preview_width,p.preview_height,
       p.selection_state,p.rating
FROM photos p
LEFT JOIN original_files raw ON raw.id=p.raw_original_id
LEFT JOIN original_files jpeg ON jpeg.id=p.jpeg_original_id
ORDER BY CASE WHEN COALESCE(raw.capture_order_key,jpeg.capture_order_key) IS NULL THEN 1 ELSE 0 END,
         COALESCE(raw.capture_order_key,jpeg.capture_order_key) COLLATE BINARY,
         p.sort_path COLLATE BINARY,p.id
"""
ORIGINALS_QUERY = "SELECT id,available FROM original_files"
PHOTO_SETS_QUERY = "SELECT id,name FROM photo_sets ORDER BY created_at,id"
MEMBERS_QUERY = """
SELECT m.photo_id,m.position,p.available,p.selection_state,p.rating
FROM photo_set_members m JOIN photos p ON p.id=m.photo_id
WHERE m.photo_set_id=? ORDER BY m.position
"""
SAVED_QUERY = "SELECT photo_id FROM review_progress WHERE photo_set_id=?"
MAXIMUM_PREVIEW_EDGE = 2560


def fail(message: str):
    print(f"project-state: {message}", file=sys.stderr)
    raise SystemExit(1)


def connect(database: str) -> sqlite3.Connection:
    for suffix in ("-journal", "-wal", "-shm"):
        if Path(database + suffix).exists():
            fail(f"sidecar {database + suffix} exists; recover at an admitted boundary first")
    try:
        connection = sqlite3.connect(
            f"file:{database}?mode=ro&immutable=1", uri=True
        )
        if connection.execute("PRAGMA quick_check").fetchone() != ("ok",):
            fail(f"{database} fails SQLite quick_check")
    except sqlite3.Error as error:
        fail(f"cannot open {database} read-only: {error}")
    return connection


SUPPORTED_SCHEMA_VERSION = 4


def check_schema(connection: sqlite3.Connection) -> None:
    version = connection.execute("PRAGMA user_version").fetchone()[0]
    if version != SUPPORTED_SCHEMA_VERSION:
        fail(
            "unsupported schema version "
            f"{version}; this projector supports {SUPPORTED_SCHEMA_VERSION}"
        )
    tables = {
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        )
    }
    for table in REQUIRED_TABLES:
        if table not in tables:
            fail(f"required table {table} is missing")
    for table, columns in REQUIRED_COLUMNS.items():
        present = {
            row[1] for row in connection.execute(f"PRAGMA table_info({table})")
        }
        missing = [column for column in columns if column not in present]
        if missing:
            fail(f"table {table} is missing columns: {', '.join(missing)}")


def project_photo(row, originals_available: dict) -> dict:
    (
        photo_id,
        raw_original_id,
        jpeg_original_id,
        ambiguous,
        available,
        preview_state,
        preview_source,
        preview_width,
        preview_height,
        selection_state,
        rating,
    ) = row
    originals = []
    for original_id, kind in (
        (raw_original_id, "raw"),
        (jpeg_original_id, "jpeg"),
    ):
        if original_id is not None and original_id in originals_available:
            originals.append(
                {"kind": kind, "available": originals_available[original_id]}
            )
    preview = {"state": preview_state}
    if preview_source is not None:
        preview["source"] = preview_source
    if preview_width is not None:
        preview["width"] = preview_width
    if preview_height is not None:
        preview["height"] = preview_height
    if preview_width is not None and preview_height is not None:
        preview["limitedDetail"] = max(preview_width, preview_height) < (
            MAXIMUM_PREVIEW_EDGE
        )
    if not available:
        preview["message"] = "Original File is unavailable"
    return {
        "id": photo_id,
        "available": bool(available),
        "ambiguous": bool(ambiguous),
        "originals": originals,
        "selectionState": selection_state,
        "rating": rating,
        "preview": preview,
    }


def project_sets(connection: sqlite3.Connection) -> list:
    sets = []
    for set_id, name in connection.execute(PHOTO_SETS_QUERY):
        members = [
            {
                "photoId": photo_id,
                "position": position,
                "available": bool(available),
                "selectionState": selection_state,
                "rating": rating,
            }
            for photo_id, position, available, selection_state, rating in (
                connection.execute(MEMBERS_QUERY, (set_id,)).fetchall()
            )
        ]
        saved = connection.execute(SAVED_QUERY, (set_id,)).fetchone()
        entry = {
            "id": set_id,
            "name": name,
            "photoCount": len(members),
            "hasSavedPosition": saved is not None,
            "members": members,
        }
        if saved is not None:
            entry["lastReviewedPhotoId"] = saved[0]
        sets.append(entry)
    return sets


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: project-state.py <state-database>")
    database = sys.argv[1]
    connection = connect(database)
    try:
        check_schema(connection)
        originals_available = {
            original_id: bool(available)
            for original_id, available in connection.execute(ORIGINALS_QUERY)
        }
        photos = [
            project_photo(row, originals_available)
            for row in connection.execute(PHOTOS_QUERY)
        ]
        snapshot = {"photos": photos, "photoSets": project_sets(connection)}
    except (sqlite3.Error, UnicodeDecodeError) as error:
        fail(f"projection failed: {error}")
    finally:
        connection.close()
    json.dump(snapshot, sys.stdout, ensure_ascii=False, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()

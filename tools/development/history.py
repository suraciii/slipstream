"""Pinned darktable history construction and post-import contract checks."""

import math
import sqlite3
import struct
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

DT = "http://darktable.sf.net/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
VERSIONS = {
    "rawprepare": 2,
    "demosaic": 6,
    "colorin": 7,
    "colorout": 5,
    "gamma": 1,
    "temperature": 4,
    "highlights": 4,
    "flip": 2,
}
CUSTOM_WB = (1.25, 1.0, 0.8, 1.0)
CUSTOM_WB_PRESET = 2  # darktable 5.4.1: user-modified coefficients


def exposure_params(exposure_ev):
    if not math.isfinite(exposure_ev):
        raise ValueError("Exposure must be finite")
    # mode=manual, black=0, EV, deflicker defaults, both bias adjustments off
    return struct.pack("<iffffii", 0, 0.0, exposure_ev, 50.0, -4.0, 0, 0)


def custom_wb_params():
    return struct.pack("<ffffi", *CUSTOM_WB, CUSTOM_WB_PRESET)


def _baseline_rows(database):
    with sqlite3.connect(f"file:{Path(database)}?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        rows = list(db.execute("SELECT * FROM history WHERE imgid=1 ORDER BY num"))
        order = db.execute(
            "SELECT version FROM module_order WHERE imgid=1"
        ).fetchone()
    operations = Counter(row["operation"] for row in rows)
    if operations != Counter({operation: 1 for operation in VERSIONS}):
        raise RuntimeError(
            "Unexpected darktable baseline history; refusing implicit or duplicate modules: "
            f"{dict(operations)}"
        )
    if order is None or order[0] != 4:
        raise RuntimeError("Unsupported darktable module-order version")
    if any(row["module"] != VERSIONS[row["operation"]] for row in rows):
        raise RuntimeError("Unsupported darktable baseline module version")
    return rows


def generated_history(database, output, exposure_ev, *, custom_wb=False):
    """Create the adapter-shaped history used by the executable qualification probe."""
    rows = _baseline_rows(database)
    for prefix, uri in [("x", "adobe:ns:meta/"), ("rdf", RDF), ("darktable", DT)]:
        ET.register_namespace(prefix, uri)
    root = ET.Element("{adobe:ns:meta/}xmpmeta")
    rdf = ET.SubElement(root, f"{{{RDF}}}RDF")
    desc = ET.SubElement(rdf, f"{{{RDF}}}Description", {
        f"{{{RDF}}}about": "", f"{{{DT}}}xmp_version": "5",
        f"{{{DT}}}history_end": str(len(rows) + 1),
        f"{{{DT}}}iop_order_version": "4", f"{{{DT}}}auto_presets_applied": "1",
    })
    sequence = ET.SubElement(ET.SubElement(desc, f"{{{DT}}}history"), f"{{{RDF}}}Seq")
    mapping = {
        "num": "num", "operation": "operation", "enabled": "enabled",
        "modversion": "module", "params": "op_params",
        "blendop_params": "blendop_params", "blendop_version": "blendop_version",
        "multi_priority": "multi_priority", "multi_name": "multi_name",
        "multi_name_hand_edited": "multi_name_hand_edited",
    }
    for row in rows:
        values = {key: row[column] for key, column in mapping.items()}
        if custom_wb and row["operation"] == "temperature":
            values["params"] = custom_wb_params()
        ET.SubElement(sequence, f"{{{RDF}}}li", {
            f"{{{DT}}}{key}": value.hex() if isinstance(value, bytes) else str(value)
            for key, value in values.items() if value is not None
        })
    values = {
        "num": str(len(rows)), "operation": "exposure", "enabled": "1",
        "modversion": "7", "params": exposure_params(exposure_ev).hex(),
        "multi_priority": "0", "multi_name": "", "multi_name_hand_edited": "0",
    }
    ET.SubElement(sequence, f"{{{RDF}}}li", {f"{{{DT}}}{k}": v for k, v in values.items()})
    output = Path(output)
    ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)
    return output


def validate_imported_history(database, exposure_ev, *, custom_wb=False):
    with sqlite3.connect(f"file:{Path(database)}?mode=ro", uri=True) as db:
        rows = list(db.execute(
            "SELECT operation, enabled, module, op_params "
            "FROM history WHERE imgid=1 ORDER BY num"
        ))
    operations = Counter(row[0] for row in rows)
    expected = Counter({operation: 1 for operation in (*VERSIONS, "exposure")})
    if operations != expected:
        raise RuntimeError(
            "Unexpected imported darktable history; duplicate adaptation or automatic/look module: "
            f"{dict(operations)}"
        )
    by_operation = {row[0]: row for row in rows}
    temperature = by_operation["temperature"]
    if temperature[2] != VERSIONS["temperature"] or not temperature[1]:
        raise RuntimeError("darktable white-balance module is disabled or has an unknown version")
    if custom_wb and temperature[3] != custom_wb_params():
        raise RuntimeError("darktable did not import the requested custom white-balance coefficients")
    exposure = by_operation["exposure"]
    if exposure != (
        "exposure", 1, 7, exposure_params(exposure_ev)
    ):
        raise RuntimeError("darktable did not import the requested manual exposure parameters")
    return {
        "operations": dict(operations),
        "exposure_ev": exposure_ev,
        "exposure_mode": "manual",
        "exposure_bias_compensation": False,
        "white_balance_module_count": operations["temperature"],
        "custom_white_balance": custom_wb,
        "additional_adaptation_modules": 0,
    }

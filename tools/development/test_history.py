"""Fast contract tests for the generated darktable history boundary."""

import sqlite3
import tempfile
import unittest
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

from history import (
    DT,
    RDF,
    VERSIONS,
    custom_wb_params,
    exposure_params,
    generated_history,
    validate_imported_history,
)


def create_database(path, *, custom_wb=False, exposure_ev=None, extra=()):
    with sqlite3.connect(path) as db:
        db.execute(
            "CREATE TABLE history (imgid INTEGER, num INTEGER, operation TEXT, "
            "enabled INTEGER, module INTEGER, op_params BLOB, blendop_params BLOB, "
            "blendop_version INTEGER, multi_priority INTEGER, multi_name TEXT, "
            "multi_name_hand_edited INTEGER)"
        )
        db.execute("CREATE TABLE module_order (imgid INTEGER, version INTEGER)")
        db.execute("INSERT INTO module_order VALUES (1, 4)")
        rows = []
        for num, (operation, module) in enumerate(VERSIONS.items()):
            params = custom_wb_params() if custom_wb and operation == "temperature" else b"baseline"
            rows.append((1, num, operation, 1, module, params, b"", 1, 0, "", 0))
        for operation, module in extra:
            rows.append((1, len(rows), operation, 1, module, b"extra", b"", 1, 0, "", 0))
        if exposure_ev is not None:
            rows.append((1, len(rows), "exposure", 1, 7, exposure_params(exposure_ev), b"", 1, 0, "", 0))
        db.executemany("INSERT INTO history VALUES (?,?,?,?,?,?,?,?,?,?,?)", rows)


class HistoryTests(unittest.TestCase):
    def test_custom_wb_and_ev_are_written_as_one_explicit_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            database = root / "baseline.db"
            output = root / "ev1-custom-wb.xmp"
            create_database(database)

            generated_history(database, output, 1.0, custom_wb=True)
            document = ET.parse(output).getroot()
            description = document.find(f".//{{{RDF}}}Description")
            items = document.findall(f".//{{{RDF}}}Seq/{{{RDF}}}li")
            operations = [item.attrib[f"{{{DT}}}operation"] for item in items]
            self.assertEqual(Counter(operations), Counter({name: 1 for name in (*VERSIONS, "exposure")}))
            self.assertEqual(description.attrib[f"{{{DT}}}auto_presets_applied"], "1")

            temperature = next(item for item in items if item.attrib[f"{{{DT}}}operation"] == "temperature")
            exposure = next(item for item in items if item.attrib[f"{{{DT}}}operation"] == "exposure")
            self.assertEqual(temperature.attrib[f"{{{DT}}}modversion"], "4")
            self.assertEqual(bytes.fromhex(temperature.attrib[f"{{{DT}}}params"]), custom_wb_params())
            self.assertEqual(exposure.attrib[f"{{{DT}}}modversion"], "7")
            self.assertEqual(bytes.fromhex(exposure.attrib[f"{{{DT}}}params"]), exposure_params(1.0))

    def test_rejects_duplicate_or_unexpected_baseline_adaptation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for extra in (("temperature", 4), ("color calibration", 3), ("exposure", 7)):
                database = root / f"invalid-{extra[0]}.db"
                create_database(database, extra=(extra,))
                with self.subTest(extra=extra), self.assertRaisesRegex(RuntimeError, "Unexpected darktable baseline"):
                    generated_history(database, root / "unused.xmp", 1.0, custom_wb=True)

    def test_imported_history_rejects_automatic_or_duplicate_adjustments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / "good.db"
            create_database(good, custom_wb=True, exposure_ev=1.0)
            facts = validate_imported_history(good, 1.0, custom_wb=True)
            self.assertEqual(facts["white_balance_module_count"], 1)
            self.assertEqual(facts["exposure_mode"], "manual")
            self.assertFalse(facts["exposure_bias_compensation"])

            bad = root / "bad.db"
            create_database(bad, custom_wb=True, exposure_ev=1.0, extra=(("temperature", 4),))
            with self.assertRaisesRegex(RuntimeError, "duplicate adaptation or automatic/look module"):
                validate_imported_history(bad, 1.0, custom_wb=True)

    def test_imported_history_requires_module_order_and_contiguous_numbers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reversed_history = root / "reversed.db"
            create_database(reversed_history, custom_wb=True, exposure_ev=1.0)
            with sqlite3.connect(reversed_history) as db:
                db.execute("UPDATE history SET num=9 WHERE operation='temperature'")
                db.execute("UPDATE history SET num=5 WHERE operation='exposure'")
                db.execute("UPDATE history SET num=8 WHERE operation='temperature'")
            with self.assertRaisesRegex(RuntimeError, "sequence or numbering"):
                validate_imported_history(reversed_history, 1.0, custom_wb=True)

            gap = root / "gap.db"
            create_database(gap, custom_wb=True, exposure_ev=1.0)
            with sqlite3.connect(gap) as db:
                db.execute("UPDATE history SET num=9 WHERE operation='exposure'")
            with self.assertRaisesRegex(RuntimeError, "sequence or numbering"):
                validate_imported_history(gap, 1.0, custom_wb=True)

    def test_exposure_rejects_non_finite_ev(self):
        for value in (float("nan"), float("inf"), float("-inf")):
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "must be finite"):
                exposure_params(value)


if __name__ == "__main__":
    unittest.main()

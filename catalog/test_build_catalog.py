"""Tests for catalog/build_catalog.py. Run from the repo root after fetch_sources.py."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REVISION = "f977235e6d736c4c0b650520ac5a5b72cbfe9244"


class CatalogTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "catalog.json"
            subprocess.run(
                [sys.executable, "-I", str(ROOT / "catalog" / "build_catalog.py"),
                 str(ROOT / "catalog" / "upstream-src"), "--revision", REVISION, "--output", str(out)],
                check=True, capture_output=True)
            cls.generated_text = out.read_text(encoding="utf-8")
        cls.c = json.loads(cls.generated_text)

    def test_key_count(self):
        self.assertGreaterEqual(len(self.c["options"]), 590)

    def test_version(self):
        self.assertEqual(self.c["studio_version"], "02.08.04.57")
        self.assertEqual(self.c["source_revision"], REVISION)

    def test_required_fields(self):
        for k, o in self.c["options"].items():
            for f in ("type", "nullable", "scopes", "enum_values", "label", "description"):
                self.assertIn(f, o, k)
            self.assertTrue("default" in o or "default_expression" in o, k)
            if o["type"] == "Enum":
                self.assertTrue(o["enum_values"], k)

    def test_demo_keys_have_project_scope(self):
        for k in ("sparse_infill_density", "sparse_infill_pattern", "wall_loops", "layer_height", "enable_support"):
            self.assertIn("project", self.c["options"][k]["scopes"], k)

    def test_object_scope_fixed(self):
        for k in ("layer_height", "enable_support", "support_type", "sparse_infill_density", "wall_loops"):
            self.assertIn("object", self.c["options"][k]["scopes"], k)

    def test_class_scope_rule(self):
        o = self.c["options"]
        self.assertEqual(o["layer_height"]["scopes"], ["project", "object"])          # PrintObjectConfig
        self.assertEqual(o["wall_loops"]["scopes"][:2], ["project", "object"])       # PrintRegionConfig
        self.assertIn("volume", o["wall_loops"]["scopes"])
        self.assertEqual(o["print_sequence"]["scopes"][0], "project")                # global (+ plate)
        self.assertNotIn("object", o["print_sequence"]["scopes"])
        for k, v in o.items():
            if v["scopes"]:
                self.assertIn("project", v["scopes"], k)

    def test_demo_keys(self):
        o = self.c["options"]
        self.assertEqual((o["sparse_infill_density"]["type"], o["sparse_infill_density"]["min"],
                          o["sparse_infill_density"]["max"]), ("Percent", 0, 100))
        self.assertIn("gyroid", o["sparse_infill_pattern"]["enum_values"])
        self.assertEqual(o["support_type"]["enum_values"], ["normal(auto)", "tree(auto)", "normal(manual)", "tree(manual)"])
        self.assertEqual(o["wall_loops"]["type"], "Int")
        self.assertEqual(o["layer_height"]["type"], "Float")

    def test_committed_catalog_is_fresh(self):
        self.assertEqual((ROOT / "catalog" / "catalog.json").read_text(encoding="utf-8"), self.generated_text)


if __name__ == "__main__":
    unittest.main()

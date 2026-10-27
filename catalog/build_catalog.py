# bamburino build_catalog.py, adapted from subhashdasyam/bambustudio-mcp tools/build_catalog.py
# (commit 13a138a20ec3f449471f23d5d5bab99a0d31306d), licensed AGPL-3.0-or-later.
# This adaptation is distributed under the same licence.
# Dev/CI tool only: never runs on the user's PC.
# Changes from the original:
#   1. --revision replaces `git rev-parse` (no clone needed).
#   2. Scope bug fixed (anchored class regex, #define skipped) and fixed class->scope rule.
#   3. Missing label/description are made explicit (label_missing / description_missing).
#   4. coverage.matrix and the defaults/evidence inputs were dropped (unused).
"""Extract the pinned source's FFF schema; use executable-exported defaults when present.

This deliberately records unresolved expressions. It never evaluates C++ as Python.
Run the compiled exporter against a matching Studio build for authoritative parity.
"""
import argparse
import ast
import hashlib
import json
import re
from pathlib import Path


GENERATOR = ("bamburino build_catalog.py, adapted from subhashdasyam/bambustudio-mcp "
             "tools/build_catalog.py (AGPL-3.0-or-later)")


def uncomment(text):
    return re.sub(r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|//[^\n]*|/\*[\s\S]*?\*/',
                  lambda m: m[0] if m[0][0] in "\"'" else "\n" * m[0].count("\n"), text)


def strings(text):
    return [ast.literal_eval(m[0]) for m in re.finditer(r'"(?:\\.|[^"\\])*"', text)]


def numeric(text):
    text = text.strip().replace("true", "True").replace("false", "False")
    tree = ast.parse(text, mode="eval")
    if any(not isinstance(n, (ast.Expression, ast.Constant, ast.UnaryOp, ast.USub, ast.UAdd,
                             ast.BinOp, ast.Add, ast.Sub, ast.Mult, ast.Div, ast.Load)) for n in ast.walk(tree)):
        raise ValueError(text)
    return eval(compile(tree, "<constant>", "eval"), {"__builtins__": {}}, {})


def generate(source, revision):
    raw = (source / "src/libslic3r/PrintConfig.cpp").read_text(encoding="utf-8")
    text = uncomment(raw)
    start = text.index("void PrintConfigDef::init_common_params()")
    end = text.index("void PrintConfigDef::init_extruder_option_keys()")
    body = text[start:end]
    for group in ("filament_extruder_override_keys", "filament_overhang_override_keys"):
        loop = body.index(f"for (auto& opt_key : {group})")
        opening = body.index("{", loop)
        depth = 0
        for token in re.finditer(r'"(?:\\.|[^"\\])*"|[{}]', body[opening:]):
            if token[0] == "{":
                depth += 1
            elif token[0] == "}":
                depth -= 1
                if depth == 0:
                    body = body[:loop] + body[opening + token.end():]
                    break
    adds = list(re.finditer(r'this->add\("([^\"]+)",\s*(co\w+)\)', body))
    enums = {}
    for m in re.finditer(r'static\s+(?:const\s+)?t_config_enum_values s_keys_map_(\w+)\s*(?:=\s*)?\{([\s\S]*?)\n\};', text):
        enums[m[1]] = {key: re.sub(r'\bint\(|\)|\s', '', symbol) for key, symbol in
                      re.findall(r'\{\s*"([^\"]+)"\s*,\s*([^}\n]+)\}', m[2])}
    options = {}
    unresolved = []
    header = uncomment((source / "src/libslic3r/PrintConfig.hpp").read_text(encoding="utf-8"))
    scope_keys = {}
    # Adaptation 2: anchor at line start so the `#define PRINT_CONFIG_CLASS_DEFINE(CLASS_NAME, ...)`
    # macro definition is skipped; the original matched it first and mis-attributed
    # PrintObjectConfig keys (layer_height, enable_support, ...) to project scope only.
    for match in re.finditer(r'(?m)^PRINT_CONFIG_CLASS_DEFINE\(\s*(\w+),([\s\S]*?)\n\)', header):
        # Adaptation 2b: the type may be templated (ConfigOptionEnum<SupportType>), so not just \w+.
        scope_keys[match[1]] = set(re.findall(r'\(\([^,()]+,\s*(\w+)\)\)', match[2]))
    assert scope_keys.get("PrintObjectConfig") and scope_keys.get("PrintRegionConfig"), sorted(scope_keys)
    object_keys = scope_keys["PrintObjectConfig"]
    region_keys = scope_keys["PrintRegionConfig"]
    for index, match in enumerate(adds):
        block = body[match.end():adds[index + 1].start() if index + 1 < len(adds) else len(body)]
        key, kind = match[1], match[2][2:]
        option = {"type": kind, "nullable": False, "scopes": ["project"],
                  "source_line": raw[:raw.index('"' + key + '"', raw.index("void PrintConfigDef::init_common_params()"))].count("\n") + 1}
        # Fixed class->scope rule: PrintObjectConfig -> project+object; PrintRegionConfig ->
        # project+object+volume+height_range; any other (global) class -> project only.
        if key in object_keys or key in region_keys or key == "extruder":
            option["scopes"].append("object")
        if key in region_keys or key == "extruder":
            option["scopes"] += ["volume", "height_range"]
        if key.startswith("filament_"):
            option["scopes"].append("filament")
        # PlateConfig is a separate persisted schema; only its defined keys are writable there.
        if key in {"curr_bed_type", "print_sequence", "spiral_mode", "first_layer_print_sequence", "other_layers_print_sequence", "other_layers_print_sequence_nums"}:
            option["scopes"].append("plate")
        for field, dest in [("label", "label"), ("tooltip", "description"), ("category", "category"),
                            ("sidetext", "units"), ("ratio_over", "ratio_over")]:
            vals = re.findall(r'def->' + field + r'\s*=\s*([\s\S]*?);', block)
            if vals:
                option[dest] = "".join(strings(vals[-1]))
        for field in ["min", "max", "readonly", "nullable"]:
            vals = re.findall(r'def->' + field + r'\s*=\s*([^;]+);', block)
            if vals:
                try:
                    option[field] = numeric(vals[-1].replace("max_temp", "360"))
                except (ValueError, SyntaxError):
                    unresolved.append(f"{key}.{field}: {vals[-1]}")
        # Adaptation 3: make missing metadata explicit instead of omitting the fields.
        if "label" not in option:
            option["label"] = key
            option["label_missing"] = True
        if "description" not in option:
            option["description"] = ""
            option["description_missing"] = True
        option["enum_values"] = [s for expr in re.findall(r'def->enum_values.(?:push_back|emplace_back)\(([^;]+)\);', block) for s in strings(expr)]
        enum_class = re.search(r'enum_keys_map\s*=\s*&ConfigOptionEnum<(\w+)>', block)
        if enum_class and not option["enum_values"]:
            option["enum_values"] = list(enums.get(enum_class[1], {}))
        if enum_class:
            option["serialized_enum_values"] = list(enums.get(enum_class[1], {}))
        direct_map = re.search(r'enum_keys_map\s*=\s*&s_keys_map_(\w+)', block)
        if direct_map and not option["enum_values"]:
            option["enum_values"] = list(enums.get(direct_map[1], {}))
        copied = re.search(r'def->enum_values\s*=\s*(\w+)->enum_values', block)
        if copied:
            declaration = re.search(r'auto\s+' + copied[1] + r'\s*=\s*def\s*=\s*this->add\("([^\"]+)"', body)
            if declaration:
                option["enum_values"] = options[declaration[1]]["enum_values"]
        if enum_class and option["enum_values"]:
            mapped = enums.get(enum_class[1], {})
            # A subset in the UI is a real restriction, except where Studio's own UI label
            # differs from its serialized value (100% cooling is serialized as 95%).
            option["serialized_enum_values"] = [v for v in option["enum_values"] if v in mapped] if all(v in mapped for v in option["enum_values"]) else list(mapped)
        alias = re.search(r'def->aliases\s*=\s*\{([^}]+)\}', block)
        option["aliases"] = strings(alias[1]) if alias else []
        default = re.search(r'set_default_value\(new ConfigOption\w+(?:<\w+>)?\s*([({][\s\S]*?)\);', block)
        if default:
            expr = default[1][1:-1].strip()
            try:
                if kind in {"Enum", "Enums"} and enum_class:
                    normalized = re.sub(r'\bint\(|\)|\s', '', expr).split("::")[-1]
                    if kind == "Enums":
                        option["default"] = [k for k, v in enums[enum_class[1]].items() if v.split("::")[-1] == normalized]
                        if not option["default"]:
                            raise ValueError(normalized)
                    else:
                        option["default"] = next(k for k, v in enums[enum_class[1]].items() if v.split("::")[-1] == normalized)
                elif kind in {"String", "Strings"}:
                    values = strings(expr)
                    option["default"] = values if kind == "Strings" else "".join(values)
                elif kind in {"Point", "Points", "PointsGroups", "Point3", "IntsGroups"}:
                    raise ValueError("coordinate default requires compiled exporter")
                else:
                    values = [numeric(v) for v in expr.split(",") if v.strip()]
                    vector = kind in {"Floats", "Ints", "Bools", "Percents", "FloatsOrPercents"}
                    if vector and default[1].startswith("(") and len(values) == 2:
                        values = [values[1]] * int(values[0])
                    option["default"] = values if vector else values[0]
                option["default_evidence"] = "source_constant"
            except (ValueError, SyntaxError, StopIteration, KeyError, IndexError):
                option["default_expression"] = expr
        if "Nullable" in block.split("set_default_value", 1)[-1]:
            option["nullable"] = True
        options[key] = option
        if key.startswith("printhost_") or key in {"print_host", "print_host_webui", "printer_host", "host_type", "preset_names", "preset_name"}:
            option["scopes"] = []
            option["excluded_reason"] = "printer communication is outside this server's scope"
    for group in ["filament_extruder_override_keys", "filament_overhang_override_keys"]:
        match = re.search(r'\b' + group + r'\s*=\s*\{([^}]+)\}', text)
        for key in strings(match[1]):
            original = options[key.removeprefix("filament_")]
            options[key] = dict(original, nullable=True, scopes=["project", "filament"], generated_from=key.removeprefix("filament_"))
    version = re.search(r'set\(SLIC3R_VERSION "([^\"]+)"', (source / "version.inc").read_text(encoding="utf-8"))[1]
    # Adaptation 1: the revision is passed in; no git checkout is needed.
    missing_defaults = [k for k, v in options.items() if "default" not in v and "default_expression" not in v]
    if missing_defaults == ["extruder"]:
        options["extruder"]["default_expression"] = "none: Studio has no static default; inherit the object's assignment"
        missing_defaults = []
    # Adaptation 4: coverage.matrix and runtime evidence fields removed (unused).
    return {"studio_version": version, "source_revision": revision, "generator": GENERATOR,
            "source_sha256": hashlib.sha256(raw.encode()).hexdigest(), "options": dict(sorted(options.items())),
            "coverage": {"discovered": len(options), "unresolved_metadata": unresolved,
                         "missing_defaults": missing_defaults}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    data = generate(args.source.resolve(), args.revision)
    args.output.write_text(json.dumps(data, indent=2, sort_keys=True) + chr(10), encoding="utf-8", newline=chr(10))
    print(json.dumps(data["coverage"], indent=2))

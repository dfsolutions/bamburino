# Parameter catalog

`catalog.json` lists the Bambu Studio print settings found by static extraction (type, scopes, label, default,
enum values) for the Studio version pinned in `driver/upstream.toml`. It is generated, never edited by hand.
`build_catalog.py` is adapted from subhashdasyam/bambustudio-mcp (AGPL-3.0-or-later).

To regenerate (Python 3.11+, dev/CI only; the service never runs Python):

    python -I catalog/fetch_sources.py
    python -I catalog/build_catalog.py catalog/upstream-src --revision <studio.tag_commit> --output catalog/catalog.json
    python -m unittest catalog/test_build_catalog.py

`upstream-src/` is git-ignored. The tests fail if the committed catalog is stale.

Notes for consumers:
- 12 keys registered in a loop over axes are not extracted: `machine_max_{speed,acceleration,jerk}_{x,y,z,e}`.
  They are listed in `coverage.known_missing`.
- A FloatOrPercent default that is a percentage is a string such as `"400%"`.
- Options also carry `default_evidence`, `source_line`, `generated_from` and `ratio_over`; consumers must not
  use `deny_unknown_fields`.
- `extruder` is a deliberate scope exception: it gets the PrintRegionConfig scopes although no such class declares it.
- Scope rule: `PrintObjectConfig` keys get `project, plate, object`; `PrintRegionConfig` keys get
  `project, plate, object, volume, height_range`; other classes get `project` only. Exceptions: the six plate-only
  keys (`print_sequence`, ...) get `project, plate`, and the driver override list exception: `initial_layer_print_height`,
  `skirt_loops`, `travel_speed` (global classes, but in the driver's 19-key `override_settings()` list, which the
  `settings.override` handler applies to every override scope) get `project, plate, object, volume`.

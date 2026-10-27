# Parameter catalog

`catalog.json` lists every Bambu Studio print setting (type, scopes, label, default, enum values)
for the Studio version pinned in `driver/upstream.toml`. It is generated, never edited by hand.
`build_catalog.py` is adapted from subhashdasyam/bambustudio-mcp (AGPL-3.0-or-later).

To regenerate (Python 3.11+, dev/CI only; the service never runs Python):

    python -I catalog/fetch_sources.py
    python -I catalog/build_catalog.py catalog/upstream-src --revision <studio.tag_commit> --output catalog/catalog.json
    python -m unittest catalog/test_build_catalog.py

`upstream-src/` is git-ignored. The tests fail if the committed catalog is stale.

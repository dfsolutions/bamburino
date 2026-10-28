# Bamburino

An MCP bridge that lets AI agents prepare prints inside a live Bambu Studio window on Windows: a patched Bambu Studio build (the driver) plus a Rust Windows service.

Status: pre-alpha, nothing verified yet.

Tests: `pwsh tools/tests/run.ps1` runs the unit tests; the integration tests (they re-apply the driver patches on
the build clone at `D:\bb\BambuStudio`) run with `-Integration`, or when the clone exists and no build is running.

## License

AGPL-3.0. See [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md).

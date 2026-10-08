# `conda ship` reference

Most users run conda-ship as `cs`.

When `conda-ship` is installed in a conda environment, it can also add a
`conda ship` command as a shortcut for the same builder:

```bash
conda ship inspect
conda ship build
```

`conda ship ...` runs the installed `cs` executable. It forwards your arguments
and can resolve a runtime version from Python project metadata.

Packaged builds find the runtime template installed next to `cs`
automatically. Source checkouts need an installed template, a
`CONDA_SHIP_TEMPLATE` environment variable, or an explicit `--template` path.

## Packaging details

The PyPI package installs the Python adapter and the Rust-built `cs` executable
together. `conda-ship` looks for `cs` next to the current Python interpreter.
It does not search `PATH`, so `conda ship` cannot accidentally run an unrelated
`cs` executable from another environment. A future conda package should use the
same layout.

Packages must install these pieces into the same environment:

- the Rust-built `cs` executable
- the Rust-built `cs-template` runtime template
- the Python `conda_ship` adapter package

For custom packaging or tests, set `CONDA_SHIP_EXECUTABLE` to an explicit
executable path. An invalid value causes an error, even when a packaged `cs`
is available.

## Argument forwarding

Arguments after `conda ship` are passed to `cs`:

```bash
conda ship build --artifact-layout embedded
```

When you need to pass an argument that conda's own parser would consume, insert
`--` before the conda-ship arguments:

```bash
conda ship -- --help
```

Running `conda ship` without arguments shows `cs --help`.

## Project metadata versions

When `[tool.conda-ship]` contains
`runtime-version = { from = "project-metadata" }`, the Python adapter resolves
the version before invoking `cs build` or `cs run`. It calls the project's PEP
517 `prepare_metadata_for_build_wheel` hook, reads `Version` from the generated
wheel metadata, and forwards the concrete value as `--runtime-version`.

The adapter skips the metadata hook for `--dry-run`, help, and an explicit
`--runtime-version`. When the runtime version comes from project metadata,
pass `--runtime-version VERSION` for a dry-run preview.

For `conda ship build --manifest PATH`, the adapter reads the selected manifest
even when a higher-priority manifest exists beside it. Python project metadata
comes from `pyproject.toml` in the selected manifest's directory, including when
`--root` points elsewhere. Relative manifest paths resolve from the current
directory. Supported manifest names are `conda.toml`, `pixi.toml`, and
`pyproject.toml` with a nonempty `[tool.conda.workspace]` or
`[tool.pixi.workspace]` table. Unsupported selections pass through to `cs`
without running the metadata hook.

The build backend must already be installed in the Python environment running
`conda ship`. The adapter does not build a wheel if the metadata hook is
unavailable.

Direct `cs build` invocations do not run Python packaging hooks. Use
`conda ship build` for this source, or pass `cs build --runtime-version VERSION`.

## Error handling

`conda ship` asks `cs` for structured builder diagnostics and translates them
into conda errors. Direct `cs` invocations keep their usual terminal formatting.

For example, when a source lockfile is missing, `cs` reports a stable diagnostic
kind to the adapter, and `conda ship` shows the message and hint without
printing raw JSON.

# Environment Variables

This page lists environment variables used by conda-ship and generated
runtimes.

## Builder Variables

`CONDA_SHIP_TEMPLATE`
: Path to a prebuilt generic runtime template. `cs build` uses this when
  `--template` is not supplied. A nonempty value takes precedence over the
  installed template next to `cs`. An invalid path causes an error.

`CONDA_SHIP_EXECUTABLE`
: Path used by the Python `conda ship` adapter to find the `cs` executable.
  This is mainly useful for source checkouts, tests, and custom packaging. If
  set, it must point to an executable file. The adapter does not fall back to
  another `cs` when this value is invalid.

`CONDA_SHIP_ERROR_FORMAT`
: Internal diagnostic format used by the Python `conda ship` adapter. When set
  to `json`, `cs` writes a single structured JSON diagnostic line for builder
  failures. The adapter parses that line and renders a normal conda-facing
  error. Users normally do not need to set this themselves.

`SOURCE_DATE_EPOCH`
: Unix timestamp in whole seconds used for the generated SBOM's creation time.
  Use it for reproducible SBOM timestamps. When unset, the build records the
  current UTC time. Invalid or unsupported timestamps cause a build error.

## Runtime Variables

`CONDA_SHIP_PREFIX`
: Universal managed-prefix override. It takes precedence over a
  runtime-specific prefix variable. Use this for a runtime named `conda`
  because that runtime does not treat `CONDA_PREFIX` as an install override.
  `CONDA_PREFIX` can describe an activated conda environment and must not
  redirect the runtime bootstrap.

The remaining runtime variable names are derived from `runtime-name`.
Non-alphanumeric characters become underscores and letters are uppercased.

`RUNTIME_PREFIX`
: Override the managed prefix path. For a runtime named `demo`, the variable is
  `DEMO_PREFIX`. This compatibility variable is accepted for runtime names
  other than `conda`. `CONDA_SHIP_PREFIX` takes precedence when both are set.

`RUNTIME_BUNDLE`
: Runtime-specific path to an external package bundle directory. The actual
  variable name is based on the runtime name. Non-alphanumeric characters become
  underscores and letters are uppercased. For `demo`, the variable is
  `DEMO_BUNDLE`.

`RUNTIME_OFFLINE`
: Runtime-specific flag for offline bootstrap mode. For `demo`, the variable is
  `DEMO_OFFLINE`. Empty, `0`, and `false` disable the flag. Other non-empty
  values enable it. The comparison with `false` is case-insensitive.

## Runtime Update Coordinator Variables

These variables configure the version-one helper used by downstream
transaction coordinators. They are intended for integration code.
See {doc}`runtime-cli` for the required bootstrap, lock, and call sequence.

`CONDA_SHIP_INTERNAL_UPDATE`
: Select the helper action. Version one defines `v1/record-installation`,
  `v1/check`, `v1/stage`, and `v1/apply`. Generated runtimes do not reserve a
  delegate subcommand for these operations.

`CONDA_SHIP_INTERNAL_UPDATE_CANDIDATE`
: Lowercase SHA256 selected from the `v1/check` result. It is required by
  `v1/stage` and is ignored by the other actions.

`CONDA_SHIP_INTERNAL_UPDATE_OFFLINE`
: Disable network access for update resolution and staging. Empty, `0`, and
  `false` disable the flag. The comparison with `false` is case-insensitive.
  Other non-empty values enable it. An HTTPS update requires previously cached
  repodata and package content in this mode. A `file://` channel is read directly.

`CONDA_SHIP_INTERNAL_UPDATE_OWNERSHIP`
: Installed executable ownership for `v1/record-installation`. Supported values
  are `direct` and `external`.

`CONDA_SHIP_INTERNAL_UPDATE_INSTALLATION`
: Non-blank installation label for `v1/record-installation`, such as
  `standalone`, `constructor`, `homebrew`, `pipx`, or `uv-tool`. The value is
  stored unchanged.

`CONDA_SHIP_INTERNAL_UPDATE_EXECUTABLE`
: Optional absolute stable executable path for `v1/record-installation`. The
  path must resolve to the running executable. When omitted, the runtime uses
  its invocation path.

`CONDA_SHIP_INTERNAL_UPDATE_INSTRUCTION`
: Optional non-empty external-manager instruction for
  `v1/record-installation`. Direct ownership rejects this variable.

## Delegate Environment

The runtime executes the configured delegate from the managed prefix without
presenting that prefix as an activated conda environment. It does not set
`CONDA_PREFIX`, `CONDA_DEFAULT_ENV`, or `CONDA_SHLVL`. Delegate arguments and
the inherited process streams remain unchanged.

`PATH`
: Managed-prefix executable directories are prepended to the inherited value so
  the delegate and its child processes can find installed commands and shared
  libraries. This changes `PATH` only. It does not activate the prefix.

## Test And Development Variable

`CONDA_SHIP_ALLOW_UNSTAMPED_TEMPLATE`
: Allows the generic runtime template binary to run without stamped runtime
  data. This is used by tests. Downstream runtimes should not set it.

```{warning}
Do not set `CONDA_SHIP_ALLOW_UNSTAMPED_TEMPLATE` in distribution builds or user
environments. It exists only so tests can exercise the generic template binary.
```

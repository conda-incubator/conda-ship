# Builder CLI Reference

The `cs` CLI builds and stages conda runtimes.

This page covers the builder CLI. For automatic bootstrap and delegate
execution in generated runtimes, see {doc}`runtime-cli`.

The `conda-ship` package can also make `conda ship` available as a
conda-style shortcut for this CLI. See {doc}`conda-plugin`.

Packaged `cs` builds find the installed runtime template next to the `cs`
executable. Pass `--template` only when you need to override that template, use
an explicit release asset, or cross-build for another target.

## `cs inspect`

Inspect the project input and derived runtime package set without writing files.
Use this as a preflight check before a local build or release job.

```bash
cs inspect [--platform PLATFORM] [--json] [--root PATH]
```

Options:

- `--platform PLATFORM`: inspect a conda platform such as `linux-64`.
- `--json`: emit machine-readable JSON with project input, validation,
  exclusions, platform summaries, and packages for the selected platform.
- `--root PATH`: use a build root instead of auto-detecting one.

## `cs build`

Build and stage a runtime artifact.

Each build also writes a CycloneDX 1.7 SBOM for the resolved conda package
graph. Its filename uses the artifact stem and ends in `.cdx.json`.

```bash
cs build [--runtime-name RUNTIME] [--artifact-name NAME] \
  [--delegate-executable EXECUTABLE] [--artifact-layout LAYOUT] [--target-label LABEL] \
  [--platform PLATFORM] [--target TRIPLE] [--template PATH] \
  [--runtime-version VERSION] [--docs-url URL] [--install-scheme SCHEME] \
  [--install-name NAME] [--installer INSTALLER] \
  [--out-dir PATH] [--dry-run] [--root PATH]
```

Identifier-like values such as `RUNTIME`, `NAME`, `EXECUTABLE`, `LABEL`,
`TRIPLE`, and `INSTALLER` must start with an ASCII letter or digit and may only
contain ASCII letters, digits, `.`, `_`, and `-`. `RUNTIME` is the base runtime
identity and default artifact name. It is not a conda environment name.
Runtime metadata can come from CLI flags or `[tool.conda-ship]`.
For the difference between runtime names, artifact names, install names, and
runtime versions, see {doc}`names`.

Options:

- `--runtime-name RUNTIME`: override `[tool.conda-ship].runtime-name`.
- `--artifact-name NAME`: override `[tool.conda-ship].artifact-name` for the
  staged executable and artifact stem. When the flag is omitted, the manifest
  value is used. Without either setting, artifacts use the runtime name.
- `--delegate-executable EXECUTABLE`: override `[tool.conda-ship].delegate-executable`.
- `--runtime-version VERSION`: version stamped into generated runtime metadata.
  Overrides `[tool.conda-ship].runtime-version`, `[project].version`, and
  project metadata resolution.
- `--artifact-layout online`: stage a runtime that downloads packages during bootstrap.
- `--artifact-layout external`: stage a runtime plus compressed bundle.
- `--artifact-layout embedded`: stage a runtime with the compressed bundle embedded.
  When omitted, `cs` uses `[tool.conda-ship].artifact-layout` or `online`.
- `--target-label LABEL`: append a platform or target label to artifact names.
- `--platform PLATFORM`: choose the conda platform for metadata and bundles.
- `--target TRIPLE`: target triple used for artifact naming and template
  selection. It also selects the staged `.exe` suffix for Windows artifacts.
  Path-like custom target specifications are not supported here.
- `--template PATH`: prebuilt generic runtime template binary to copy and
  stamp. When omitted, `CONDA_SHIP_TEMPLATE` takes precedence over the template
  installed next to `cs`. Supplying `--target` disables installed-template lookup.
- `--docs-url URL`: documentation URL stamped into runtime metadata. Must
  start with `https://` or `http://` and must not contain whitespace or control
  characters.
- `--install-scheme SCHEME`: install scheme stamped into the runtime. Currently
  supported: `conda-home`, which installs below `~/.conda/INSTALL_NAME`, and
  `user-data`, which installs below the platform user data directory.
- `--install-name NAME`: directory name for this runtime's managed base prefix
  under the install scheme. Overrides the manifest `install-name` setting,
  including a derived name. Without either setting, the install name is the
  runtime name.
- `--installer INSTALLER`: package manager or installer stamped into runtime
  metadata. Overrides `[tool.conda-ship].installer`.
- `--out-dir PATH`: write staged artifacts somewhere other than `dist/`.
- `--dry-run`: validate the build input and print the planned artifacts without
  downloading, stamping, or writing files.
- `--root PATH`: use a project root instead of auto-detecting one.

## `cs run`

Build a runtime artifact and execute it immediately.

```bash
cs run [--runtime-name RUNTIME] [--artifact-name NAME] \
  [--delegate-executable EXECUTABLE] [--artifact-layout LAYOUT] [--platform PLATFORM] \
  [--template PATH] [--runtime-version VERSION] [--docs-url URL] \
  [--install-scheme SCHEME] [--install-name NAME] [--installer INSTALLER] \
  [--install-path PATH] [--out-dir PATH] [--root PATH] \
  -- RUNTIME_ARGS...
```

Everything after `--` is passed unchanged to the configured delegate after the
staged runtime automatically bootstraps if needed.

Options:

- `--runtime-name RUNTIME`: override `[tool.conda-ship].runtime-name`.
- `--artifact-name NAME`: override `[tool.conda-ship].artifact-name` for the
  staged executable and artifact stem. When the flag is omitted, the manifest
  value is used. Without either setting, artifacts use the runtime name.
- `--delegate-executable EXECUTABLE`: override `[tool.conda-ship].delegate-executable`.
- `--runtime-version VERSION`: version stamped into generated runtime metadata.
  Overrides `[tool.conda-ship].runtime-version`, `[project].version`, and
  project metadata resolution.
- `--artifact-layout online`: stage a runtime that downloads packages during bootstrap.
- `--artifact-layout external`: stage a runtime plus compressed bundle.
- `--artifact-layout embedded`: stage a runtime with the compressed bundle embedded.
  When omitted, `cs` uses `[tool.conda-ship].artifact-layout` or `online`.
- `--platform PLATFORM`: choose the conda platform for metadata and bundles.
- `--template PATH`: prebuilt generic runtime template binary to copy and
  stamp. When omitted, `CONDA_SHIP_TEMPLATE` takes precedence over the template
  installed next to `cs`.
- `--docs-url URL`: documentation URL stamped into runtime metadata. Must
  start with `https://` or `http://` and must not contain whitespace or control
  characters.
- `--install-scheme SCHEME`: install scheme stamped into the runtime. Currently
  supported: `conda-home` and `user-data`.
- `--install-name NAME`: directory name for this runtime's managed base prefix
  under the install scheme. Overrides the manifest `install-name` setting,
  including a derived name. Without either setting, the install name is the
  runtime name.
- `--installer INSTALLER`: package manager or installer stamped into runtime
  metadata.
- `--install-path PATH`: managed prefix path used by the staged runtime for
  this smoke-test invocation.
- `--out-dir PATH`: write staged artifacts somewhere other than `dist/`.
- `--root PATH`: use a project root instead of auto-detecting one.
- `RUNTIME_ARGS`: arguments passed unchanged to the configured delegate after
  the staged runtime is built and bootstrapped if needed.

## `cs package-update`

Wrap a finalized runtime executable in a native conda package for direct
executable updates.

```bash
cs package-update --info PATH [--binary PATH] [--out-dir PATH] [--json]
```

The command reads the artifact info JSON written by `cs build`. Without
`--binary`, it packages the recorded runtime executable and requires its size
and SHA256 to match the artifact info. Use `--binary` after signing or another
release step changes the executable bytes. The finalized executable must still
contain a readable conda-ship stamp.

The command validates:

- the artifact info schema
- runtime name, artifact name, version, layout, and native platform
- executable update configuration and source
- the executable stamp and embedded bundle, when present
- the recorded executable checksum when `--binary` is omitted

Only update-enabled `online` and `embedded` runtimes are accepted. The output
is a dependency-free native `.conda` package. It contains one executable
payload at `bin/ARTIFACT_NAME` on Unix or
`ARTIFACT_NAME.exe` on Windows, plus normal conda package metadata.

The output filename is `PACKAGE-VERSION-BUILD_NUMBER.conda`. The command
refuses to overwrite an existing file. By default it writes next to `--info`.
Use `--out-dir` to select another directory.

Without `--json`, stdout contains the output path. With `--json`, stdout
contains one object with:

- schema version
- path and filename
- package name, runtime version, build number, and platform
- package SHA256 and size
- executable payload SHA256 and size

`cs package-update` does not index a channel or upload the package. Downstream
release tooling owns those provider-specific operations.

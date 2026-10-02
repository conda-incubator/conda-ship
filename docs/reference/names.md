# Runtime And Artifact Names

conda-ship uses several names because one build has several audiences: users
run a command, release jobs upload files, and the generated runtime manages an
install location. Keep these names the same unless a downstream distribution has
a reason to separate them.

## Quick Choice

For most projects, configure the normal build policy and let `runtime-name`
drive the other name defaults:

```toml
[tool.conda-ship]
runtime-name = "demo"
runtime-version = "1.0.0"
delegate-executable = "conda"
artifact-layout = "online"
source-environment = "ship"
```

This stages `dist/demo`, stamps `demo` into runtime metadata, writes install
metadata such as `.demo.json`, and uses runtime-specific environment variables
such as `DEMO_PREFIX`, `DEMO_BUNDLE`, and `DEMO_OFFLINE`.

`CONDA_SHIP_PREFIX` is the universal prefix override and takes precedence over
the runtime-specific name. A runtime named `conda` does not treat
`CONDA_PREFIX` as an override because it may refer to an activated environment.

Add `artifact-name` only when the release command or file stem should differ
from the base runtime identity:

```toml
[tool.conda-ship]
runtime-name = "cx"
artifact-name = "cxz"
```

This stages an artifact such as `dist/cxz`, while display, install metadata,
and environment variable names continue to use `cx`.

Add `install-name` only when the managed install location should use a different
name:

```toml
[tool.conda-ship]
runtime-name = "cx"
install-name = "express"
```

This keeps the user-facing runtime identity `cx`, but installs below the
install scheme path for `express`, such as `~/.conda/express` with the default
`conda-home` scheme.

## Name Fields

`runtime-name`
: Base runtime identity and default artifact name. This is the name users see
  in runtime metadata, install ownership metadata, and runtime-specific
  environment variables. It is required unless passed as `--runtime-name` or the
  GitHub Action `runtime-name` input.

`artifact-name`
: Optional staged executable and artifact stem. This is the name used for files
  written to `dist/`, including the staged runtime, `.info.json`,
  `.runtime.lock`, `.packages.txt`, `.cdx.json`, `.sha256`, and the external
  bundle stem.
  When omitted, it defaults to `runtime-name`.

`install-name`
: Optional directory name for this runtime's managed base prefix under the
  selected install scheme. A string is used literally. The derived form
  `{ from = "runtime-lock" }` appends the runtime version and a 16-character
  prefix of the rendered runtime lock SHA-256 to `runtime-name`. Set its
  optional `base` field to use another base. When omitted, the install name
  defaults to `runtime-name`. An explicit `--install-name` remains a final-name
  override.
  See {doc}`../explanation/install-locations-and-ownership` for lifecycle and
  security details.

## Related Fields

`delegate-executable`
: Executable inside the managed prefix that receives every runtime argument.
  It is an executable name, not a path.

`artifact-layout`
: How package archives are distributed: `online`, `external`, or `embedded`.

`exclude-packages`
: Package names removed from the derived runtime lock.

`installer`
: Package manager or installer metadata. It is not part of the install-location
  controls.

## Runtime Version

`runtime-version` is related to `runtime-name` only because both are stamped
runtime metadata. It is not derived from `runtime-name`, and changing one does
not change the other.

Use `runtime-name` to choose the runtime identity and default file stem. Use
`runtime-version` to choose the version recorded in runtime and prefix ownership
metadata. A runtime invocation such as `RUNTIME --version` belongs to the
delegate.

`runtime-version` can come from:

- `[tool.conda-ship].runtime-version`
- `cs build --runtime-version`
- the GitHub Action `runtime-version` input
- static `[project].version`
- `runtime-version = { from = "project-metadata" }` when the Python
  `conda ship` adapter or GitHub Action resolves project metadata

## Examples

Default naming:

```toml
[tool.conda-ship]
runtime-name = "demo"
runtime-version = "1.0.0"
```

Stages:

```text
dist/demo
dist/demo.info.json
dist/demo.runtime.lock
dist/demo.packages.txt
dist/demo.cdx.json
dist/demo.sha256
```

Separate artifact name:

```toml
[tool.conda-ship]
runtime-name = "cx"
artifact-name = "cxz"
runtime-version = "1.0.0"
```

Stages:

```text
dist/cxz
dist/cxz.info.json
dist/cxz.runtime.lock
dist/cxz.packages.txt
dist/cxz.cdx.json
dist/cxz.sha256
```

The generated runtime still uses `cx` for display and runtime-specific
environment variables. Its managed prefix defaults to `~/.conda/cx`.
Set `install-name` or `install-scheme` to change that default location.

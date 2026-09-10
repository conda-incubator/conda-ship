# Customize A Runtime

Use this guide when you want a conda-ship-built runtime with your own package
set, runtime name, delegate executable, install location, channels, or
documentation URL.

conda-ship is generic. It does not publish a first-party runtime, and it does
not reserve a default runtime name. [conda-express](https://jezdez.github.io/conda-express/)
is one downstream distribution that uses conda-ship to publish `cx` and `cxz`;
use a runtime name owned by your distribution.

For exact field definitions and alternate manifest formats, see
{doc}`../reference/configuration`.

## Choose Runtime Identity

Set `runtime-name` to the command and base identity users should see:

```toml
[tool.conda-ship]
runtime-name = "demo"
runtime-version = "0.1.0"
delegate-executable = "conda"
source-environment = "ship"
```

Add `artifact-name` only when the staged command or release file stem should
differ from the base runtime identity:

```toml
[tool.conda-ship]
runtime-name = "demo"
artifact-name = "demo-offline"
```

Avoid publishing downstream builds as `cx` or `cxz`; those names identify the
conda-express artifacts maintained in the `jezdez/conda-express` repository.

## Choose An Install Location

By default, a runtime uses the `conda-home` install scheme and installs below
`~/.conda/RUNTIME`, where `RUNTIME` is the runtime name. Use `install-name`
when a short command should install into a clearer directory:

```toml
[tool.conda-ship]
runtime-name = "cx"
install-scheme = "conda-home"
install-name = "express"
```

That builds a runtime named `cx` whose default install path resolves to
`~/.conda/express` on the user's machine. Users can still override the resolved
path with the prefix variable derived from the runtime name, such as
`CX_PREFIX=/tmp/express cx info`.

Use `install-scheme = "user-data"` when the runtime should install below the
platform user data directory instead of `~/.conda`.

If a package manager owns the runtime binary, set `installer` in the manifest
or pass it from the release job so the provider is retained in runtime
metadata:

```toml
[tool.conda-ship]
runtime-name = "demo"
installer = "homebrew"
```

During automatic bootstrap, that stamp writes Constructor-compatible
`.installer.info` JSON into the managed prefix. Its `name`, `version`, and
`platform` fields identify the stamped distribution, while `type` contains the
configured `installer` value. This metadata identifies the distribution and
installer. It is not a launcher ownership record.

## Choose Runtime Packages

The selected source environment is the complete runtime package set.
conda-ship does not add or require packages by name. Include the configured
delegate executable and everything that delegate needs. A conda distribution
usually includes `python`, `conda`, its selected solver plugin, and
`conda-spawn` when it exposes the `conda shell` alias.

Record the complete package set in the selected source environment and commit
the matching lockfile.

Add `conda-self` when the generated runtime should let users reset the managed
base prefix back to the packages shipped by the runtime:

```toml
[feature.ship.dependencies]
python = ">=3.12"
conda = ">=25.1"
conda-rattler-solver = "*"
conda-spawn = ">=0.1.0"
conda-self = "*"
```

conda-ship writes `conda-meta/initial-state.explicit.txt` during bootstrap.
`conda-self` treats that file as the installer snapshot for reset commands.

## Configure Condarc And Base Freezing

By default, conda-ship does not create `.condarc` or freeze the managed base
prefix. A downstream conda distribution can opt into both behaviors:

```toml
[tool.conda-ship]
condarc-file = "runtime.condarc"
freeze-base = true
```

Keep `runtime.condarc` as YAML next to the selected manifest:

```yaml
channels:
  - conda-forge
solver: rattler
auto_activate_base: false
notify_outdated_conda: false
show_channel_urls: true
```

The builder validates that the file contains a YAML mapping and stamps its
exact text. It does not derive or merge lockfile channels into this file.
Omitting `condarc-file` leaves `.condarc` alone. Leaving `freeze-base` false
also preserves any frozen marker created by an installed package.

## Configure Build Input

Keep package and channel intent in the manifest format owned by your workspace
tool. Keep conda-ship build policy in `[tool.conda-ship]`.

For `conda.toml`, a minimal downstream runtime project looks like this:

```toml
[workspace]
name = "demo"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[feature.ship.dependencies]
python = ">=3.12"
conda = ">=25.1"
conda-rattler-solver = "*"
conda-spawn = ">=0.1.0"
numpy = "*"
pandas = "*"

[environments]
ship = { features = ["ship"], no-default-feature = true }

[tool.conda-ship]
runtime-name = "demo"
runtime-version = "0.1.0"
delegate-executable = "conda"
artifact-layout = "online"
source-environment = "ship"
exclude-packages = ["conda-libmamba-solver"]
docs-url = "https://example.com/demo/"
install-scheme = "conda-home"
install-name = "demo"
installer = "homebrew"
```

Refresh the source lockfile:

```bash
conda workspace lock
```

For `pyproject.toml` and Pixi layouts, keep the same `[tool.conda-ship]` policy
but place workspace package data under the tool-specific sections documented in
{doc}`../reference/configuration`.

## Build Locally

Build the runtime:

```bash
cs build
```

The staged runtime and metadata files are written to `dist/`.

## Build In GitHub Actions

For CI builds, commit the manifest and lockfile, then point the composite action
at that project root:

```yaml
- uses: actions/checkout@v4

- uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  id: cs
  with:
    conda-ship-version: "X.Y.Z"
    root: .
```

The action does not run `conda workspace lock`, `pixi lock`, or any other solve
step. That keeps release artifacts tied to reviewed project files.

## Build An Embedded Variant

Use the `embedded` layout when you want a larger single binary that carries the
package archives inside itself:

```bash
cs build --artifact-layout embedded
```

The embedded runtime uses `runtime-name` by default. Set
`artifact-name = "demo-offline"` or pass `--artifact-name demo-offline` when a
release artifact should have a distinct command name.

The embedded runtime detects its built-in bundle automatically during the first
invocation. Users do not need to set the bundle or offline environment
variables.

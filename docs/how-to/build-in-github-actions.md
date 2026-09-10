# Build In GitHub Actions

Use the composite action when a downstream distribution repository wants
conda-ship to build release artifacts in CI.

The action reads committed project input, downloads released conda-ship builder
assets, verifies them, runs `cs build --dry-run`, and then runs `cs build`.
It does not solve, generate a manifest, or refresh a lockfile.

For a full guided workflow, see {doc}`../tutorials/github-action-runtime`.
For exact inputs and outputs, see {doc}`../reference/github-action`.

## Prepare The Repository

Commit one supported manifest and lockfile pair:

- `conda.toml` and `conda.lock`
- `pixi.toml` and `pixi.lock`
- `pyproject.toml` with nonempty `[tool.conda.workspace]` and `conda.lock`
- `pyproject.toml` with nonempty `[tool.pixi.workspace]` and `pixi.lock`

Before pushing a release workflow, run the local preflight:

```bash
cs inspect
cs build --dry-run
```

## Add A Build Job

Pin the action source to a full conda-ship release commit SHA and pass the
matching conda-ship release through `conda-ship-version`:

```yaml
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1

      - uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
        id: cs
        with:
          conda-ship-version: "X.Y.Z"

      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ${{ steps.cs.outputs.asset-name }}
          path: ${{ steps.cs.outputs.dist-path }}
```

When the action is invoked by an exact release tag, `conda-ship-version` can be
omitted for backwards compatibility. Release workflows should prefer full
commit SHA pins.

## Build From A Subdirectory

When the downstream manifest lives below the repository root, set `root`:

```yaml
- uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  id: cs
  with:
    conda-ship-version: "X.Y.Z"
    root: dist/demo
```

Update and commit the lockfile before running release builds.

## Choose Layout Or Metadata At Release Time

Keep package and channel choices in the manifest and lockfile. Use action inputs
for release-job metadata that may vary across jobs:

```yaml
- uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  id: cs
  with:
    conda-ship-version: "X.Y.Z"
    artifact-layout: embedded
    runtime-version: ${{ github.ref_name }}
    installer: homebrew
```

The action passes non-empty inputs to `cs build --dry-run`, so invalid values
fail in conda-ship before artifact files are written.

## Matrix Builds

Matrix the operating system, layout, and release-channel metadata when a release
needs platform-specific outputs:

```yaml
strategy:
  fail-fast: false
  matrix:
    include:
      - os: ubuntu-latest
        layout: online
        installer: standalone
      - os: macos-15
        layout: embedded
        installer: homebrew
      - os: windows-latest
        layout: online
        installer: standalone

runs-on: ${{ matrix.os }}

steps:
  - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1

  - uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
    id: cs
    with:
      conda-ship-version: "X.Y.Z"
      artifact-layout: ${{ matrix.layout }}
      runtime-version: ${{ github.ref_name }}
      installer: ${{ matrix.installer }}
```

Each job emits an asset name qualified with the runner target triple.

## Publish Or Attest Outputs

Use `dist-path` as the source of truth for artifact uploads. It contains the
runtime, optional external bundle, `.info.json`, `.runtime.lock`,
`.packages.txt`, `.cdx.json`, and `.sha256` files for that build.

For release workflows, attest the complete `dist-path` before publishing or
wrapping the files. See {doc}`verify-release-artifacts` for the GitHub Actions
attestation recipe.

# Build A Runtime In GitHub Actions

This tutorial takes a committed conda-ship project and builds runtime artifacts
with the composite GitHub Action.

You will add a workflow that downloads released conda-ship build tools, verifies
them, runs `cs build --dry-run`, builds the runtime, and uploads the generated
`dist` directory as a workflow artifact.

## Before You Start

You need a repository that already contains one supported manifest and lockfile
pair:

- `conda.toml` and `conda.lock`
- `pixi.toml` and `pixi.lock`
- `pyproject.toml` with nonempty `[tool.conda.workspace]` and `conda.lock`
- `pyproject.toml` with nonempty `[tool.pixi.workspace]` and `pixi.lock`

The manifest must contain `[tool.conda-ship]` with at least:

```toml
[tool.conda-ship]
runtime-name = "demo"
runtime-version = "0.1.0"
delegate-executable = "conda"
source-environment = "ship"
```

Run the local preflight before committing:

```bash
cs inspect
cs build --dry-run
```

## Add The Workflow

Create `.github/workflows/build-runtime.yml`:

```yaml
name: Build runtime

on:
  workflow_dispatch:
  push:
    branches: [main]

permissions:
  contents: read
  id-token: write
  attestations: write
  artifact-metadata: write

jobs:
  build:
    name: Build ${{ matrix.os }} ${{ matrix.layout }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            layout: online
            installer: standalone
          - os: macos-15-intel
            layout: embedded
            installer: homebrew
          - os: macos-15
            layout: embedded
            installer: homebrew
          - os: windows-latest
            layout: online
            installer: standalone

    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1

      - uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
        id: cs
        with:
          conda-ship-version: "X.Y.Z"
          artifact-layout: ${{ matrix.layout }}
          installer: ${{ matrix.installer }}

      - uses: actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6 # v4.2.2
        with:
          subject-path: ${{ steps.cs.outputs.dist-path }}/*

      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ${{ steps.cs.outputs.asset-name }}
          path: ${{ steps.cs.outputs.dist-path }}
```

Pin the action source to a full conda-ship release commit SHA and pass the
matching conda-ship release through `conda-ship-version`.

```{warning}
Use the latest reviewed `actions/attest` release in your workflow and pin it by
commit SHA. The SHA above is an example, not a recommendation to keep using that
exact revision indefinitely.
```

## Run It

Push the workflow and start it from the GitHub Actions tab, or wait for the next
push to `main`.

The action downloads these release assets for the current runner:

- `cs-<target>`
- `cs-template-<target>`
- `SHA256SUMS`

It verifies GitHub artifact attestations and the checksums before running the
downloaded `cs` binary.

The workflow also attests the generated runtime output directory before
uploading it. That downstream attestation covers the runtime binary,
`.runtime.lock`, `.packages.txt`, `.cdx.json`, `.info.json`, `.sha256`, and any
external bundle produced by that job.

## Inspect The Artifact

Each job uploads the full generated output directory. Download one artifact and
inspect the files:

```text
demo-x86_64-unknown-linux-gnu
demo-x86_64-unknown-linux-gnu.info.json
demo-x86_64-unknown-linux-gnu.packages.txt
demo-x86_64-unknown-linux-gnu.runtime.lock
demo-x86_64-unknown-linux-gnu.cdx.json
demo-x86_64-unknown-linux-gnu.sha256
```

For an `external` build, the directory also contains
`demo-<target>.bundle.tar.zst`. For an `embedded` build, the runtime carries
the bundle inside the binary and uses the configured runtime name unless
`artifact-name` sets a distinct artifact name.

## Override Runtime Metadata

Keep package and channel choices in the manifest and lockfile. Use action inputs
for release-job metadata that may vary across a matrix:

```yaml
- uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  id: cs
  with:
    conda-ship-version: "X.Y.Z"
    runtime-name: demo
    delegate-executable: conda
    artifact-layout: ${{ matrix.layout }}
    docs-url: https://example.com/demo/
    install-scheme: conda-home
    install-name: demo
    installer: ${{ matrix.installer }}
```

The action does not validate those values itself. It passes them to
`cs build --dry-run`. Invalid values fail in conda-ship before artifact files
are written.

## What You Learned

The workflow builds runtime artifacts from the committed manifest and lockfile.
Use conda-workspaces or Pixi to update those files when packages change. The
action reads them and applies any runtime metadata overrides from the job.

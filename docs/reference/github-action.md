# GitHub Action Reference

The repository root provides a composite GitHub Action for downstream
distribution repositories.

The action downloads the configured conda-ship release assets for the current
runner, verifies their GitHub artifact attestations and `SHA256SUMS`, and runs
the downloaded `cs` binary to preflight and build a runtime. The preflight uses
`cs build --dry-run`, then the action runs the real build. It does not build
conda-ship from source.
Self-hosted runners must provide the GitHub CLI because attestation
verification uses `gh attestation verify`.

The action builds only from committed project input. The selected root must
contain `conda.toml` plus `conda.lock`, `pyproject.toml` with `[tool.conda]`
plus `conda.lock`, `pixi.toml` plus `pixi.lock`, or `pyproject.toml` with
`[tool.pixi]` plus `pixi.lock`. When the manifest or matching lockfile is
missing, the action fails instead of generating or solving project configuration
in CI. This minimal example assumes the manifest contains
`[tool.conda-ship].runtime-name`,
`[tool.conda-ship].delegate-executable`, and a downstream runtime version.

When the selected conda-ship config sets
`runtime-version = { from = "project-metadata" }`, the action first lets
`cs build --dry-run` report that project metadata resolution is required. It
then sets up Python with `actions/setup-python`, resolves the downstream
project version through `pypa/build`, and retries the build with an explicit
`--runtime-version`. Static runtime versions and explicit `runtime-version`
inputs do not set up Python.

```yaml
- uses: actions/checkout@v4

- uses: jezdez/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  id: cs
  with:
    conda-ship-version: "X.Y.Z"
```

## Inputs

`conda-ship-version`
: conda-ship release version to download, such as `0.3.0`. Set this when the
  action source is pinned by full commit SHA. When omitted, the action uses the
  exact action tag if available.

`runtime-name`
: Runtime name override. Set this when the release job intentionally stamps a
  different runtime name than `[tool.conda-ship].runtime-name`.

`artifact-name`
: Staged executable and artifact stem override. Set this when any layout should
  stage a different command and release artifact name than `runtime-name`, such
  as `cxz` for a distribution whose base runtime name is `cx`. When omitted,
  artifacts use the resolved `runtime-name` exactly.

`delegate-executable`
: Delegate executable override. Set this when the release job intentionally
  changes which executable receives runtime arguments.

`runtime-version`
: Runtime version override. Set this when the release job intentionally stamps
  a version different from `[tool.conda-ship].runtime-version` or
  `[project].version`, or when the manifest does not provide a downstream
  runtime version.

`python-version`
: Python version used only when the action must resolve
  `runtime-version = { from = "project-metadata" }`. Defaults to `3.12`.

`root`
: Project root containing `conda.toml`/`conda.lock`, `pixi.toml`/`pixi.lock`,
  or `pyproject.toml` with either `[tool.conda]`/`conda.lock` or
  `[tool.pixi]`/`pixi.lock`. Defaults to the workflow workspace.

`artifact-layout`
: Artifact layout to build. Supported values are `online`, `external`, and
  `embedded`. Overrides `[tool.conda-ship].artifact-layout` when set. Otherwise
  the action leaves layout selection to the manifest and `cs` defaults to
  `online`. External artifacts stage the runtime and bundle as separate files.
  Embedded artifacts carry package archives inside the runtime.

`docs-url`
: Documentation URL stamped into generated runtime metadata. Must start
  with `https://` or `http://` and must not contain whitespace or control
  characters.

`install-scheme`
: Install scheme stamped into the generated runtime. Supported values are
  `conda-home` and `user-data`.

`install-name`
: Directory name for this runtime's managed base prefix under the install
  scheme. When omitted, `cs` uses `[tool.conda-ship].install-name` or the
  resolved runtime name.

`installer`
: Package manager or installer stamped into runtime metadata.

The action does not duplicate `cs build` validation in shell. It passes
non-empty inputs to `cs build --dry-run` and then to `cs build`. Invalid values
fail in the builder.

## Supported Runner Platforms

The action selects `cs-<target>` and `cs-template-<target>` from the current
runner's operating system and architecture:

| Runner OS | Runner arch | Target | Support status |
| --- | --- | --- | --- |
| `Linux` | `X64` | `x86_64-unknown-linux-gnu` | End-to-end runtime bootstrap covered. |
| `Linux` | `ARM64` | `aarch64-unknown-linux-gnu` | End-to-end runtime bootstrap covered. |
| `macOS` | `X64` | `x86_64-apple-darwin` | End-to-end runtime bootstrap covered. |
| `macOS` | `ARM64` | `aarch64-apple-darwin` | End-to-end runtime bootstrap covered. |
| `Windows` | `X64` | `x86_64-pc-windows-msvc` | End-to-end runtime bootstrap covered. |
| `Windows` | `ARM64` | `aarch64-pc-windows-msvc` | Builder assets, template assets, PyPI wheels, action target mapping, and native structural runtime stamping are covered. Full runtime bootstrap is not end-to-end supported yet. |

Use GitHub-hosted or self-hosted runners that report one of those
`runner.os`/`runner.arch` combinations. Release workflows should pin the action
source by full commit SHA and pass the matching conda-ship release through the
`conda-ship-version` input.

## Outputs

`cs-path`
: Absolute path to the downloaded and verified `cs` builder. Use this after
  downstream signing or other finalization to run `cs package-update` with the
  action's `info-path` and finalized executable. The action does not generate
  or upload an update package itself.

`dist-path`
: Absolute path to the directory containing all generated runtime artifacts.
  Use this for artifact uploads when the complete build output should be
  published together.

`binary-path`
: Absolute path to the generated runtime.

`asset-name`
: Platform-qualified asset filename.

`info-path`
: Absolute path to the artifact info JSON.

`lock-path`
: Absolute path to the staged runtime lock.

`package-list-path`
: Absolute path to the staged package list.

`sbom-path`
: Absolute path to the CycloneDX 1.7 SBOM.

`checksums-path`
: Absolute path to the SHA256 checksum file.

`bundle-path`
: Absolute path to the external bundle when `artifact-layout: external`. Empty
  for `online` and `embedded`.

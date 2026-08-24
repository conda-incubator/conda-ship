# Changelog

All notable changes to `conda-ship` are documented here.

## Unreleased

### Fixed

- Extended the Mach-O `__LINKEDIT` segment over appended macOS runtime data and
  ad hoc signed native builds after stamping so generated runtimes remain valid
  Mach-O code and can receive a downstream Developer ID signature.
- Anchored runtime-data footer selection before the Mach-O code signature and
  in an Authenticode-covered PE `.cship` section so unsigned signature or
  certificate padding cannot shadow authenticated runtime identity and update
  policy.
- Made runtime stamping failure-atomic and bounded malformed Mach-O load-command
  allocation. Mach-O signatures larger than 16 MiB no longer hide a valid
  runtime stamp.
- Required a versioned reader ABI declaration in dedicated Mach-O and PE image
  sections. This rejects unmodified pre-fix templates whose embedded reader
  could search mutable signature data. Builders and templates for those
  platforms must now come from the same conda-ship release family.
- Rejected mismatched template architectures and image kinds, non-executable or
  non-macOS images, overlapping or overwritten Mach-O link-edit data, malformed
  PE section layouts, and PE32+ images beyond the loader size limit. Artifact
  signing and hashing now finish in restricted staging before per-artifact
  publication, with the checksum manifest published last. Changing a same-stem
  artifact from `external` to another layout now retires the obsolete external
  bundle.
- Snapshotted an embedded bundle after checksum verification so later pathname
  replacement cannot change the bytes passed to decompression and extraction.

## 0.8.0 - 2026-08-22

### Added

- Added opt-in derived install names with
  `install-name = { from = "runtime-lock" }`, allowing different runtime
  versions or locked package sets to use separate managed prefixes.
- Added a CycloneDX 1.7 `.cdx.json` SBOM for every staged runtime. It is
  recorded in `.info.json` and `.sha256`, and exposed through the GitHub Action
  `sbom-path` output.
- Added `RuntimeSpec::from_stamped_artifact` to the optional Fleet Rust API for
  deriving validated Fleet input from a selected conda-ship artifact.

### Fixed

- Initialized the default TLS provider before Fleet installs packages through
  the Rust API.
- Updated `h2` to 0.4.16, addressing RUSTSEC-2026-0258.
- Fixed release rehearsals to pair the released action with matching released
  builder assets.

## 0.7.0 - 2026-07-24

### Added

- Added `v1/record-installation` to record direct or external executable
  ownership, the installation kind, and the stable executable path in the
  existing runtime metadata file.

### Changed

- Installed executable ownership is now independent of the stamped update
  source. One direct-capable runtime can be delivered by a standalone
  installer, Homebrew, a Python package, or another external manager.
- New builds stamp only the update channel, package, and build number.
  `ownership` and `instruction` are rejected as build settings. An installer
  or delivery integration records them through `v1/record-installation`.
  Runtimes still read those fields from existing 0.6.x artifacts.
- External package-manager replacement preserves the recorded installation
  kind and verifies the replacement against the original runtime identity and
  update source.

### Fixed

- Updated rattler so Linux builds use `quick-xml` 0.41, addressing
  RUSTSEC-2026-0194 and RUSTSEC-2026-0195.

## 0.6.4 - 2026-07-23

### Fixed

- Added native platform and architecture metadata to executable update
  packages so Anaconda.org receives their complete upload identity.

## 0.6.3 - 2026-07-23

### Fixed

- Fixed deferred Windows executable replacement when helper output is captured
  by preventing the update worker from inheriting the helper's open handles.

## 0.6.2 - 2026-07-23

### Fixed

- Fixed verification of the released GitHub Action by configuring its required
  conda delegate.

## 0.6.1 - 2026-07-23

### Fixed

- Fixed verification of the released GitHub Action by providing the runtime
  identity required by its smoke build.

## 0.6.0 - 2026-07-23

### Added

- Added optional `[tool.conda-ship.update]` metadata for runtimes that discover
  executable releases through conda channel repodata. Online and embedded
  artifacts can select direct or external executable ownership.
- Added `cs package-update` to wrap one finalized, stamped native executable in
  a dependency-free `.conda` package. The GitHub Action now exposes the
  downloaded builder as `cs-path` for this packaging step.
- Added staged executable updates with package and stamp validation, cached
  offline resolution, Unix replacement, Windows deferred replacement,
  interrupted-update recovery, and reconciliation after external replacement.
- Added a version-one environment-driven helper so downstream transaction
  coordinators can hold `.RUNTIME_NAME.update.lock` across check, stage, an
  inner transaction, and apply. Successful helper calls return one JSON object.
- Added `CONDA_SHIP_PREFIX` as a common managed-prefix override for generated
  runtimes and update coordinators.

### Changed

- A runtime named `conda` no longer interprets an activated `CONDA_PREFIX` as
  its managed installation path. Use `CONDA_SHIP_PREFIX` for an explicit
  override.
- Runtimes without update configuration retain their existing behavior. Update
  configuration does not restrict the configured delegate executable.
- Fleet launcher ownership remains with Fleet callers and does not use the
  stamped executable update path automatically.

## 0.5.0 - 2026-07-21

### Added

- Added `condarc-file` and `freeze-base` build settings. Both are disabled by
  default and let downstream distributions choose whether bootstrap writes a
  `.condarc` or CEP 22 frozen marker.
- Added Constructor-compatible `.installer.info` prefix metadata when a build
  configures `installer`.
- Added `cs run --install-path` and a runtime-specific `_PREFIX` environment
  variable for choosing a managed prefix during local runs or deployment.
- Added the opt-in `conda_ship::fleet` Rust API behind the non-default `fleet`
  Cargo feature. It installs and runs multiple locked prefixes without changing
  the stamped binary-template workflow.

### Changed

- The selected source environment now defines the complete runtime package set.
  conda-ship no longer requires `conda`, `conda-rattler-solver`, or
  `conda-spawn`. It only requires the configured delegate executable.
- A generated runtime now bootstraps automatically on first invocation and
  passes every argument to its configured delegate unchanged. The runtime no
  longer reserves private `bootstrap`, `status`, `shell`, or `uninstall`
  commands.
- Delegate execution no longer sets conda activation variables or rewrites
  delegate output. Prefix executables remain available through `PATH`.
- Bootstrap now uses an adjacent process lock and an internal ownership marker.
  A later invocation can recover an interrupted bootstrap by reinstalling the
  complete locked package set without deleting unrelated prefix contents.

### Migration

- Include the delegate and every optional command in the selected source
  environment. Conda-based distributions that want `shell` can include a
  conda-spawn version with its `shell` alias. Include conda-self when the
  distribution should expose its reset or self-management commands.
- Replace use of the removed private runtime commands with the delegate's
  commands. For a conda delegate, use `RUNTIME info` for runtime information
  and the applicable conda or conda-self command for management operations.
- Set `condarc-file` or `freeze-base = true` explicitly if a distribution wants
  the configuration files that earlier conda-ship versions wrote by default.

## 0.4.0 - 2026-06-16

### Added

- Added `[tool.conda-ship].artifact-name`, `cs build --artifact-name`,
  `cs run --artifact-name`, and the GitHub Action `artifact-name` input for
  downstream distributions that want staged artifacts to use a distinct
  executable name for any layout.

### Changed

- Renamed `[tool.conda-ship].runtime`, `cs build --runtime`,
  `cs run --runtime`, and the GitHub Action `runtime` input to
  `runtime-name`.
- Embedded builds no longer append `z` to the runtime name automatically.
  `artifact-layout = "embedded"` now stages the configured `runtime-name` unless an
  explicit artifact name is provided.
- Renamed public configuration keys, CLI flags, and GitHub Action inputs for a
  clearer flat naming scheme.

### Migration

- Inside `[tool.conda-ship]`, rename `runtime = "demo"` to
  `runtime-name = "demo"`. Replace `--runtime demo` with
  `--runtime-name demo`, and use `runtime-name: demo` in GitHub Actions.
- Update the remaining renamed fields:

  | Old | New |
  | --- | --- |
  | `delegate = "conda"` | `delegate-executable = "conda"` |
  | `layout = "embedded"` | `artifact-layout = "embedded"` |
  | `exclude = ["conda-libmamba-solver"]` | `exclude-packages = ["conda-libmamba-solver"]` |
  | `install-method = "homebrew"` | `installer = "homebrew"` |
  | `cs build --delegate conda` | `cs build --delegate-executable conda` |
  | `cs build --layout embedded` | `cs build --artifact-layout embedded` |
  | `cs build --install-method homebrew` | `cs build --installer homebrew` |
  | GitHub Action `delegate: conda` | `delegate-executable: conda` |
  | GitHub Action `layout: embedded` | `artifact-layout: embedded` |
  | GitHub Action `install-method: homebrew` | `installer: homebrew` |

- If you currently rely on the old `runtime = "demo"` setting producing
  `dist/demoz` for embedded builds, set `artifact-name = "demoz"` in
  `[tool.conda-ship]`, pass `cs build --artifact-name demoz`, or set the
  GitHub Action `artifact-name: demoz` input.
- If you do not need a distinct embedded executable name, no additional
  artifact-name setting is needed. Update release scripts and artifact upload
  patterns to use `demo` instead of `demoz`.

## 0.3.2 - 2026-06-15

### Fixed

- Set an explicit `conda-ship/<version>` user agent for package archive
  downloads. This fixes 403 responses from `repo.anaconda.com` during embedded
  bundle creation and online runtime bootstrap.

## 0.3.1 - 2026-06-15

### Added

- Added a `conda-ship-version` GitHub Action input so release workflows can
  pin the action source by full commit SHA while selecting the conda-ship
  release assets to download. Exact release-tag usage remains supported for
  backwards compatibility.

## 0.3.0 - 2026-06-10

### Added

- Added Windows ARM64 builder release assets and PyPI wheels for
  `aarch64-pc-windows-msvc`. Full Windows ARM64 runtime bootstrap coverage
  remains gated by the conda package ecosystem.
- Generated runtimes now write constructor-compatible
  `conda-meta/history` and `conda-meta/initial-state.explicit.txt` during
  bootstrap. Conda tooling can recognize the managed prefix as an environment,
  and runtimes that include `conda-self` can reset back to the shipped package
  set.
- Runtime delegate environments now set `CONDA_COMPLETION_COMMAND_NAME` to the
  stamped runtime executable name for shell completion integrations.

### Development

- CI linting now runs through `prek`, with test, documentation, package, and
  platform canary jobs split so common checks finish sooner.

### Fixed

- Fixed conda-workspaces `conda.lock` parsing for lockfiles that use the
  `version: 1` schema.

## 0.2.1 - 2026-06-03

### Fixed

- Fixed the GitHub Action path for
  `runtime-version = { from = "project-metadata" }`. The action now detects
  when the Rust `cs` binary needs project metadata resolution, sets up Python
  with the official `actions/setup-python` action, resolves the downstream
  version through `pypa/build`, and retries the build with an explicit
  `--runtime-version`.
- Fixed the Windows runtime template release build warning path so release
  binary builds can deny compiler warnings across all targets.

## 0.2.0 - 2026-06-03

### Added

- Added standards-compliant dynamic Python project version resolution for
  runtime stamping. Projects can set
  `runtime-version = { from = "project-metadata" }` in `[tool.conda-ship]`,
  and the Python `conda ship` adapter resolves the concrete version through the
  PEP 517 `prepare_metadata_for_build_wheel` hook before invoking `cs`.
- Added a quickstart tutorial and a compact README quickstart for the shortest
  local path from a solved conda-workspaces environment to a staged runtime.
- Added terminal demo recordings for quickstart, `cs inspect`,
  `cs build --dry-run`, staged artifact verification, and generated runtime CLI
  behavior. The recordings are embedded in the README and the relevant docs
  pages.

### Changed

- Generated runtime metadata now requires a downstream runtime version. Builds
  use `[tool.conda-ship].runtime-version`, static `[project].version`, an
  explicit `--runtime-version` or GitHub Action input, or the Python adapter's
  project metadata resolution. conda-ship no longer falls back to its own
  package version for generated runtimes.
- Documentation now describes the PyPI-first install flow, runtime version
  requirements, local preflight commands, artifact verification, and generated
  runtime CLI behavior in more detail.

### Fixed

- Fixed builder output handling for closed stdout pipes so commands such as
  `cs inspect | head` or filtered demo commands do not report Rust panics when
  the reader exits early.

## 0.1.0 - 2026-06-01

Initial release of `conda-ship`, a generic builder for producing ready-to-run
conda runtimes from solved conda environments.

### Added

- The `cs` builder CLI.
  - `cs inspect` checks the selected manifest, lockfile, source environment,
    exclusions, platforms, and package set without writing files.
  - `cs build` stages runtime artifacts.
  - `cs build --dry-run` validates planned artifact work before downloading,
    stamping, or writing files.
  - `cs run` builds a runtime and immediately runs it for local smoke tests.
- The generic `cs-template` runtime template used to produce downstream runtime
  binaries.
- Platform PyPI wheels that install `cs`, `cs-template`, and the Python adapter
  together.
- A Python adapter that exposes `conda ship` as a conda-style shortcut for
  `cs`, including structured builder diagnostics so common failures can be
  reported predictably through conda.
- Build input from committed source manifests and lockfiles:
  - `conda.toml` with `conda.lock`
  - `pyproject.toml` with `[tool.conda]` and `conda.lock`
  - `pixi.toml` with `pixi.lock`
  - `pyproject.toml` with `[tool.pixi]` and `pixi.lock`
- `[tool.conda-ship]` build policy for generated runtimes, including the
  runtime name, runtime version, delegate executable, source environment,
  artifact layout, package exclusions, install scheme, install name, install
  method, and documentation URL.
- Three artifact layouts:
  - `online`, for small runtime artifacts that download packages during
    bootstrap
  - `external`, for a runtime plus a separate compressed package bundle
  - `embedded`, for a larger single runtime that carries the compressed package
    bundle inside the binary
- Generated runtime commands for `bootstrap`, `status`, `shell`, and
  `uninstall`, plus pass-through support to the configured delegate executable.
- Generated runtime version output, so downstream binaries such as `cx` can
  report their own distribution version instead of the generic conda-ship
  builder version.
- Runtime install ownership metadata so generated runtimes can protect managed
  prefixes from accidental use or removal by the wrong runtime.
- Install schemes for `~/.conda/INSTALL_NAME` and platform user data
  directories, plus a runtime `--path` override for local testing and advanced
  install paths.
- Staged runtime metadata files:
  - `.runtime.lock`
  - `.packages.txt`
  - `.info.json`
  - `.sha256`
  - optional `.bundle.tar.zst` for `external` builds
- Package exclusion after source-lock resolution, so downstream distributions
  can prune packages from a solved environment before building a runtime.
- Validation that the selected runtime environment contains the packages
  required by generated runtimes: `conda`, `conda-rattler-solver`, and
  `conda-spawn`.
- A composite GitHub Action for downstream release jobs. The action uses
  committed manifest and lockfile input, verifies downloaded conda-ship release
  assets, runs `cs build --dry-run`, and exposes `dist-path` for publishing the
  complete generated artifact directory. Release jobs can override runtime
  metadata such as runtime name, runtime version, delegate, layout, install
  scheme, install name, install method, and documentation URL from workflow
  inputs or matrices.
- Tagged release assets for `cs`, `cs-template`, and `SHA256SUMS`.

### Security And Provenance

- Bundle builds require SHA256 package metadata.
- Downloaded, cached, external, embedded, and offline package archives are
  verified before they are staged or installed.
- Runtime templates refuse to run directly; `cs build` must stamp a template
  before it becomes a downstream runtime.
- Runtime names, runtime versions, delegates, install names, install methods,
  target labels, and documentation URLs are validated before they are stamped
  into runtime binaries or artifact names.
- The GitHub Action verifies artifact attestations for downloaded `cs`,
  `cs-template`, and `SHA256SUMS` assets before running them.
- The `conda ship` adapter only runs the `cs` executable installed in the same
  Python environment, unless `CONDA_SHIP_EXECUTABLE` explicitly selects another
  executable.
- Tagged GitHub releases publish immutable asset sets. If a release is wrong,
  publish a new version instead of replacing files under an existing tag.
- GitHub workflows and the composite action use pinned actions, minimal
  permissions, explicit artifact verification, and no shell `eval` for user
  input.
- Release workflows use unprefixed version tags such as `0.1.0`.
- Release checks include Rust advisory, license, dependency-ban, and source
  policy checks.

### Notes

- This is an alpha 0.1.0 release. The project is ready for early downstream
  distribution work, but configuration details and artifact metadata may still
  evolve before 1.0.
- `conda-ship` is not itself a conda distribution. Downstream projects choose
  package sets, channels, runtime names, delegates, install methods, release
  channels, signing policy, and user documentation.
- The GitHub Action should be used from a release tag. Branch refs do not have
  matching `cs` and `cs-template` release assets. Use tags such as `0.1.0`,
  without a leading `v`.
- Downstream release workflows should sign or attest the full `dist-path`
  output after `cs build`.

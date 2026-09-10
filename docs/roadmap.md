# Roadmap

`cs` is focused on the generic build system for single-binary conda
runtimes.

The builder CLI covers the core local workflow:

- `cs inspect`: preflight the selected manifest, lockfile, source
  environment, exclusions, and package set
- `cs build`: stage an `online`, `external`, or `embedded` runtime
- `cs run`: build and execute a local runtime for smoke testing
- `cs package-update`: wrap one finalized runtime executable in a native update
  package

Every staged build writes the runtime plus artifact metadata: the runtime
lock, a package list, a CycloneDX SBOM, an info JSON file, and SHA256 checksums.
`cs build --dry-run` validates planned artifact work without writing files.

Generated runtime behavior lives in `cs-template`. This includes automatic
bootstrap and executable updates when the runtime has update configuration.
Downstream projects choose their own package sets and distribution defaults.

The repository stays focused on producing runtimes. Distribution
wrappers such as Homebrew formulae, constructor-based installers, Docker images,
or enterprise package manager recipes live outside the core builder.

## Experimental Fleet API

Fleet is an experimental Rust API. The Cargo feature that enables it is
`fleet`. It lets orchestrators manage multiple locked conda prefixes while
reusing conda-ship package installation, metadata, offline bundle code, shared
package cache, prefix mutation locking, and interrupted-install recovery.
Stamped runtime artifacts remain the primary conda-ship output.

The API installs, lists, inspects, and removes prefixes using locks supplied by
the caller. It also returns command and shim plans. Callers provide their own
catalog, solver, launchers, shell setup, and update or repair workflows.

See [fleet concepts](explanation/fleet.md) and the
[API reference](reference/fleet.md).

## Manifest And Plugin Work

conda-ship supports conda-workspaces project input for downstream
distribution builds:

- `conda.toml` is the primary conda-workspaces manifest.
- `conda.lock` is the matching source lockfile.
- `pyproject.toml` with nonempty `[tool.conda.workspace]` uses `conda.lock`.
- `pixi.toml` or `pyproject.toml` with nonempty `[tool.pixi.workspace]` uses
  `pixi.lock`.
- `[tool.conda-ship].source-environment` chooses which solved environment becomes the
  runtime.
- `[tool.conda-ship].runtime-name` names the generated runtime.
- `[tool.conda-ship].delegate-executable` chooses which executable receives
  every argument after automatic bootstrap.
- `[tool.conda-ship].exclude-packages` records post-solve pruning policy.
- Package and channel settings come from
  {external+conda-workspaces:doc}`conda workspace sections <reference/conda-toml-spec>`
  when `conda.toml` is available.
- `conda-ship` provides a `conda ship` adapter while preserving
  `cs` as the primary CLI.

The packaged builder path now uses release-published runtime templates, so
installed `cs build` and `conda ship build` can stamp downstream
runtimes without a conda-ship source checkout.

Current follow-up work is mostly distribution hardening:

- add richer provenance examples for package-manager specific release workflows
- keep builds based on committed manifests and lockfiles, with package and
  channel changes made in the manifest
- keep full Windows ARM64 conda runtime bootstrap coverage behind the regular
  canary until the conda package ecosystem has enough stable `win-arm64`
  runtime coverage

# Manifests And Plugin Entry Points

conda-ship supports conda-workspaces manifests and Pixi manifests.

conda-ship is not an environment manager. It consumes a solved conda environment
and turns it into runtimes.

## Manifest Priority

conda-ship looks for `conda.toml`, then `pixi.toml`, then a supported
`pyproject.toml` in the build root. The embedded `pyproject.toml` form requires
a nonempty `[tool.conda.workspace]` or `[tool.pixi.workspace]` table.

A nonempty `[tool.conda.workspace]` takes precedence when both are present.
Otherwise, a nonempty `[tool.pixi.workspace]` selects Pixi input, even if
`[tool.conda]` contains other settings. Conda-workspaces input uses `conda.lock`,
and Pixi input uses `pixi.lock`. See {doc}`../reference/configuration` for the
supported pairs.

The lockfile remains the source of concrete package records. If the selected
lockfile is missing, create it with the tool that owns the manifest, then run
`cs inspect` or `cs build --dry-run` again.

## Source Environment Selection

The solved environment used for the runtime is selected explicitly by
`[tool.conda-ship].source-environment`:

```toml
[tool.conda-ship]
runtime-name = "demo"
delegate-executable = "conda"
artifact-layout = "online"
source-environment = "ship"
exclude-packages = ["conda-libmamba-solver"]
docs-url = "https://example.com/demo/"
```

If `source-environment` is omitted, conda-ship fails. That keeps release builds
from accidentally packaging a default, development, or test environment.

conda-ship derives a runtime lock that contains only the selected source
environment, renamed to `default` for the generated runtime. The derived lock is
stamped into staged runtimes and copied into the staged artifact directory. It
is build output, not another source project lockfile.

## Conda Workspace Shape

A conda-workspaces project defines packages and channels using the
{external+conda-workspaces:doc}`conda-workspaces schema <reference/conda-toml-spec>`
and configures the runtime build in `[tool.conda-ship]`:

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

[environments]
ship = { features = ["ship"], no-default-feature = true }

[tool.conda-ship]
runtime-name = "demo"
delegate-executable = "conda"
artifact-layout = "online"
source-environment = "ship"
exclude-packages = ["conda-libmamba-solver"]
```

`[tool.conda-ship]` is for conda-ship build behavior: which source environment to
turn into a runtime, which packages to prune after the solve, artifact naming
policy, bundle policy, and runtime documentation links.

Package and channel settings belong in the
{external+conda-workspaces:doc}`conda workspace sections <reference/conda-toml-spec>`
when that manifest is available. conda-ship reads the selected lockfile
environment and stamps the resolved package names and channel URLs into runtime
metadata.

For conda-workspaces projects that keep conda config in `pyproject.toml`, use
`[tool.conda.*]` table names, such as `[tool.conda.workspace]` and
`[tool.conda.feature.ship.dependencies]`. For Pixi projects, use Pixi's
`[tool.pixi.*]` table names, such as `[tool.pixi.workspace]` and
`[tool.pixi.feature.ship.dependencies]`. `[tool.conda-ship]` remains a separate
tool table because it configures conda-ship. A `pyproject.toml` containing only
that table is not a supported source manifest.

## CLI Entry Points

`cs` is the primary builder command. The Python adapter also provides
`conda ship` when installed in a conda environment. It runs the same `cs`
builder and forwards the arguments.

By default, the adapter uses the `cs` executable next to the current Python
interpreter. It does not search `PATH`. Set `CONDA_SHIP_EXECUTABLE` to select
a different executable for tests or custom packaging. An invalid override
causes an error. See {doc}`../reference/conda-plugin` for packaging requirements
and project metadata version support.

## Builder And Runtime Template

The downstream project manifest lives in the downstream repository. The
conda-ship builder and generic runtime template come from the conda-ship
release or package installation.

`cs build` copies the selected template, stamps the copy with the runtime name,
delegate, install scheme, install name, runtime lock, metadata, and optional
embedded bundle. That stamped copy is the runtime. conda-ship then writes the
staged artifacts to the downstream project's output directory. Packaged builds
use the runtime template installed next to `cs`, unless `--template` points at
an explicit template asset. Source checkouts use the same rule and do not
compile a runtime template implicitly.

# Troubleshoot Builds

Use this guide when `cs inspect`, `cs build --dry-run`, `cs build`, or the
GitHub Action fails.

Start with the local preflight:

```bash
cs inspect
cs build --dry-run
```

The GitHub Action runs the same dry run before it writes artifacts.

## Project Root Not Found

Error shape:

```text
could not find project root containing conda.toml, pixi.toml, or supported pyproject.toml
```

Fix:

- run `cs` from the project directory, or
- pass `--root PATH`, or
- set the GitHub Action `root` input.

For `pyproject.toml`, add a nonempty `[tool.conda.workspace]` or
`[tool.pixi.workspace]` table. `[tool.conda-ship]` alone is build policy and does
not define the source workspace. See {doc}`../reference/configuration`.

## Lockfile Not Found

Error shape:

```text
lockfile not found at conda.lock
```

Fix the lockfile with the tool that owns the manifest:

```bash
conda workspace lock
```

or:

```bash
pixi lock
```

Commit the lockfile before using the GitHub Action.

## Source Environment Missing

Error shape:

```text
source environment is required
```

Fix:

```toml
[tool.conda-ship]
source-environment = "ship"
```

conda-ship does not fall back to a default environment because that can
accidentally ship development or test dependencies.

## Runtime Version Is Missing

Error shape:

```text
runtime version is required
```

Fix:

```toml
[tool.conda-ship]
runtime-version = "1.0.0"
```

For Python projects with static metadata, conda-ship can use `[project].version`
from the selected `pyproject.toml`. For projects with `dynamic = ["version"]`,
the Python `conda ship` adapter can resolve PEP 517 project metadata:

```toml
[tool.conda-ship]
runtime-version = { from = "project-metadata" }
```

When using the standalone `cs` binary directly, pass `--runtime-version` with a
concrete value instead.

## Source Environment Not In The Lockfile

Error shape:

```text
source environment "ship" not found
```

Fix:

- add the environment to `conda.toml`, `pixi.toml`, or `pyproject.toml`
- refresh the matching lockfile
- rerun `cs inspect`

## Runtime Template Not Found

Error shape:

```text
runtime template not found
```

Packaged builds look for `cs-template` next to `cs`. Source checkouts do
not compile a template implicitly.

Fix by either:

- installing conda-ship from a package that includes `cs-template`
- setting `CONDA_SHIP_TEMPLATE`
- passing `--template PATH`

Cross-builds require an explicit template for the requested target.

## Bundle Build Missing SHA256 Data

Error shape:

```text
cannot bundle packages without SHA256 hashes
```

External and embedded layouts need package hashes so package archives can be
verified. Refresh the source lockfile with a tool/version that records SHA256
metadata for the selected packages.

## Invalid Runtime, Delegate, Target, Or Install Method

Identifier-like values must start with an ASCII letter or digit and may contain
only ASCII letters, digits, dots, dashes, and underscores.

Examples:

```text
demo
demo-runtime
x86_64-unknown-linux-gnu
homebrew
```

Avoid path-like values such as `demo/runtime` or shell-like values with spaces.

## Runtime Refuses An Existing Prefix

Error shape:

```text
refusing to use unmanaged install path
```

The runtime found an existing conda prefix without its ownership metadata. Use
`CONDA_SHIP_PREFIX` to choose another path, such as
`CONDA_SHIP_PREFIX=/tmp/demo demo info`, or remove the old prefix yourself if
you know it is safe. Runtime-specific `_PREFIX` variables also work for names
other than `conda`.

Generated runtimes refuse unmanaged prefixes to avoid deleting or mutating conda
installations they did not create.

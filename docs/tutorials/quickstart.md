# Quickstart

Use this page when you want the shortest local path from an empty directory to a
staged runtime artifact. It uses conda-workspaces. The full first-runtime
tutorial also covers automatic first-run bootstrap and embedded builds.

## Install The Builder

Create an environment with conda-workspaces, then install conda-ship from PyPI
through conda:

```bash
conda install --name base -c conda-forge conda-pypi
conda create -n cs-demo -c conda-forge python pip conda-workspaces
conda activate cs-demo
conda pypi install conda-ship
```

If you prefer not to install `conda-pypi` into `base`, use
`python -m pip install conda-ship` in the activated environment instead.

Check that the builder is available:

```bash
cs --help
conda workspace --help
```

## Create A Runtime Project

Create a project and add the packages chosen for this conda runtime:

```bash
mkdir demo-runtime
cd demo-runtime
conda workspace init --format conda --name demo-runtime
conda workspace add --feature ship --no-lockfile-update \
  "python>=3.12" \
  "conda>=25.1" \
  conda-rattler-solver \
  "conda-spawn>=0.1.0"
```

Add conda-ship build policy:

```bash
cat >> conda.toml <<'TOML'

[tool.conda-ship]
runtime-name = "demo"
runtime-version = "0.1.0"
delegate-executable = "conda"
artifact-layout = "online"
source-environment = "ship"
exclude-packages = ["conda-libmamba-solver"]
TOML
```

## Lock And Build

Solve the source lockfile, inspect the derived runtime package set, preview the
build, and write the runtime artifact:

```bash
conda workspace lock
cs inspect
cs build --dry-run
cs build
```

The online runtime is staged at `dist/demo` on Unix and `dist/demo.exe` on
Windows. Invoke the conda delegate. The first invocation automatically
bootstraps the managed prefix before running `conda info`:

```bash
./dist/demo info
```

Next, follow the [first runtime tutorial](first-runtime.md) to use a temporary
install path for smoke testing.

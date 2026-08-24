# Build Your First Runtime

This tutorial builds a local conda runtime named `demo` from a
conda-workspaces project.

You will create a small project, lock it, build a runtime binary, and invoke it
with a temporary managed prefix.

## Before You Start

You need:

- `conda-ship`
- {external+conda-workspaces:doc}`conda-workspaces <index>`
- network access for solving and for the first bootstrap

Install the tools in an environment where you want to run the builder:

```bash
conda install --name base -c conda-forge conda-pypi
conda create -n cs-demo -c conda-forge python pip conda-workspaces
conda activate cs-demo
conda pypi install conda-ship
```

If you prefer not to install `conda-pypi` into `base`, use
`python -m pip install conda-ship` in the activated environment instead.

Check that both commands are available:

```bash
cs --help
conda workspace --help
```

## Create A Project

Create an empty project directory:

```bash
mkdir demo-runtime
cd demo-runtime
```

Create a `conda.toml`:

```bash
conda workspace init --format conda --name demo-runtime
```

Add the packages chosen for this conda runtime. The `ship` feature is
the source environment that conda-ship will turn into a runtime lock:

```bash
conda workspace add --feature ship --no-lockfile-update \
  "python>=3.12" \
  "conda>=25.1" \
  conda-rattler-solver \
  "conda-spawn>=0.1.0"
```

Add conda-ship's build policy:

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

## Lock The Project

Solve the source lockfile with conda-workspaces:

```bash
conda workspace lock
```

This writes `conda.lock`. conda-ship consumes the committed lockfile; it does
not solve directly from loose package names during normal builds.

## Inspect The Package Set

Run a preflight check before building. This derives the runtime package set,
applies exclusions, and prints the selected packages without writing files:

```bash
cs inspect
```

The output lists the selected manifest and lockfile, each locked platform, and
the package set for your current platform.

## Build The Runtime

Build an online runtime named `demo`:

```bash
cs build
```

The generated runtime is written to `dist/demo` on Unix and `dist/demo.exe` on
Windows.

An online runtime contains the lockfile and runtime metadata. It downloads conda
package archives when it bootstraps.

## Smoke-Test The Runtime

For this tutorial, invoke the generated runtime with a temporary local prefix:

```bash
mkdir -p .tmp
DEMO_PREFIX="$PWD/.tmp/demo" ./dist/demo info
```

Because the prefix is absent, the runtime automatically bootstraps the selected
package set and then executes `conda info`. The command output is the delegate's
normal status output. A real downstream distribution should document how its
users install and update the runtime it publishes.

The runtime also writes conda prefix metadata during bootstrap:

```bash
ls "$PWD/.tmp/demo/conda-meta/history"
ls "$PWD/.tmp/demo/conda-meta/initial-state.explicit.txt"
```

`history` lets conda recognize the install path as an environment.
`initial-state.explicit.txt` records the exact package URLs from the stamped
runtime lock. If your runtime package set includes `conda-self`, that file is
the installer snapshot used by `conda self reset --snapshot installer-updated`
and `conda self reset --snapshot installer-exact`.

```{note}
The `DEMO_PREFIX` override keeps this tutorial install inside the project
directory. The variable name comes from `runtime-name`. Published runtimes
should document their normal install location and reserve the `_PREFIX`
variable for packaging and advanced overrides.
```

Clean up the temporary install:

```bash
rm -rf -- "$PWD/.tmp/demo"
```

The generated runtime does not reserve an uninstall command. For a published
distribution, removal belongs to its installer, package manager, or a
self-management plugin such as conda-self.

## Optional: Build An Embedded Runtime

The embedded layout puts compressed package archives inside the generated
binary. This makes the build slower and the binary larger, but bootstrap no
longer needs to download package archives.

```bash
cs build --artifact-layout embedded
```

Embedded runtimes use the configured runtime name by default, so this stages
`dist/demo` on Unix and `dist/demo.exe` on Windows.

Smoke-test it:

```bash
DEMO_PREFIX="$PWD/.tmp/demo-embedded" ./dist/demo info
rm -rf -- "$PWD/.tmp/demo-embedded"
```

## What You Learned

You created a small workspace project, solved it, built an online runtime, and
used that binary to install its conda prefix automatically before running the
configured delegate.

For a real downstream distribution, choose a runtime name owned by that
distribution, keep its package choices in the source manifest, and publish the
staged files from `dist/`.

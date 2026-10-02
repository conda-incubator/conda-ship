# Build Offline Artifacts

Offline artifacts let the generated runtime install from package archives that
were downloaded during the build.

Use them when a downstream distribution needs air-gapped installs, native
installer integration, or a single self-contained runtime.

## Choose A Layout

::::{tab-set}

:::{tab-item} External
Use `external` when you want the runtime and compressed bundle as separate
files. This is useful when an installer or package manager already knows how to
place supporting files next to the binary.

```bash
cs build --artifact-layout external
```
:::

:::{tab-item} Embedded
Use `embedded` when you want one larger runtime that can bootstrap without a
separate bundle file.

```bash
cs build --artifact-layout embedded
```
:::

::::

## Bootstrap From An External Bundle

For an `external` build, distribute these files together:

- `demo`
- `demo.bundle.tar.zst`
- `demo.runtime.lock`
- `demo.info.json`
- `demo.packages.txt`
- `demo.cdx.json`
- `demo.sha256`

Point the runtime at an extracted bundle directory on its first invocation:

```bash
mkdir -p /opt/demo-bundle
tar -I zstd -xf demo.bundle.tar.zst -C /opt/demo-bundle
DEMO_PREFIX=/opt/demo \
DEMO_BUNDLE=/opt/demo-bundle \
DEMO_OFFLINE=1 \
demo info
```

Pass the directory that contains the package archive files themselves. A bundle
contains top-level `.conda` and `.tar.bz2` files named in the runtime lock.

The bundle, offline, and prefix controls are derived from the runtime name. For
a runtime named `demo`, they are `DEMO_BUNDLE`, `DEMO_OFFLINE`, and
`DEMO_PREFIX`.

`CONDA_SHIP_PREFIX` is also available and takes precedence over `DEMO_PREFIX`.
For a runtime named `conda`, use `CONDA_SHIP_PREFIX`. The runtime deliberately
does not interpret an activated `CONDA_PREFIX` as its installation path.

```{note}
Pass the directory containing the package archives directly. Do not add
`linux-64/`, `noarch/`, or `repodata.json`.
```

## Bootstrap From An Embedded Bundle

An embedded runtime carries the bundle inside the binary:

```bash
DEMO_PREFIX=/opt/demo demo info
```

The runtime extracts the compressed package archives to a temporary directory
during bootstrap and installs from that extracted bundle without network
access.

The embedded tar archive may only contain top-level package archive files.
Nested paths, directory entries, symbolic links, hard links, and non-package
files are rejected before install.

```{important}
Keep embedded bundles as package archives only. conda-ship rejects paths and
links before extraction so a bundled runtime cannot write outside the temporary
bundle directory during bootstrap.
```

An explicit `DEMO_BUNDLE` value still takes priority over the embedded bundle.
The supplied archives must still match the packages and hashes in the runtime
lock. To change the package set, update the source lockfile and rebuild.

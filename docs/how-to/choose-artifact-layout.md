# Choose An Artifact Layout

Use this guide when you need to choose between `online`, `external`, and
`embedded` builds.

The layout changes how package archives travel with the runtime. It does not
change the package set. All layouts use the same runtime lock derived from the
selected source environment.

::::{grid} 1 1 3 3
:gutter: 3

:::{grid-item-card} `online`

Small runtime artifact. Downloads package archives during bootstrap.
:::

:::{grid-item-card} `external`

Runtime binary plus separate package bundle. Useful for installers and managed
deployment systems.
:::

:::{grid-item-card} `embedded`

One larger runtime file. Carries package archives for offline bootstrap.
:::

::::

## Use `online` For Small Release Assets

Choose `online` when users can download conda package archives during
bootstrap:

```bash
cs build --artifact-layout online
```

An online runtime contains:

- runtime metadata
- the stamped runtime lock
- no package archive bundle

Bootstrap downloads packages from the channels recorded in the lock. This keeps
the runtime artifact small and is the best default for GitHub Releases,
Homebrew, and package-manager wrappers that expect network access.

## Use `external` For Split Binary And Bundle Delivery

Choose `external` when you want the runtime and package archives as separate
files:

```bash
cs build --artifact-layout external
```

This stages:

- `RUNTIME`
- `RUNTIME.bundle.tar.zst`
- metadata files

Use this layout when an installer, archive, or enterprise deployment system can
place the runtime and bundle side by side. Set the bundle and offline variables
derived from the runtime name on first invocation:

```bash
DEMO_BUNDLE=./bundle-dir DEMO_OFFLINE=1 demo info
```

For another runtime name, derive the variable names by uppercasing it and
replacing non-alphanumeric characters with underscores.

The external bundle is not a conda channel mirror. It is a flat set of `.conda`
and `.tar.bz2` archives named in the runtime lock.

## Use `embedded` For One Larger Offline Runtime

Choose `embedded` when a single runtime binary must carry the package archives:

```bash
cs build --artifact-layout embedded
```

Embedded runtimes use the configured runtime name unless you set
`artifact-name`:

```text
demo          -> online, external, or embedded runtime
demo-cli      -> staged runtime when configured explicitly
```

The first invocation detects the embedded bundle automatically, installs the
managed prefix, and then runs the delegate:

```bash
demo info
```

This is useful when the runtime must install without network access and you do
not want a separate bundle file. The tradeoff is a larger binary, slower builds,
and additional temporary storage during bootstrap. Budget for a verified copy
of the compressed embedded bundle in addition to the extracted packages.

## Decision Table

| Need | Layout |
| --- | --- |
| Smallest runtime artifact | `online` |
| Network access during bootstrap is acceptable | `online` |
| Runtime and packages should be distributed separately | `external` |
| Installer can unpack a bundle next to the runtime | `external` |
| One file should bootstrap offline | `embedded` |
| Release channel has strict single-binary ergonomics | `embedded` |

## Keep The Layout Out Of The Solve

Do not create separate source environments only to change layout. Keep package
and channel intent in the source manifest, commit the lockfile, and override
layout at build time when needed:

```bash
cs build --artifact-layout online
cs build --artifact-layout embedded
```

In GitHub Actions, matrix the layout:

```yaml
strategy:
  matrix:
    layout: [online, embedded]

steps:
  - uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
    with:
      conda-ship-version: "X.Y.Z"
      artifact-layout: ${{ matrix.layout }}
```

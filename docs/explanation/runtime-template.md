# How Generated Runtimes Work

When you run `cs build`, conda-ship copies the `cs-template` runtime template
and writes your configuration, lockfile, and optional bundle into that copy.
This step is called stamping. Set the runtime name with
`[tool.conda-ship].runtime-name` or `--runtime-name`. Use
`[tool.conda-ship].artifact-name` or `--artifact-name` when the staged
executable needs a different filename.

## What `cs build` Writes

During a runtime build, conda-ship writes these details into the copied
binary:

- runtime name, artifact name, and delegate executable
- install scheme and install name
- installer, when configured
- runtime lock
- optional compressed package bundle
- documentation URL
- metadata filename
- bundle and offline environment variable names
- optional condarc contents and base-freezing setting
- optional executable update source and build number

## Where The Template Comes From

The conda-ship package installs `cs-template` alongside `cs`. The GitHub Action
downloads it from conda-ship's release assets. Release asset names include the
target platform, for example:

```text
cs-template-x86_64-unknown-linux-gnu
cs-template-aarch64-apple-darwin
cs-template-x86_64-pc-windows-msvc.exe
```

Template selection uses this order:

1. `--template PATH`
2. `CONDA_SHIP_TEMPLATE`, when nonempty
3. `cs-template` next to `cs`, when `--target` is omitted

When passing `--target`, also select a matching template with `--template` or
`CONDA_SHIP_TEMPLATE`. The builder does not search `PATH` or download a template.

Running the template directly fails with a message that points back to
`cs build`. The stamped copy contains the runtime name, lockfile, package
metadata, and install settings needed to run.

Source checkouts use the same selection order. `cs build` does not compile a
template automatically.

## Keep Native Builders And Templates Paired

macOS and Windows builders require the signed-layout reader declaration emitted
by the matching `cs-template` release. conda-ship 0.9.0 rejects native templates
from 0.8.0 and earlier. Upgrade `cs` and `cs-template` together instead of
mixing release assets. Linux templates do not use this native declaration.

For the GitHub Action, update the pinned full action commit SHA and the
`conda-ship-version` input together. For custom packaging, download the builder
and template from the same release and verify both against that release's
attestations or `SHA256SUMS`.

## What Users See

The finished runtime does not expose conda-ship commands. On first invocation it
installs the selected package set into its managed prefix, then executes the
configured delegate with the original arguments. Later invocations execute the
same delegate directly through the existing prefix.

When update configuration is stamped, the native runtime can check, stage,
apply, and recover executable updates. It can also reconcile a replacement
performed by an external package manager. The installed ownership and
installation kind are recorded in the managed prefix, so the same stamped
bytes can be directly or externally managed. This behavior is part of the
stamped native template. The conda-ship Python package is not installed in the
managed prefix and is not needed at runtime.

This means `--help`, `--version`, `status`, `shell`, `uninstall`, and every
other argument belong to the delegate. For a conda delegate, `conda info`
reports conda and prefix status. If the distribution includes a conda-spawn
version that provides `conda shell`, users can run `RUNTIME shell`.

Downstream distributions can stamp native condarc contents and protect the base
prefix with a CEP 22 frozen marker. Without those opt-ins, conda-ship leaves
conda configuration and package-created frozen markers untouched.

## What Each Project Chooses

Some runtime behavior is visible to users:

- automatic bootstrap before the first delegate invocation
- unchanged delegate arguments, process streams, signals, and exit status
- optional commands provided by packages such as conda-spawn and conda-self
- bundle and offline variables derived from the runtime name
- `CONDA_SHIP_PREFIX` and a runtime-specific prefix variable for names other
  than `conda`
- optional executable update behavior selected by stamped configuration

The package set, runtime name, delegate, documentation URL, and release channel belong to
the project using conda-ship.

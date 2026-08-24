# How Generated Runtimes Work

When you run `cs build`, conda-ship does not invent a new program from scratch.
It starts with a small generic runtime template, copies it to the resolved
runtime name, and writes your build data into that copy. The runtime name can
come from `[tool.conda-ship].runtime-name` or from `--runtime-name`. Builds can use
`[tool.conda-ship].artifact-name` or `--artifact-name` when the staged artifact
needs a distinct command name.

Users rarely need to think about the template. They run the
finished runtime, such as `demo`, `cx`, or a downstream-specific embedded name
like `cxz`.

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

That is what turns the same generic bootstrap code into a specific runtime
with its own runtime name, delegate, package set, and install location.

## Where The Template Comes From

For packaged builds, the template is downloaded from conda-ship's GitHub
Release assets. The asset name includes the platform it runs on, for example:

```text
cs-template-x86_64-unknown-linux-gnu
cs-template-aarch64-apple-darwin
cs-template-x86_64-pc-windows-msvc.exe
```

You usually only see those names when wiring a packaging job. The GitHub Action
downloads the matching template automatically. A packaged `cs` CLI looks for
an installed `cs-template` next to the `cs` executable; it does not
search arbitrary `PATH` entries for a template. `--template PATH` is an
override for custom packaging or cross-builds.

The template is not a runtime. Running it directly fails with a message that
points back to `cs build`; only the stamped copy has a runtime name,
lockfile, package metadata, and install policy.

When running from a source checkout, `cs build` still expects either an
installed template next to `cs`, a `CONDA_SHIP_TEMPLATE` environment variable,
or an explicit `--template PATH`.

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
reports conda and prefix status. A distribution that includes conda-spawn with
the alias from
[conda-spawn PR #59](https://github.com/conda/conda-spawn/pull/59) can expose
`RUNTIME shell` as a command provided by conda-spawn.

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

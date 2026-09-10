# Overview

conda-ship turns a solved conda environment into a ready-to-run runtime.
It contains generic build and bootstrap code. It is not a distribution, an
environment manager, or an installer generator.

The workflow is:

- A downstream project uses conda-workspaces or Pixi to solve and commit its
  package records.
- The builder selects one solved environment, derives a runtime lock, stamps
  the generic runtime template, and stages online, external, or embedded
  artifacts.
- The generated runtime automatically bootstraps a managed prefix when absent
  and passes every argument to its configured delegate unchanged.
- The downstream project owns package sets, runtime names, user-facing policy,
  installers, documentation, and release channels.

## The Runtime Flow

```{mermaid}
flowchart TB
    subgraph downstream["Downstream project"]
        direction LR
        intent["Packages and channels"] --> solver["conda-workspaces or Pixi"] --> source_lock["Source lock"]
        choices["Runtime and release choices"]
    end

    subgraph ship["conda-ship"]
        direction LR
        builder["Builder"] --> runtime_lock["Runtime lock"]
        builder --> bundle["Optional package bundle"]
        builder --> artifacts["Staged artifacts"]
        runtime_lock --> artifacts
        bundle --> artifacts
    end

    subgraph machine["User machine"]
        direction LR
        runtime["Generated runtime"] --> prefix["Managed prefix"] --> delegate["Delegate"]
    end

    source_lock --> builder
    choices -. "configuration" .-> builder
    artifacts --> runtime
```

The following sections describe the builder, generated runtime, lockfile, and
package bundle.

## Builder

The builder turns a project's selected locked environment into release
artifacts. It reads one of these standard manifest and lockfile pairs:

| Project type | Manifest | Lockfile |
| --- | --- | --- |
| Conda Workspaces | `conda.toml` or configured `pyproject.toml` | `conda.lock` |
| Pixi | `pixi.toml` or configured `pyproject.toml` | `pixi.lock` |

It applies the project's [build configuration](../reference/configuration.md),
then derives a runtime lock, bundle files, runtimes, and artifact metadata.

The selected source lockfile is the source of the concrete conda package
records. conda-ship is not a replacement for
{external+conda-workspaces:doc}`conda-workspaces <index>`, Pixi, or another
workspace solver. It consumes a solved environment and turns it into runtime
artifacts.

## Runtime

A runtime is the generated executable that users run after a build.

Its runtime name is its base identity, not a conda environment name. By
default, the build uses the same name for the staged executable and artifact.
Projects can choose a distinct artifact name when a release needs a different
filename. See the [configuration reference](../reference/configuration.md).

Runtime
: The executable conda-ship produces.

Managed prefix
: The directory where the runtime installs its conda environment.

Delegate
: The executable inside the managed prefix that receives every runtime
  argument, such as `conda` or `python`.

Artifact
: A release file staged by the build.

## Runtime Template

The generic runtime template, `cs-template`, contains the code for installing
the environment and running the delegate. During a build, `cs` copies it and
writes the runtime configuration, lockfile, and optional package bundle into
that copy. The documentation calls this step **stamping**. The stamped copy is
the runtime that users run.

Released builds and packaged local builds use prebuilt template assets.

## Runtime Lock

The runtime lock comes from the configured source environment after applying
the project's package exclusions. conda-ship stamps the derived lock into every
runtime artifact and stages a copy next to the output binary. It is build
output, not a second checked-in project lockfile.

The inspection command derives the same runtime lock without writing files,
which makes it the local preflight step. Build and run operations derive the
lock as part of their normal work.

The generated runtime can install from:

- the stamped lockfile and network package downloads
- the stamped lockfile and an external package bundle
- the stamped lockfile and an embedded package bundle

## Bundles

Bundles contain downloaded conda package archives. They are not conda channel
mirrors. The runtime lock already records the channels and package records.

An external artifact stages its bundle alongside the runtime. An embedded
artifact carries the compressed bundle inside the runtime. See
[artifact layouts](../how-to/choose-artifact-layout.md) for the exact files and
configuration choices.

An embedded runtime automatically uses its bundled archives during first-run
bootstrap. Its bundle can be overridden with the bundle environment variable
derived from the runtime name when needed.

A bundle contains top-level `.conda` and `.tar.bz2` files. The runtime verifies
them against the lockfile at install time.
Embedded bundles reject nested paths and links before extraction.

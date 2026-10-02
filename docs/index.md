# conda-ship

Build ready-to-run conda runtimes.

`conda-ship` builds an executable from a locked conda environment. On first
use, that executable installs the environment into a directory called the
**managed prefix**, then runs the configured command, called the **delegate**.
Later invocations reuse the installed environment.

Use the `cs` CLI to build runtimes. Choose the runtime name, delegate, package
set, channels, documentation URL, and release channel in your project.

[conda-express](https://jezdez.github.io/conda-express/) is one downstream
distribution maintained by Jannis Leidel: it uses conda-ship to build the `cx`
and `cxz` runtimes. conda-express chooses their packages, settings, and release
channels.

## Start Here

The [quickstart](tutorials/quickstart.md) installs the builder, creates and
locks a small conda workspace, and builds a `demo` runtime. For an explanation
of each step, follow the [first runtime tutorial](tutorials/first-runtime.md).

## Choose A Path

- New to conda-ship: follow the
  [first runtime tutorial](tutorials/first-runtime.md).
- Building a downstream runtime: use
  [customize a runtime](how-to/customize-runtime.md), then check the exact
  fields in the [configuration reference](reference/configuration.md).
- Shipping from CI: start with
  [build in GitHub Actions](how-to/build-in-github-actions.md).
- Choosing names or release files: read
  [runtime and artifact names](reference/names.md) and
  [artifacts](reference/artifacts.md).
- Unsure what belongs here versus downstream: read
  [project scope](explanation/project-boundaries.md).
- Building an orchestrator for multiple locked runtimes: read
  [Fleet concepts](explanation/fleet.md) and the
  [Fleet API reference](reference/fleet.md).

## Scope

conda-ship builds runtimes from solved conda environments. It does not choose
package sets, reserve downstream runtime names, publish a first-party runtime,
or generate operating-system installers.

```{toctree}
:hidden:
:caption: Tutorials
:maxdepth: 1

tutorials/quickstart
tutorials/first-runtime
tutorials/github-action-runtime
tutorials/custom-delegate-runtime
```

```{toctree}
:hidden:
:caption: How-To Guides
:maxdepth: 1

how-to/build-locally
how-to/choose-artifact-layout
how-to/customize-runtime
how-to/build-in-github-actions
how-to/build-offline-artifacts
how-to/package-a-runtime
how-to/verify-release-artifacts
how-to/troubleshoot-builds
```

```{toctree}
:hidden:
:caption: Reference
:maxdepth: 1

reference/cli
reference/names
reference/conda-plugin
reference/runtime-cli
reference/fleet
reference/github-action
reference/configuration
reference/artifacts
reference/environment-variables
reference/runtime-data-format
reference/errors
```

```{toctree}
:hidden:
:caption: Explanation
:maxdepth: 1

explanation/overview
explanation/source-locks-and-runtime-locks
explanation/runtime-template
explanation/fleet
explanation/install-locations-and-ownership
explanation/trust-and-provenance
explanation/project-boundaries
explanation/manifests-and-conda-plugin
```

```{toctree}
:hidden:
:caption: Project
:maxdepth: 1

roadmap
changelog
```

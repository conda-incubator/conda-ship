# Project Scope

conda-ship builds ready-to-run conda runtimes. It is not itself a conda
distribution.

conda-ship provides the builder and runtime code. Downstream projects choose
the packages, settings, and release process for their distributions.

## Ownership At A Glance

::::{grid} 1 1 2 2
:gutter: 3

:::{grid-item-card} conda-ship

Generic builder, runtime behavior, artifact layouts, bundle handling, and
metadata files.
:::

:::{grid-item-card} Downstream Distributions

Package sets, runtime names, delegate choices, release channels, installer wrappers, and user
documentation.
:::

::::

## What conda-ship Owns

conda-ship provides the reusable build and runtime code:

- deriving a runtime lock from a conda or Pixi source lockfile
- pruning excluded packages and exclusive dependencies after the solve
- downloading package archives into compressed bundles
- copying the generic runtime template and stamping distribution data
- staging `online`, `external`, and `embedded` artifact layouts
- writing artifact metadata: `.runtime.lock`, `.packages.txt`, `.cdx.json`,
  `.info.json`, and `.sha256`
- stamping optional executable update configuration
- creating native executable update packages from finalized runtime bytes
- exposing the composite GitHub Action and local builder CLI

The generated runtime behavior also lives here: automatic first-run bootstrap,
execution of the configured delegate, offline bundle handling, and embedded
bundle handling. For update-enabled artifacts it also includes update package
verification, staged replacement, recovery, external replacement
reconciliation, and the version-one helper invoked by a coordinator as a child
process.
conda-ship does not define the delegate's commands or plugins.

The experimental [Fleet API](fleet.md) also lives here because it reuses the
same prefix mutation lock, recovery of interrupted installs, full-lock
reinstall, metadata, and offline bundle code for multiple locked prefixes.
Fleet is an optional Rust API. It does not replace stamped runtime artifacts or
choose catalogs, delegates, condarc contents, frozen-base policy, installer
provenance, user-facing command names, global PATH policy, or shim filesystem
writes.

## What Downstream Distributions Own

Downstream projects decide what their users get:

- runtime names and delegate executables
- package sets
- channels
- package exclusions
- installed condarc contents and frozen-base policy
- default release channels
- documentation URLs
- Homebrew formulae
- Python or conda integration packages
- Docker images
- GitHub Release policy
- CRA product-scope review and SBOM publication, access, and retention policy
- constructor-based installers or enterprise package manager recipes
- orchestrator catalogs, onboarding, login, policy, and user-facing shim names
- publication and indexing of executable update packages
- detecting supported package-manager receipts and recording installed
  executable ownership
- user prompting and coordination with the inner package transaction
- credentials supplied through an explicit `RATTLER_AUTH_FILE`
- commands used to replace an externally owned executable
- ownership and update guidance for launchers created by Fleet callers

conda-ship produces the runtimes and metadata those channels can distribute. It
does not decide whether every runtime includes the same conda plugins or uses
the same name. Fleet launchers remain caller-owned and do not use the stamped
runtime executable update path automatically.

## conda-express

{external+conda-express:doc}`conda-express <index>` is the downstream
distribution that publishes `cx` and `cxz`.

It chooses the conda package set, the `cx`/`cxz` runtime names,
Homebrew and shell-script installation, Docker images, Python or conda
distribution wrappers, and release policy for those artifacts.

When conda-express needs runtimes, its workflows call conda-ship from the
conda-express project root. The conda-express manifest names the `cx` runtime.
Its release can explicitly set `artifact-name = "cxz"` when it wants that staged
artifact name. The package set remains conda-express project input, and
conda-ship does not hard-code those choices. Its own scope page is
{external+conda-express:doc}`Project scope <scope>`.

## Relationship To Other Tools

conda-ship complements other conda ecosystem tools:

| Tool | Role |
| --- | --- |
| {external+conda-workspaces:doc}`conda-workspaces <index>` | Defines workspace manifests and lockfiles that conda-ship can consume |
| Pixi | Solves and records compatible source environments that conda-ship can consume |
| rattler-build | Builds conda packages |
| constructor | Builds OS installers |
| conda-ship | Builds runtimes that can be distributed directly or wrapped by other channels |
| {external+conda-express:doc}`conda-express <index>` | A conda-ship-based downstream distribution for `cx` and `cxz` |

conda-ship does not produce installer-generator output such as `.sh`, `.pkg`, or
`.msi`. Those formats can wrap conda-ship-built runtimes when a downstream
distribution needs them.

The {external+conda-express:doc}`conda-express docs <index>` are useful as a
concrete example of a downstream distribution. They describe `cx` and `cxz` as
products. conda-ship docs describe how to build runtimes like them.

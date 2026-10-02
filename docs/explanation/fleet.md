# Fleet Concepts

Fleet is an experimental Rust API inside the `conda-ship` crate. It is enabled
only with the non-default Cargo feature `fleet`.

Stamped runtime artifacts remain the primary conda-ship output. On first
invocation, a stamped artifact bootstraps one prefix and then passes the
original arguments to its configured delegate. The delegate can be `conda` or
any other installed entry point. The artifact does not reserve bootstrap,
status, repair, update, or uninstall commands.

Fleet is an optional Rust API for callers that need several locked prefixes. It
does not create a new conda-ship distribution or CLI. Each runtime is installed
under:

```text
install_root/<id>
```

The runtime id also selects its conda-ship metadata file:

```text
install_root/<id>/.<id>.json
```

## Install And Recovery

Fleet and stamped artifacts use the same adjacent process lock, installing
marker, delegate validation, reinstall code, and final metadata rename that
marks a prefix ready. After an interrupted install, Fleet reinstalls every
package in the selected lock. This reruns link scripts while preserving named
environments and unrelated files under the prefix.

`Fleet::install` refuses unknown non-empty prefixes. `force = true` means an
in-place full-lock reinstall of a prefix already owned by the same Fleet runtime
id. It does not recursively replace the prefix. Recursive deletion happens only
through an explicit `Fleet::remove` call.

## Prefix Metadata

Fleet has no separate registry database. `Fleet::list()` scans direct children
of the install root. `Fleet::get(id)` validates the runtime's regular metadata
file, ready state, identity, and recorded delegate.

Fleet metadata records the explicit delegate and the SHA256 digest of the lock
content. It also records lockfile channels as provenance. Those channels are
not turned into an implicit `.condarc`.

`RuntimeSpec` is not a user-facing catalog format. Callers construct it from
their catalog or conda-ship stamped runtime data.

## Runtime Configuration

The caller decides whether each runtime receives:

- exact `.condarc` text supplied by the caller
- a CEP 22 frozen-base marker
- Constructor-compatible `.installer.info` provenance

All three are disabled by default. During a Fleet install or forced reinstall,
Fleet removes existing regular files at these paths and writes only the
configured outputs. Symbolic links and other nonregular entries are refused
before package installation begins.

`.installer.info` is distribution provenance. It is not launcher ownership
evidence.

## Commands And Launchers

Fleet returns executable paths and prefix-local PATH entries. It does not
set `CONDA_PREFIX`, `CONDA_ROOT_PREFIX`, `CONDA_DEFAULT_ENV`,
`CONDA_SHLVL`, or completion variables. A delegate receives the same minimal
PATH treatment as a stamped runtime.

Fleet returns shim plans but does not write files. Callers handle shim contents,
overwrite policy, PATH setup, and removal. A launcher created by a Fleet caller
is externally managed. conda-ship does not own its update or removal flow.

## Responsibilities

Fleet handles the conda-ship parts of prefix installation:

- installing a resolved lock into a known prefix
- shared package-cache, bundle, and offline behavior
- interrupted-install recovery and mutation locking
- constructor history and initial-state files
- ready prefix metadata and lock provenance
- explicit condarc, frozen-base, and installer-provenance outputs
- command and shim plans

The caller still handles:

- catalog lookup and runtime selection
- solving and lock production
- user-facing command and shim names
- global PATH and shell setup
- login, onboarding, enterprise policy, telemetry, and prompts
- update, uninstall, and migration workflows
- launcher installation and external package-manager guidance

## Experimental Status

The API is experimental. Pin a repository revision and enable the feature
explicitly:

```toml
[dependencies]
conda-ship = { git = "https://github.com/conda-incubator/conda-ship", rev = "<pinned-commit>", features = ["fleet"] }
```

To use native TLS:

```toml
[dependencies]
conda-ship = { git = "https://github.com/conda-incubator/conda-ship", rev = "<pinned-commit>", default-features = false, features = ["fleet", "native-tls"] }
```

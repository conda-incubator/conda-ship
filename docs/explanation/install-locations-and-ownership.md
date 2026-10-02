# Install Locations And Ownership

Generated runtimes install into managed prefixes. They also record ownership
metadata so later operations can tell whether a prefix belongs to that runtime.

## Install Schemes

The install scheme is stamped at build time.

`conda-home`
: Installs below `~/.conda/INSTALL_NAME`.

`user-data`
: Installs below the platform user data directory:

  - Linux: `${XDG_DATA_HOME:-~/.local/share}/conda/INSTALL_NAME`
  - macOS: `~/Library/Application Support/conda/INSTALL_NAME`
  - Windows: `%LOCALAPPDATA%\\conda\\INSTALL_NAME`

The default is `conda-home`.

## Install Name

The install name is the final directory name inside the scheme.

If omitted, conda-ship uses the runtime name. A downstream distribution can use
a short executable name and a clearer install name:

```toml
[tool.conda-ship]
runtime-name = "cx"
install-name = "express"
```

With the `conda-home` scheme, that runtime installs below `~/.conda/express`.

## Content-addressed Install Names

Use the derived `install-name` form to give each runtime version and rendered
runtime lock its own managed prefix:

```toml
[tool.conda-ship]
runtime-name = "cx"
runtime-version = "1.2.3"
install-name = { from = "runtime-lock", base = "express" }
```

The builder stamps a flat install name in the form
`BASE-RUNTIME_VERSION-LOCK_HASH`. `BASE` is the configured `base` or the runtime
name when `base` is omitted. `LOCK_HASH` is the first 16 lowercase hexadecimal
characters of the SHA-256 of the exact rendered `.runtime.lock` bytes. For
example, the `conda-home` scheme might resolve to
`~/.conda/express-1.2.3-d8c9f2728aa278eb`.

Changing the runtime version or lock selects a new prefix. conda-ship does not
remove older prefixes automatically. The abbreviated digest chooses the
install directory. The runtime still verifies package hashes against the
runtime lock and checks prefix ownership before reusing the directory.

The derived name identifies the inputs used for the initial installation. It
does not prevent later package changes inside the prefix. Set
`freeze-base = true` when a CEP 22-aware delegate should reject ordinary base
environment mutations.

Content-addressed runtime versions must contain only ASCII letters, digits,
dots, dashes, and underscores so the complete install name remains one safe
path component. The resolved name is limited to 128 characters. When active,
the derived form cannot be combined with `[tool.conda-ship.update]` because
runtime update candidates must preserve the stamped install name.

An explicit `cs build --install-name` or GitHub Action `install-name` input
remains a final-name override and disables content-addressed name generation
for that build.
`CONDA_SHIP_PREFIX` and the runtime-specific prefix environment variable still
override the resolved path when the stamped runtime executes.

## Runtime Prefix Override

`CONDA_SHIP_PREFIX` overrides the resolved install path for every generated
runtime:

```bash
CONDA_SHIP_PREFIX=/tmp/demo demo info
```

Runtime names other than `conda` also accept a prefix environment variable
derived from the runtime name:

```bash
DEMO_PREFIX=/tmp/demo demo info
```

For a runtime named `demo`, the variable is `DEMO_PREFIX`. Non-alphanumeric
characters in the runtime name become underscores and letters are uppercased.
`CONDA_SHIP_PREFIX` takes precedence when both variables are set. A runtime
named `conda` ignores the derived `CONDA_PREFIX` because it may describe an
activated environment. Use `CONDA_SHIP_PREFIX` for that runtime.

For a local `cs run` smoke test, use the builder-side option instead:

```bash
cs run --install-path /tmp/demo -- info
```

## Ownership Metadata

After automatic bootstrap, the runtime writes a metadata file inside the
managed prefix.
It records:

- schema version
- bootstrap state
- display name
- install name
- metadata filename
- runtime version
- channels
- package names

For an update-enabled runtime, the same file also records:

- executable path and artifact name
- direct or external executable ownership
- installation kind, when an installer or delivery detector recorded one
- source channel and package name
- current build number and executable SHA256
- optional external update instruction
- any pending replacement phase, version, build number, and executable SHA256

Later runtime invocations check that metadata before reusing a prefix.
The metadata file marks bootstrap complete. Metadata written by older
conda-ship runtimes is accepted when its ownership identity and delegate still
validate.

While bootstrap is running, the runtime holds a lock in the prefix's parent
directory and writes a separate internal `installing` marker inside the prefix.
The marker identifies the runtime that started bootstrap. A later invocation
waits for a live bootstrap to release the lock, then checks the prefix again. If
the previous process stopped and the marker matches this runtime, recovery
reinstalls every locked package and reruns post-link scripts without deleting
the prefix.

This ownership file is conda-ship-specific. The runtime also writes standard
conda prefix metadata:

- `conda-meta/history`
- `conda-meta/initial-state.explicit.txt`

Those files serve a different purpose. `history` lets conda recognize the
managed prefix as a conda environment. `initial-state.explicit.txt` records the
exact packages that were installed from the runtime lock at bootstrap time.
Tools that understand constructor-style installer snapshots, including
`conda-self`, can use that explicit file to reset the managed base prefix back
to the package set originally shipped by the runtime.

## Executable Update Ownership

Managed-prefix ownership and executable update ownership are separate. The
prefix metadata file stores both kinds of ownership. Its adjacent
update lock coordinates processes but does not contain update state.

`direct`
: A transaction coordinator may hold the runtime update lock, stage a verified
  executable, complete its inner package transaction, and ask the runtime to
  replace itself. An uncoordinated change to a directly owned executable is
  rejected.

`external`
: The channel package is a release signal. A downstream coordinator reports an
  instruction recorded for that installation and leaves replacement to the
  package manager or installer. On the next invocation, a newly stamped
  executable at the recorded stable path is verified and reconciled with the
  prefix record.

The executable stamp records update capability and source identity. Installed
ownership is a property of the copy on disk. A standalone installer and an
external package manager can therefore install identical runtime bytes while
recording different ownership in the existing prefix metadata file.

Metadata without an installation kind is an unclassified compatibility state.
It preserves existing direct-update behavior, but it may reconcile a valid
newer executable with the same identity and update source before a delivery
detector records external ownership. Direct installers should record their
installation kind immediately so later uncoordinated replacement is rejected.

`v1/record-installation` records this decision. It can make a direct-capable
runtime external but cannot make an external installation direct. The
external installation kind is immutable once present. A one-way transition
from direct to external may change the kind and stable path when a package
manager adopts an existing installation. Once external ownership is recorded,
a missing package-manager receipt cannot silently enable direct replacement.
That adoption must record the new ownership during installation, before a
normal invocation attempts to reconcile the changed executable.

On Unix, a direct replacement is committed through adjacent file renames while
keeping the previous executable recoverable until the change succeeds. Windows
starts a copy of the previous executable as a deferred worker. The worker waits
for the stable path to close, installs the candidate, and leaves any remaining
cleanup for the next invocation.

## Why Runtimes Refuse Unmanaged Prefixes

A runtime can find an existing directory at its install path. That directory may
be:

- a prefix created by the same runtime
- a prefix created by another runtime
- a normal conda installation
- an unrelated directory

conda-ship-generated runtimes refuse to operate on non-empty unmanaged prefixes.
This protects existing conda installations from accidental mutation.

Automatic bootstrap and later delegate invocations use ownership checks before
reusing an existing prefix.

## Lifecycle Commands

The generated runtime does not own `status`, `repair`, or `uninstall` commands.
Those names are passed to the configured delegate like every other argument.

For conda delegates, use `conda info` for status. Use `conda doctor` and its
supported fixes to diagnose and repair an installed prefix. Installer snapshot
and self-management commands can come from conda-self when a distribution
includes it. Removal of the runtime binary remains the responsibility of the
package manager or installer that placed it.

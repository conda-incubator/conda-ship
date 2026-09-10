# Generated Runtime Reference

Every conda-ship artifact is a stamped copy of the generic runtime template. In
this page, `RUNTIME` stands for the staged executable name and `DELEGATE`
stands for the configured executable inside the managed prefix.

The generated runtime installs the managed prefix and runs the delegate.
Normal command arguments belong to the delegate. When executable updates are
configured, a downstream transaction coordinator can also invoke an update
helper through environment variables.

## First Invocation

When the managed prefix is absent, the first invocation automatically installs
the stamped package set and then executes the delegate with the original
arguments:

```bash
RUNTIME info
```

For a conda delegate, that invocation bootstraps the prefix and then runs
`conda info`. For a Python delegate, this invocation bootstraps the prefix and
then runs `python --version`:

```bash
RUNTIME --version
```

A bare invocation also bootstraps and then invokes the delegate without
arguments.

The same behavior applies to `online`, `external`, and `embedded` artifacts.
Online artifacts download packages from the stamped runtime lock. External
artifacts read archives from the configured bundle directory. Embedded
artifacts automatically extract their built-in bundle.

During bootstrap, the runtime writes conda-ship ownership metadata and the
prefix metadata expected by conda tools in `conda-meta/history` and
`conda-meta/initial-state.explicit.txt`. It writes `.condarc` only when the
build configured `condarc-file`, and writes the CEP 22 frozen marker only when
the build configured `freeze-base = true`.

Bootstrap is serialized with a process lock next to the managed prefix. An
internal `installing` marker identifies an incomplete prefix owned by this
runtime. The runtime metadata file is written after package installation,
post-link scripts, prefix metadata, configured policy, bytecode compilation,
and delegate validation finish. Its `ready` state marks bootstrap complete.

If bootstrap is interrupted, the next invocation automatically retries only
when that internal marker belongs to the same stamped runtime. Recovery forces
every locked package through Rattler's reinstall path so post-link scripts run
again. It does not delete the prefix, named environments, or unrelated paths.
An unknown non-empty prefix is still refused.

## Delegate Execution

After the prefix is available, every argument belongs to the delegate. The
runtime does not reserve or rewrite any of these names:

- `--help` and `--version`
- `status` and `uninstall`
- `shell` and `self`
- `activate`, `deactivate`, and `init`
- delegate verbosity and quiet options

Standard input, output, and error pass through unchanged, and the runtime
preserves the delegate's signal and exit behavior. It does not filter delegate
output or set `CONDA_PREFIX`, `CONDA_DEFAULT_ENV`, or `CONDA_SHLVL`.

For a conda delegate, normal commands therefore look like direct conda
commands:

```bash
RUNTIME create -n myenv python=3.12 numpy
RUNTIME install -n myenv pandas
RUNTIME list -n myenv
RUNTIME env list
RUNTIME info
RUNTIME --help
RUNTIME --version
```

`RUNTIME info` is the normal status command for a conda delegate. If the
distribution includes a conda-spawn version that provides `conda shell`,
`RUNTIME shell` starts a shell through that plugin.

Use `conda doctor` and its supported fixes to diagnose and repair an installed
prefix. Use the commands supplied by conda-self for installer snapshots and
self-management when the distribution includes that plugin.

## Executable Updates

Executable updates are disabled unless the runtime was built with
`[tool.conda-ship.update]`. Runtimes without that table keep the normal
bootstrap and delegate behavior. The update engine does not reserve an
`update`, `self`, or other delegate subcommand.

The stamp records the update channel, package, and current build number. Every
update-enabled executable contains the direct update engine. The installed
metadata has two ownership modes:

`direct`
: A downstream coordinator can check, stage, and apply a native executable
  update. The executable resolves packages from its stamped conda channel and
  replaces its recorded stable path only after the coordinator approves the
  candidate and completes the inner transaction.

`external`
: The check result reports a newer package record and an optional instruction.
  The executable does not stage or apply that package. A package manager or
  installer replaces the executable. The next normal invocation validates the
  new stamp and reconciles the existing `.RUNTIME_NAME.json` record.

Every normal invocation recovers an interrupted replacement before starting
the delegate. A directly owned executable that changes outside the coordinated
flow is rejected. An externally owned executable can be reconciled when its
stamp and recorded identity are valid.

### Resolution And Verification

The runtime reads native `repodata.json` for the current platform and selects
the newest `.conda` package whose `(version, build number)` pair sorts after the
stamped executable. A higher version can therefore use a lower build number.
The runtime does not solve an environment and it does not install the update
package into the managed prefix.

Direct staging verifies the repodata size and SHA256, conda package metadata,
payload size and SHA256, executable stamp, runtime and artifact identity,
platform, version, build number, and update source. A candidate cannot rotate
its update channel or package.

Update channels must use `https://` or `file://`. Stamped URLs cannot contain
credentials, a query, or a fragment. HTTPS requests can read credentials from
the explicit JSON file selected with `RATTLER_AUTH_FILE`. The runtime does not
enable keyring, netrc, or default auth-file discovery. It does not provide an
interactive login or a provider-specific API.

Online requests cache repodata and verified package content. Offline HTTPS
checks require cached repodata and offline staging requires the selected
package content to be cached. A `file://` channel reads local repodata and
packages directly.

## Version-One Coordinator API

The helper provides an API for downstream transaction coordinators and
installers. The coordinator
invokes the stamped executable as a child process with `CONDA_SHIP_PREFIX` set
to the managed prefix when it needs to override the runtime's stamped install
location.

Before invoking check, stage, or apply, the coordinator opens
`<prefix>/.RUNTIME_NAME.update.lock` and holds an exclusive operating-system
file lock. The runtime creates this one-byte regular file during update
initialization. The coordinator must hold it through check, stage, the inner
transaction, and apply. Each version-one action fails when the lock is not
held.

### Record Installation

An installer or delivery detector records how this copy of the executable is
managed:

```text
CONDA_SHIP_INTERNAL_UPDATE=v1/record-installation
CONDA_SHIP_INTERNAL_UPDATE_OWNERSHIP=external
CONDA_SHIP_INTERNAL_UPDATE_INSTALLATION=homebrew
CONDA_SHIP_INTERNAL_UPDATE_EXECUTABLE=/opt/homebrew/bin/demo
```

This action completes bootstrap when needed and does not invoke the delegate.
The executable path must be absolute and resolve to the running executable.
External package managers should pass their stable executable path rather than
a versioned target.

Direct installers use `direct` ownership and an installation label such as
`standalone` or `constructor`. An external installer should also set
`CONDA_SHIP_INTERNAL_UPDATE_INSTRUCTION` to the final user-facing update
instruction when one is available.

Bootstrap metadata created before an installer records this value has no
installation label. This compatibility state may reconcile a valid newer
executable with the same runtime identity and update source. The installer or
delivery detector should record ownership before the first update check.

The action writes one JSON object:

```json
{
  "recorded": true
}
```

The updated values are persisted in `.RUNTIME_NAME.json`, which is the source
of truth for later checks.

Recording may change a direct installation to external. It cannot make an
external installation direct or change an existing external instruction.
Changing direct to external may also replace the installation label and
stable path, which supports moving an existing standalone runtime under a
package manager. Other attempts to change an existing installation label
are rejected. The action also rejects pending executable replacement state.
An adopting package manager must invoke this action during installation,
before the replacement executable is run normally. Post-delegation receipt
detection cannot adopt a confirmed direct installation after its executable
has already changed.

### Check

Set:

```text
CONDA_SHIP_INTERNAL_UPDATE=v1/check
```

A successful check writes one JSON object to stdout:

```json
{
  "available": true,
  "current_version": "1.0.0",
  "current_build_number": 0,
  "version": "1.1.0",
  "build_number": 0,
  "package": "demo-runtime",
  "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "ownership": "direct",
  "installation": "standalone",
  "instruction": null
}
```

When no candidate exists, `available` is `false` and the candidate fields are
`null`. Ownership, installation, and instruction come from the installed
runtime metadata rather than the executable stamp. External ownership must not
continue to stage.

If check completes recovery of an interrupted update, it exits with an error
that asks the coordinator to retry. This keeps recovery separate from candidate
selection.

### Stage

After the user or non-interactive policy approves a direct candidate, set:

```text
CONDA_SHIP_INTERNAL_UPDATE=v1/stage
CONDA_SHIP_INTERNAL_UPDATE_CANDIDATE=SHA256_FROM_CHECK
```

Stage resolves the candidate again, rejects a changed selection, downloads and
validates the package, and copies the executable next to the stable executable.
A successful stage writes:

```json
{"staged":true}
```

The coordinator then performs the inner transaction while retaining the update
lock. If that transaction fails, it must not invoke apply. It releases the
lock and leaves the old executable working. The next normal invocation or
check discards an unapproved staged candidate.

### Apply

After the inner transaction succeeds, set:

```text
CONDA_SHIP_INTERNAL_UPDATE=v1/apply
```

On Unix, a successful atomic replacement writes:

```json
{"applied":true}
```

On Windows, apply can defer replacement until the running executable exits:

```json
{"applied":false,"replacement_pending":true}
```

The coordinator releases the update lock after apply returns. Helper failures
write diagnostics to stderr and exit nonzero. Successful actions write one JSON
object to stdout.

Set `CONDA_SHIP_INTERNAL_UPDATE_OFFLINE=1` on check and stage to disable network
access. Empty, `0`, and `false` leave network access enabled. The comparison
with `false` is case-insensitive. This flag is separate from the
runtime-specific bootstrap offline variable.

All persistent update and recovery state remains inside the existing
`.RUNTIME_NAME.json` prefix metadata file. The helper introduces no daemon,
service, receipt, or second metadata record.

## Windows Deferred Replacement

Windows cannot replace the executable while the current process is using it.
Apply preserves a verified copy of the old executable as a detached replacement
worker, records the replacing state, and returns `replacement_pending`.

The worker waits up to 30 seconds for the stable executable to close, installs
the staged candidate, and leaves the old copy in place until the new stable path
is verified. The next invocation completes metadata reconciliation and cleanup.
If the worker is interrupted or times out, the old executable remains usable
and a later invocation retries recovery.

## Bootstrap Controls

`CONDA_SHIP_PREFIX` is the universal managed-prefix override. It takes
precedence over a runtime-specific prefix variable. Bundle and bootstrap
offline controls are derived from the configured runtime name.
Non-alphanumeric characters become underscores and letters are uppercased.

For a runtime named `demo`, the variables are:

`DEMO_PREFIX`
: Compatibility override for the managed prefix path. Runtime names other than
  `conda` accept this derived form. A runtime named `conda` ignores
  `CONDA_PREFIX` because it can describe an activated environment. Use
  `CONDA_SHIP_PREFIX` for that runtime.

`DEMO_BUNDLE`
: Path to an external directory containing the package archives named in the
  stamped runtime lock.

`DEMO_OFFLINE`
: Disable network access during bootstrap. Empty, `0`, and `false` disable the
  flag. Other non-empty values enable it. The comparison with `false` is
  case-insensitive.

Example:

```bash
CONDA_SHIP_PREFIX=/opt/demo \
DEMO_BUNDLE=/opt/demo-bundle \
DEMO_OFFLINE=1 \
demo info
```

The runtime reads these values only to locate and bootstrap the managed prefix.
They do not consume or replace delegate arguments.

Embedded artifacts need no bundle or offline override:

```bash
CONDA_SHIP_PREFIX=/opt/demo demo info
```

For local smoke tests through the builder, prefer `cs run --install-path PATH`
instead of setting the prefix variable yourself:

```bash
cs run --install-path /tmp/demo-smoke -- info
```

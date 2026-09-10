# Fleet API Reference

Fleet is an experimental Rust API for downstream orchestrators that manage
multiple locked prefixes. Enable it explicitly:

```toml
[dependencies]
conda-ship = { git = "https://github.com/conda-incubator/conda-ship", rev = "<pinned-commit>", features = ["fleet"] }
```

For a native-TLS caller:

```toml
[dependencies]
conda-ship = { git = "https://github.com/conda-incubator/conda-ship", rev = "<pinned-commit>", default-features = false, features = ["fleet", "native-tls"] }
```

Fleet does not solve environments, publish catalogs, add a runtime command
namespace, write global launchers, or edit shell startup files. Stamped runtime
artifacts remain conda-ship's primary build output.

## Core Types

`Fleet::new` selects the install root:

```rust
use conda_ship::fleet::Fleet;

let fleet = Fleet::new("/tmp/fleet");
```

Each runtime is installed under `install_root/<id>`. `RuntimeSpec` contains the
inputs for one runtime:

```rust
use conda_ship::fleet::RuntimeSpec;

let spec = RuntimeSpec {
    id: "demo".to_string(),
    version: "2026.7.0".to_string(),
    delegate_executable: "demo".to_string(),
    lock_content: std::fs::read_to_string("runtime.lock")?,
    requested_specs: vec!["demo".to_string()],
    condarc: None,
    freeze_base: false,
    installer: None,
};
```

The delegate is required and has no `conda` default. `RuntimeSpec::validate()`
checks the runtime id, delegate name, version, lock content, optional condarc
mapping, and optional installer type.

For a runtime already selected as a conda-ship stamped artifact, use the
supported adapter instead of parsing the private stamp format:

```rust
let spec = RuntimeSpec::from_stamped_artifact("dist/demo")?;
```

The adapter preserves the stamped runtime identity, version, explicit
delegate, lock, package specs, exact condarc text, frozen-base policy, and
installer provenance. The same mapping applies to online, external, and
embedded artifact layouts.

The adapter verifies the stamped header checksum and validates the resulting
spec. It does not authenticate the artifact, verify or extract embedded
bundles, select install options, or choose update policy. Callers remain
responsible for artifact selection and trust. Fleet bundle input remains an
explicit `bundle_dir`.

`condarc` contains exact `.condarc` text supplied by the caller. `None` means
the final Fleet runtime has no `.condarc`. `freeze_base` controls the CEP 22 marker.
`installer` controls Constructor-compatible `.installer.info` provenance. All
three default to disabled and none of them are inferred from the lock.

`RuntimeSpec::lock_sha256()` returns the digest written to prefix metadata.
Channels from the default lock environment are recorded separately as
provenance. They do not become condarc policy.

## Install And Recovery

Install a selected lock with:

```rust
use conda_ship::fleet::InstallOptions;

let installed = fleet.install(spec, InstallOptions::default()).await?;
```

`InstallOptions` controls:

- `force`: reinstall every selected locked package in an existing prefix owned
  by the same Fleet runtime id
- `offline`: disable package downloads
- `bundle_dir`: add local `.conda` and `.tar.bz2` archives to the shared package
  cache before installation

Fleet serializes prefix mutations with the same adjacent cross-process lock as
stamped runtimes. It writes an installing marker before installing packages. It
writes the ready metadata through a synced temporary file and rename only after
package installation, configured `.condarc` and CEP 22 files, Constructor
metadata, bytecode compilation, and delegate validation succeed.

If an owned install was interrupted, the next install retries it by
reinstalling every package in the selected lock. A forced install uses the same
in-place transaction. Both preserve named environments and unrelated files.
Fleet never turns `force` into recursive prefix deletion.

Fleet refuses unknown non-empty prefixes and prefix symlinks. It also refuses
metadata and managed policy paths that are symlinks or other nonregular entries.

Fleet manages these files:

- `<prefix>/.condarc`
- `<prefix>/conda-meta/frozen`
- `<prefix>/.installer.info`

Before package mutation, Fleet verifies that existing entries at those paths
are regular files. It removes them after the installing marker is committed,
rechecks them after package installation, then rewrites only the outputs enabled
by the new `RuntimeSpec`. This removes files when an option changes from enabled
to disabled.

## List, Get, And Remove

```rust
let runtimes = fleet.list()?;
let maybe_demo = fleet.get("demo")?;
fleet.remove("demo")?;
```

`Fleet::list()` scans direct children of the install root and returns prefixes
with valid Fleet metadata. `Fleet::get(id)` waits for the prefix mutation lock,
then validates the ready metadata file and recorded delegate. It
returns `None` when the metadata file is absent.

`Fleet::remove(id)` is the only recursive prefix deletion operation. It uses the
same mutation lock and removes only an empty directory, a prefix with matching
ready metadata, or an incomplete prefix with a matching installing marker. A
metadata symlink is rejected.

## Installed Runtime And Commands

`InstalledRuntime` contains the id, version, prefix, explicit delegate,
lockfile channels, lock SHA256, and requested specs.

```rust
let runtime = fleet.get("demo")?.expect("installed");
let command = runtime.command("demo")?;
let executable = runtime.executable_path("demo");
let path_entries = runtime.path_entries();
```

`RuntimeCommand` contains only the executable and prefix-local PATH entries.
Fleet does not set conda activation variables. Callers can prepend the returned
entries to the existing child PATH just as a stamped runtime does.

## Shim Plans And Launcher Ownership

Fleet can return a shim plan:

```rust
let plan = runtime.shim_plan("demo", "demo", None)?;
```

`ShimPlan` contains a destination, target command, executable, and PATH entries.
Fleet never writes or removes the launcher. The downstream orchestrator owns
file contents, overwrite checks, PATH setup, and removal.

Launchers created by Fleet callers are externally managed. The downstream
orchestrator owns their update and removal policy.

## Cache, Bundle, And Offline Behavior

All Fleet runtimes use rattler's standard shared package cache. Installing a
second runtime can reuse package archives and extracted cache entries already
present from the first runtime.

`bundle_dir` indexes flat `.conda` and `.tar.bz2` archives, verifies that they
match locked records, and populates the same cache before running the install
transaction. With `offline = true`, every locked package must already be in the
cache or supplied bundle. Missing packages fail the install and leave the owned
installing state available for a later retry.

Fleet does not own cache retention, cleanup schedules, enterprise mirroring, or
bundle publication. CI and managed deployments should populate a deterministic
bundle or cache before selecting offline mode. A stamped artifact with an
embedded bundle remains a stamped-artifact workflow. A Fleet caller supplies a
bundle directory explicitly.

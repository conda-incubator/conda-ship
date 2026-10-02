# Configuration Reference

Configure the runtime in your project manifest. conda-ship reads the selected
environment's resolved package records from the matching lockfile.

`cs inspect`, `cs build`, and `cs run` can read the supported manifest and
lockfile pairs.
Packaged builds find the installed runtime template automatically, so local
projects do not need a conda-ship source checkout.

## Manifest Discovery

conda-ship looks in the build root for:

1. `conda.toml`
2. `pixi.toml`
3. `pyproject.toml` with a nonempty `[tool.conda.workspace]` or
   `[tool.pixi.workspace]` table

The selected manifest determines the lockfile:

| Manifest | Lockfile |
| --- | --- |
| `conda.toml` | `conda.lock` |
| `pixi.toml` | `pixi.lock` |
| `pyproject.toml` with nonempty `[tool.conda.workspace]` | `conda.lock` |
| `pyproject.toml` with nonempty `[tool.pixi.workspace]` | `pixi.lock` |

When both workspace tables are nonempty, `[tool.conda.workspace]` takes
precedence. An empty `[tool.conda.workspace]` or a `[tool.conda]` table without
workspace settings does not override a nonempty `[tool.pixi.workspace]`.
`[tool.conda-ship]` is separate build policy and does not make `pyproject.toml`
a source manifest on its own.

`conda.lock` and `pixi.lock` are source lockfiles owned by their respective
workspace tools. conda-ship derives a runtime lock from that source lockfile
while inspecting, building, or smoke-testing a runtime.

## Source Environment

The selected source environment determines the conda packages available to the
generated runtime. Only conda packages are supported. Locked PyPI packages in
that environment, on any platform, cause an `unsupported_pypi_packages`
diagnostic. Other environments in the source lockfile may contain PyPI packages.
Use conda packages for runtime dependencies, or select a dedicated environment
without PyPI packages and refresh the lockfile.

In `conda.toml` or `pixi.toml`, use a dedicated `ship` environment for the
packages that should be included in the runtime:

```toml
[feature.ship.dependencies]
python = ">=3.12"
conda = ">=25.1"
conda-rattler-solver = "*"
conda-spawn = ">=0.1.0"

[environments]
ship = { features = ["ship"], no-default-feature = true }
```

In `pyproject.toml`, conda-workspaces sections live below `[tool.conda]`, for
example `[tool.conda.feature.ship.dependencies]`. Pixi sections live below
`[tool.pixi]`, for example `[tool.pixi.feature.ship.dependencies]`.

conda-ship does not require specific packages in the selected environment. The
environment must provide the configured delegate executable. Conda-like
distributions include `conda` and the plugins they use. For example,
`RUNTIME shell` is available when the selected conda-spawn version provides
`conda shell`.
Generated runtimes automatically install the selected environment
as the managed base prefix, then pass every argument to the configured delegate
executable inside that prefix.

`conda-self` is optional. Include it in the selected source environment when
the runtime should expose `conda self reset` for the managed base prefix.
Generated runtimes always write the reset snapshot that `conda-self` expects.

## `[tool.conda-ship]`

`[tool.conda-ship]` records conda-ship-specific build policy:

```toml
[tool.conda-ship]
runtime-name = "demo"
artifact-name = "demo-cli"
runtime-version = "1.0.0"
delegate-executable = "conda"
artifact-layout = "online"
source-environment = "ship"
exclude-packages = ["conda-libmamba-solver"]
docs-url = "https://example.com/demo/"
install-scheme = "conda-home"
install-name = "demo"
installer = "homebrew"
condarc-file = "runtime.condarc"
freeze-base = true

[tool.conda-ship.update]
channel = "https://packages.example.com/conda"
package = "demo-runtime"
build-number = 0
```

For the naming model behind `runtime-name`, `artifact-name`, `install-name`, and
`runtime-version`, see {doc}`names`.

`runtime-name`
: Base runtime identity and default artifact name. `cs build` and `cs run`
  require this value, either here or through `--runtime-name`. It is not a
  conda environment name.

`artifact-name`
: Optional staged executable and artifact stem for any layout. When omitted,
  builds use `runtime-name` exactly. Set this when a release artifact should
  have a distinct command name, such as `cxz` while keeping
  `runtime-name = "cx"` for install metadata and environment variable names.

`runtime-version`
: Version stamped into runtime and prefix ownership metadata.
  When omitted from `[tool.conda-ship]`, conda-ship uses static
  `[project].version` from the selected `pyproject.toml` if it exists. Release
  workflows can override this with `cs build --runtime-version VERSION` or the
  GitHub Action `runtime-version` input.

  Projects that declare `dynamic = ["version"]` can opt into standards-based
  metadata resolution:

  ```toml
  [tool.conda-ship]
  runtime-version = { from = "project-metadata" }
  ```

  The Python `conda ship` adapter resolves this source before invoking `cs`: it
  calls the project's PEP 517 `prepare_metadata_for_build_wheel` hook, reads
  `Version` from the generated `.dist-info/METADATA`, and passes the resolved
  value as `--runtime-version` on `cs build` or `cs run`. It does not fall back
  to building a wheel. The build backend must already be installed in the
  Python environment running
  `conda ship`.

`delegate-executable`
: Required executable name inside the managed prefix that receives every
  argument after automatic bootstrap. Set it here or pass
  `--delegate-executable`. Use `conda` for conda-like runtimes such as `cx`. Other
  values, such as `python`, are supported when a runtime should expose a
  different command interface.

`artifact-layout`
: Artifact layout to build. Supported values are `online`, `external`, and
  `embedded`. When omitted, `cs build` defaults to `online`.

`source-environment`
: Name of the solved environment to turn into the runtime lock. This value is
  required. conda-ship does not fall back to a default environment because that
  can accidentally ship development or test dependencies.

`exclude-packages`
: Package names removed from the derived runtime lock, including dependencies
  used only by excluded packages.

`docs-url`
: Documentation URL stamped into generated runtime metadata. Must start
  with `https://` or `http://` and must not contain whitespace or control
  characters.

`install-scheme`
: Install scheme stamped into the generated runtime. Supported values are
  `conda-home`, which installs below `~/.conda/INSTALL_NAME`, and `user-data`,
  which installs below the platform user data directory. `conda-home` is the
  default when `install-scheme` is not configured.

`install-name`
: Directory name for this runtime's managed base prefix under the install
  scheme. When omitted, conda-ship uses the runtime name. For example,
  `runtime-name = "cx"` can use `install-name = "express"` so the `conda-home`
  install scheme resolves to `~/.conda/express`.
  Choose a product-specific install name. conda-ship does not reserve names
  under `~/.conda`. It relies on runtime metadata to avoid overwriting prefixes
  owned by other tools.

  ```{versionadded} 0.8.0
  The derived `install-name = { from = "runtime-lock" }` form.
  ```

  To derive a content-addressed name from immutable runtime inputs, use
  `install-name = { from = "runtime-lock" }`. Add `base = "express"` inside
  the table to override the default runtime-name base. The builder resolves it
  to
  `BASE-RUNTIME_VERSION-LOCK_HASH`, where `BASE` is the configured `base` or
  `runtime-name` and `LOCK_HASH` is the first 16 lowercase hexadecimal
  characters of the SHA-256 of the exact rendered runtime lock bytes.
  Changing the runtime version or runtime lock selects a new managed prefix.
  Existing prefixes are retained. The derived name does not itself prevent
  package changes inside the prefix. Use `freeze-base = true` for the separate
  CEP 22 mutation guard. Content-addressed runtime versions must use only ASCII
  letters, digits, dots, dashes, and underscores. The resolved name is limited
  to 128 characters. The derived form cannot be combined with
  `[tool.conda-ship.update]`. An explicit `cs build --install-name` or GitHub
  Action `install-name` input remains a final-name override and disables
  derivation for that build.

`installer`
: Optional package manager or installer hint stamped into the generated runtime.
  Release workflows can override this with `cs build --installer INSTALLER` or
  the GitHub Action `installer` input.

  When configured, automatic bootstrap writes Constructor-compatible
  `<prefix>/.installer.info` JSON with the exact fields `name`, `version`,
  `platform`, and `type`. The configured `installer` value becomes `type`.
  This metadata records how the prefix was distributed. Update and uninstall
  code must not use it as a launcher ownership record.

`condarc-file`
: Optional path to a YAML condarc file. Relative paths are resolved from
  the selected project manifest. The builder requires a YAML mapping and stamps
  the file's exact text content into the runtime. During bootstrap, the runtime
  writes that content to `<prefix>/.condarc`.

  When omitted, conda-ship does not create, replace, or remove `.condarc`.
  The runtime lock still records the channels used to build the prefix.
  conda-ship does not merge them into persistent conda configuration.

`freeze-base`
: Whether bootstrap writes the existing CEP 22 marker to
  `<prefix>/conda-meta/frozen`. Defaults to `false`. When false, conda-ship
  leaves any marker created by an installed package untouched.

## `[tool.conda-ship.update]`

`[tool.conda-ship.update]` opts a stamped runtime into executable update
metadata and the runtime update engine. Omitting the table preserves the
normal bootstrap and delegate behavior without executable update handling.

The table is supported for `online` and `embedded` artifact layouts. The
`external` artifact layout is not supported.

`channel`
: Absolute conda channel URL used to resolve native runtime update packages.
  The URL must use `https://` or `file://`. It must not contain credentials, a
  query, or a fragment.

`package`
: Conda package name used for runtime update records. The package must contain
  one finalized native runtime executable produced by `cs package-update`.

`build-number`
: Build number for the stamped executable. Defaults to `0`. Candidate ordering
  compares the conda version first and the build number second.

Every update-enabled executable contains the direct update engine. Installed
ownership and any external update instruction belong to the installed copy,
not the build. An installer or delivery detector records them in
`.RUNTIME_NAME.json` through `v1/record-installation`. The same executable bytes
can therefore be installed directly or by an external package manager.

`ownership` and `instruction` are rejected as new build settings. They remain
readable only in stamps produced by conda-ship 0.6.x.

Generated runtimes write ownership metadata into every bootstrapped prefix.
That metadata records the schema version, display name derived from
`runtime-name`, install name, and metadata filename expected by the runtime.
Automatic bootstrap refuses to use an existing non-empty conda prefix when
that ownership metadata is missing, invalid, or belongs to another stamped
runtime.

Generated runtimes also write constructor-compatible prefix metadata into
`conda-meta/history` and `conda-meta/initial-state.explicit.txt`. Conda uses
the history file to recognize the prefix as an environment and to preserve the
runtime's requested package specs for future conda operations. The explicit
initial-state file records the exact package URLs and checksums from the
stamped runtime lock. When `conda-self` is installed in the runtime, it uses
that file as the installer snapshot for the `installer-updated` and
`installer-exact` reset modes.

Keep package selection and lockfile channels in the selected source environment.
conda-ship records the resolved package names and channel URLs in runtime
metadata. It writes persistent conda configuration only when `condarc-file` is
set.

## Stamped Runtime Metadata

`cs build` stamps these values onto the runtime after resolving `runtime-name`,
`artifact-name`, and `artifact-layout` from CLI flags or `[tool.conda-ship]`:

- artifact name: `ARTIFACT_NAME`, or `RUNTIME_NAME` when
  `artifact-name` is not configured
- runtime version: the configured `runtime-version`, static
  `[project].version` from the selected `pyproject.toml`, or the concrete
  value resolved by `conda ship` from `{ from = "project-metadata" }`. Builds
  fail when no downstream version can be resolved
- runtime name: `RUNTIME_NAME`
- delegate executable: the configured `delegate-executable`
- install scheme: `conda-home`, or the configured `install-scheme`
- install name: `RUNTIME_NAME`, the literal `install-name`, or the resolved
  content-addressed name from the derived `install-name` form
- installer: the configured `installer`, when present
- condarc contents: the exact text from `condarc-file`, when configured
- frozen base policy: the configured `freeze-base` value, defaulting to `false`
- executable update configuration: channel, package, and build number when
  `[tool.conda-ship.update]` is configured
- metadata file: `.RUNTIME_NAME.json`
- bundle environment variable: uppercased `RUNTIME_NAME` plus `_BUNDLE`
- offline environment variable: uppercased `RUNTIME_NAME` plus `_OFFLINE`

`CONDA_SHIP_PREFIX` is the universal managed-prefix override and takes
precedence over runtime-specific variables. For runtime names other than
`conda`, the runtime also accepts uppercased `RUNTIME_NAME` plus `_PREFIX`.
A runtime named `conda` ignores `CONDA_PREFIX` as an install override because
that variable can describe an activated conda environment.

At bootstrap time, the generated runtime writes a separate prefix metadata file
inside the managed prefix. That file is used for ownership checks before later
operations touch the prefix. It is written last to mark bootstrap complete.
The internal installing marker is then removed.

When executable updates are configured, the same `.RUNTIME_NAME.json` file
stores update and recovery state. It records the stable
executable path, artifact identity, update channel and package, build number,
installed ownership, installation kind, executable SHA256, optional external
instruction, and any pending replacement. The adjacent
`.RUNTIME_NAME.update.lock` file coordinates processes but is not another
update state record.

The bootstrap also writes standard conda prefix metadata:

- `conda-meta/history`
- `conda-meta/initial-state.explicit.txt`

These files are not stamped into the runtime binary. They are rendered from the
runtime lock when the prefix is bootstrapped.

The runtime writes `.condarc` and the CEP 22 frozen marker only when their
corresponding options are set.

Non-alphanumeric characters in environment variable names become underscores.

## Downstream Defaults

conda-ship's repository default package set exists so the builder and
runtime behavior can be tested. A downstream distribution makes its own
package choices in its project manifest before committing the matching lockfile.

For example, conda-express owns the package set and runtime names used when
building `cx` and `cxz`. Those choices are conda-express policy, not
conda-ship policy.

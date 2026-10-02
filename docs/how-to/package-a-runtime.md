# Package A Runtime

Use this guide after `cs build` has produced runtime artifacts and you want to
hand those files to another distribution channel.

conda-ship does not generate `.sh`, `.pkg`, `.msi`, Homebrew formulae, Docker
images, or constructor installers. It produces runtimes and metadata that those
systems can wrap.

::::{grid} 1 1 2 3
:gutter: 3

:::{grid-item-card} Direct Assets

Upload the complete `dist/` contents to a release channel.
:::

:::{grid-item-card} Package Managers

Install the runtime binary through Homebrew, a conda package, or another
package manager.
:::

:::{grid-item-card} Installers And Images

Wrap online, external, or embedded runtimes in installers, Docker images, or
internal deployment systems.
:::

::::

## Start From The Output Directory

Every build writes a directory like `dist/`:

```bash
cs build --out-dir dist
```

For release automation, use the GitHub Action `dist-path` output. It contains
all files produced by the build.

```yaml
- uses: conda-incubator/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  id: cs
  with:
    conda-ship-version: "X.Y.Z"

- uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
  with:
    name: ${{ steps.cs.outputs.asset-name }}
    path: ${{ steps.cs.outputs.dist-path }}
```

## Publish Direct Release Assets

For direct GitHub Releases, upload the full `dist/` contents:

- runtime binary
- optional `.bundle.tar.zst`
- `.runtime.lock`
- `.packages.txt`
- `.cdx.json`
- `.info.json`
- `.sha256`

The metadata files help users and downstream packagers inspect what was built.
Do not publish only the runtime binary unless your release channel has another
place for checksums and package metadata.

```{important}
Treat `dist/` as the release unit. If a channel uploads only the runtime binary,
it needs an equivalent place for checksums, package metadata, and provenance.
```

## Publish Executable Updates

An update-enabled runtime discovers finalized executable releases through
native conda channel repodata. The release sequence is:

1. Build the runtime and keep its `.info.json` file.
2. Apply platform signing or other processing that changes the executable.
3. Run `cs package-update` against those finalized bytes.
4. Add the resulting native `.conda` package to the configured channel index.
5. Upload the indexed channel with the downstream project's normal tooling.

```bash
cs package-update \
  --info dist/demo.info.json \
  --binary dist/demo \
  --out-dir update-packages
```

On Windows, pass the finalized `.exe` through `--binary`. The command snapshots
the file, verifies its stamp against the build information, and writes a native
package with one executable payload. It refuses to replace an existing package
filename.

The package is update transport. A directly owned runtime downloads and
inspects it before replacing the outer executable. It is not installed into the
managed prefix. conda-ship does not index or upload the package.

```{important}
A Windows runtime built with conda-ship 0.8.0 or earlier cannot validate a
0.9.0 update candidate because its reader expects the legacy trailing footer.
An adopting installer or package manager must call `v1/record-installation` to
record external ownership during replacement, before the new executable's first
normal invocation. An installation that must remain directly managed needs a
fresh install that does not reuse its old direct-install metadata. Once a 0.9.0
runtime is installed, later native updates can use packages made from finalized
0.9.0 artifacts.
```

For GitHub Action builds, `cs-path` is the absolute path to the downloaded
builder. Use that output for the post-sign packaging step:

```yaml
- name: Package finalized executable
  shell: bash
  run: |
    "${{ steps.cs.outputs.cs-path }}" package-update \
      --info "${{ steps.cs.outputs.info-path }}" \
      --binary "${{ steps.cs.outputs.binary-path }}" \
      --out-dir update-packages
```

`cs package-update` accepts the canonical update-enabled executable. Package
managers can install those same bytes and record the installed copy as external
with `v1/record-installation`. The indexed package remains the common release
signal. An external installation does not stage its payload, and a replacement
at the recorded stable path is reconciled on the next runtime invocation.

## Wrap With Homebrew

For an online runtime, a Homebrew formula usually installs the runtime binary
and lets the runtime download packages at first bootstrap.

Set installer metadata when downstream packaging needs to record the provider:

```bash
cs build --installer homebrew
```

The formula should install the runtime onto `PATH`. It should not modify the
managed prefix directly. The first invocation automatically bootstraps the
prefix, then every invocation delegates its arguments. The downstream runtime
coordinator should validate the Homebrew keg receipt and record external
ownership with the stable Homebrew `bin` link before checking for an update.

## Wrap With A Conda Package

A conda package can install the runtime binary into the package environment.
This is useful for distributing a downstream runtime in a conda channel.

The two installation steps are:

- the conda package installs the runtime binary
- the generated runtime bootstraps and owns its managed prefix

Use `installer` to record where the runtime binary came from:

```bash
cs build --installer conda-package
```

## Wrap With constructor Or Another Installer

Installer generators can include either:

- an online runtime and no package bundle
- an external runtime plus extracted or adjacent bundle
- an embedded runtime

For `external`, place the extracted bundle where the installer or first-run
script can expose it through the variables derived from the runtime name:

```bash
DEMO_BUNDLE=/path/to/bundle DEMO_OFFLINE=1 demo info
```

For another runtime name, derive the variable names by uppercasing it and
replacing non-alphanumeric characters with underscores.

For `embedded`, no extra bundle path is needed:

```bash
demo info
```

The installer should not unpack the managed conda prefix by itself. Let the
runtime bootstrap so ownership metadata, configured condarc and frozen-base
policy, constructor-compatible prefix metadata, and package verification are
applied consistently.

```{warning}
Do not preinstall the managed prefix behind the runtime's back. Runtime
bootstrap writes ownership metadata, `conda-meta/history`,
`conda-meta/initial-state.explicit.txt`, and verification state that later
delegate and conda-self commands rely on.
```

## Package For Docker Or Internal Images

For images, decide whether bootstrap happens at image build time or container
run time.

Build-time bootstrap gives faster startup:

```dockerfile
COPY demo /usr/local/bin/demo
ENV CONDA_SHIP_PREFIX=/opt/demo
RUN demo info
```

Run-time bootstrap gives a smaller image layer before first use:

```dockerfile
COPY demo /usr/local/bin/demo
ENV CONDA_SHIP_PREFIX=/opt/demo
ENTRYPOINT ["demo"]
```

Use `CONDA_SHIP_PREFIX` in images. Runtime-specific `_PREFIX` variables remain
available for names other than `conda`. Avoid relying on a user home directory
when the image will run as different users.

## Verify Before Publishing

Before handing files to another system:

```bash
(cd dist && shasum -a 256 --check ./*.sha256)
```

The build checksum describes the staged executable. If signing changes those
bytes, keep the build metadata for identity checks and pass the finalized file
to `cs package-update --binary`. Add `--json` to report the package and payload
digests for the finalized bytes.

For GitHub Action builds, also keep the release attestation checks enabled in
the action. They verify the conda-ship tools used to stamp the downstream
runtime.

For release workflows, attest the unchanged output directory before signing.
After signing, attest the finalized executable separately or generate and
attest a downstream final manifest. See {doc}`verify-release-artifacts` for the
complete sequence.

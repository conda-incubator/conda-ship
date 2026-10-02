# Artifact Reference

Every `cs build` writes a runtime plus metadata files. The runtime
is the final stamped binary artifact. Downstream signing and attestation
workflows run after conda-ship writes these files.

## conda-ship Release Assets

Tagged conda-ship releases publish the builder assets that the GitHub Action
downloads:

`cs-<target>`
: Builder CLI for the target platform.

`cs-template-<target>`
: Generic runtime template for the target platform.

`SHA256SUMS`
: Checksums for release assets.

The PyPI release for the same tag publishes platform wheels that install `cs`
and `cs-template` into the Python environment's scripts directory, plus a
source distribution for packaging systems. Published release assets are
immutable. Fixes use a new tag rather than replacing files under an existing
tag.

## Layouts

| Layout | Runtime | Bundle file | Network during bootstrap |
| --- | --- | --- | --- |
| `online` | `RUNTIME` | none | yes |
| `external` | `RUNTIME` | `RUNTIME.bundle.tar.zst` | optional |
| `embedded` | `RUNTIME` | embedded in binary | no |

When `[tool.conda-ship].artifact-name` or `--artifact-name` is set, all layouts use
that value for the staged runtime and metadata stem. On Windows, binary
filenames also include `.exe`.

For the difference between `runtime-name` and `artifact-name`, see
{doc}`names`.

## Runtime Update Packages

`cs package-update` writes a dependency-free native `.conda` package from a
finalized update-enabled `online` or `embedded` runtime. Run it separately
after `cs build`. By default, it writes next to the `.info.json` passed through
`--info`. Use `--out-dir` to write update packages to a separate directory.

The package contains one executable payload plus normal conda package metadata:

- `bin/ARTIFACT_NAME` on Unix
- `ARTIFACT_NAME.exe` on Windows

The package name, version, build number, and platform come from the executable
stamp and its artifact info JSON. The package has no runtime dependencies. It
is transport for the executable update engine and is not installed into the
managed prefix.

The command refuses to overwrite an existing output. Its `--json` result
reports SHA256 and size values for both the package and finalized executable
payload. Channel indexing and upload remain downstream release operations.

## Bundle Contents

Bundles are a transport for the conda package archives already named in the
runtime lock. They are not channel mirrors and do not use a `linux-64/` or
`noarch/` directory layout.

`external` and `embedded` bundles contain top-level `.conda` and `.tar.bz2`
package archive files. The runtime matches those filenames against the stamped
lockfile and verifies package SHA256 values before installing from them.

External bundle directories may contain unrelated files, but conda-ship only
indexes top-level conda package archives and skips symbolic links. Embedded
bundles are stricter because they are extracted from a tar archive: every entry
must be a top-level regular `.conda` or `.tar.bz2` file. Directory entries,
nested paths, symbolic links, hard links, and other file types are rejected.

## Metadata Files

For an `online` build with runtime `demo`, conda-ship stages:

- `demo` or `demo.exe`
- `demo.runtime.lock`
- `demo.packages.txt`
- `demo.cdx.json`
- `demo.info.json`
- `demo.sha256`

When `--target-label` is used, the label is inserted into the stem, for example
`demo-linux-64.info.json`.

If `artifact-name = "demo-cli"` or `--artifact-name demo-cli` is set, the artifact
stem uses that explicit name for any layout, for example `demo-cli.info.json`
or `demo-cli-linux-64.info.json`.

For an `external` build, conda-ship also stages `demo.bundle.tar.zst` or a
target-qualified equivalent. Rebuilding the same artifact stem as `online` or
`embedded` retires any previous external bundle before publishing the new
checksum manifest.

These files describe the staged release output. During automatic first-run
bootstrap, the generated runtime also writes managed-prefix metadata such as
`conda-meta/history` and `conda-meta/initial-state.explicit.txt` inside the
install path.

## CycloneDX SBOM

Every staged build writes a CycloneDX 1.7 JSON SBOM named
`ARTIFACT.cdx.json`. The document identifies the staged runtime as the root
application and includes every conda package for the target platform in the
derived runtime lock. Package components include the exact version, build,
subdir, channel, filename, download URL, SHA256 and MD5 hashes, and license
value when those fields are available. Direct dependency edges come from the
solved package records. Dependency sets containing conditional or unparseable
MatchSpecs are marked `unknown` rather than inventing relationships that the
runtime lock cannot prove.

Channel and remote download URLs are sanitized before they are written. URL
credentials, Anaconda `/t/<token>` path segments, queries, and fragments are
removed. Local paths and `file:` URLs are omitted.

This mapping follows the conda specifications for
[package identifiers](https://conda.org/learn/ceps/cep-0026/),
[MatchSpecs](https://conda.org/learn/ceps/cep-0029/),
[package metadata](https://conda.org/learn/ceps/cep-0034/), and
[repodata records](https://conda.org/learn/ceps/cep-0036/). No accepted conda
CEP currently defines an environment SBOM format or CRA profile.

The derived runtime lock does not currently retain the original requested
MatchSpecs. The root component therefore points to graph roots inferred from
the resolved package edges, plus a representative of any otherwise unreachable
dependency cycle. Every resolved package remains present even when that
inference cannot reproduce the user's declared top-level set.

When a package record names a dependency that is absent from the target
platform's resolved package set, the SBOM records the number of omitted edges
on the root and marks the affected package dependency sets as incomplete. It
does not invent a component without an exact package record.

For a product that falls within the EU
[Cyber Resilience Act](https://eur-lex.europa.eu/eli/reg/2024/2847/2024-11-20/eng),
this file can provide the resolved conda environment portion of its technical
documentation. The CRA requires a commonly used, machine-readable SBOM that
covers at least top-level dependencies. It does not mandate CycloneDX 1.7.
conda-ship chooses the
[CycloneDX 1.7 JSON schema](https://cyclonedx.org/docs/1.7/json/) and records the
resolved target-platform package graph to the extent that package records
permit.

The SBOM composition is deliberately marked `incomplete`. Conda package
records do not describe every operating-system component or every dependency
vendored or statically linked into a package. Rust crates linked into
`cs-template` are also outside the resolved conda graph. The broader package
metadata gap is discussed in
[conda/ceps#127](https://github.com/conda/ceps/issues/127). The generated file is
therefore a CRA-oriented inventory of the resolved conda environment, not proof
of complete product coverage or legal conformity.

Conda package PURLs follow the current
[package-url conda type](https://github.com/package-url/purl-spec/blob/main/docs/types/definitions/conda-definition.md).
License values are kept as named licenses because historical repodata does not
guarantee a valid SPDX expression.

conda-ship does not infer an SBOM author or product manufacturer from a channel
or package record. Compliance profiles that require author, manufacturer, or
contact metadata need additional information from the downstream project. Do
not edit the staged SBOM in place because its checksum is recorded in
`.info.json` and `.sha256`.

`SOURCE_DATE_EPOCH` controls the SBOM timestamp when set. Otherwise the build
time is recorded in UTC. Rebuild the SBOM whenever the runtime or package set
changes.

## Stamped Runtime Data

conda-ship stamps runtime data into every staged runtime. On macOS, it extends
the Mach-O `__LINKEDIT` segment over an appended block before native signing.
On Windows, it writes the footer into a read-only `.cship` PE section and
appends the JSON header and optional bundle. The data contains the runtime lock,
runtime and artifact identity, version, platform, delegate executable, install
scheme, install name, docs URL, installer, optional executable update
configuration, bundle and offline environment variable names, and the embedded
bundle bytes for `embedded` builds. The universal `CONDA_SHIP_PREFIX` override
is runtime behavior rather than a stamped variable name.

The format-specific footer records:

- format version
- header length
- bundle length
- header SHA256
- bundle SHA256, or the SHA256 of empty bytes when no embedded bundle is present
- conda-ship runtime-data magic bytes

The generated runtime validates the stamped header at startup. For embedded
artifacts, it copies the embedded bundle to a temporary snapshot while checking
its checksum, then extracts package archives only from that verified snapshot.

The binary checksum in `.sha256` covers the final stamped artifact. The
conda-ship release workflow also publishes GitHub Artifact Attestations for
the `cs` CLI, runtime templates, and `SHA256SUMS` manifest.

If signing or another downstream step changes the staged executable, the
original `.sha256` and the executable checksum inside `.info.json` continue to
describe the `cs build` output. Attest the finalized executable separately or
generate a downstream final manifest. Pass the finalized file to
`cs package-update --binary` when building an executable update package. The
command snapshots those bytes and reports the finalized payload digest in its
JSON output, but does not rewrite the original metadata files.

Verify assets published after the move to `conda-incubator` with:

```bash
gh attestation verify ./cs-x86_64-unknown-linux-gnu \
  -R conda-incubator/conda-ship \
  --signer-workflow conda-incubator/conda-ship/.github/workflows/release.yml
```

Releases through 0.9.0 were published by `jezdez/conda-ship`. Their signed
repository and workflow identities retain that name, and GitHub serves their
attestation bundles from the original owner's account. For example, verify
the 0.9.0 checksum manifest in an otherwise empty directory with:

```bash
gh release download 0.9.0 -R conda-incubator/conda-ship --pattern SHA256SUMS
gh attestation download ./SHA256SUMS --owner jezdez
gh attestation verify ./SHA256SUMS \
  --bundle ./*.jsonl \
  -R jezdez/conda-ship \
  --signer-workflow jezdez/conda-ship/.github/workflows/release.yml \
  --source-ref refs/tags/0.9.0
```

Use the same bundle-download and verification commands for each binary.
Change `--source-ref` to its exact release tag. The current composite action
handles the published pre-transfer tags explicitly. It does not accept the
original signer for new release tags.

Downstream distributions can add their own attestations or platform signing
after conda-ship finishes staging their runtime artifacts.

## Info JSON

The info JSON contains:

- schema version
- artifact stem, artifact name, and runtime name
- runtime version
- layout
- conda platform
- optional executable update configuration
- runtime filename
- optional external bundle filename
- lock filename
- package list filename
- SBOM filename
- package count
- SHA256 checksums

## Package List

The package list is tab-separated and contains:

- package name
- version
- build string
- package URL
- SHA256, when available from the lockfile

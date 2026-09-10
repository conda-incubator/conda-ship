# Trust And Provenance

conda-ship verifies build inputs and package archives. Downstream distributions
choose signing and release policy for their final artifacts.

## Build Tool Trust

The GitHub Action downloads conda-ship release assets:

- `cs-<target>`
- `cs-template-<target>`
- `SHA256SUMS`

It verifies GitHub artifact attestations for those files and checks the
published SHA256 sums before running `cs`.

This protects the builder path from accidentally executing an unverified
downloaded binary in CI.

Published conda-ship GitHub releases are immutable. The release workflow creates
a draft release, uploads the complete asset set, then verifies the assets and
composite action before publication. A failed check removes the draft. After
publication, the tag and assets are not replaced. If a published release is
wrong, the project should publish a new version instead of modifying the
existing one.

## Source Lock Trust

The source lockfile is committed project input. conda-ship assumes the
downstream project reviewed and committed that lockfile intentionally.

conda-ship does not solve loose matchspecs in the GitHub Action. That avoids a
release build changing package records because a channel changed between
workflow runs.

## Package Archive Trust

The runtime lock contains concrete package records. For bundle builds,
conda-ship requires SHA256 metadata so downloaded package archives can be
verified.

During bootstrap:

- online installs use the stamped runtime lock
- external bundle installs match local package archives to the lock
- embedded bundle installs verify the embedded bundle before extraction

The runtime rejects package archive mismatches instead of silently installing
unexpected files.

## Executable Update Trust

An update-enabled runtime resolves native `.conda` packages from its stamped
channel. Before staging an executable it verifies:

- the package size and SHA256 recorded in repodata
- package name, version, build number, platform, dependencies, and payload count
- the payload size and SHA256 recorded by the package
- the executable stamp, runtime and artifact identity, platform, and version
- update channel, package, and build number
- installed direct ownership before staging

The runtime accepts only a newer version or build number. It refuses a package
that rotates the stamped update source. Installed ownership and installation
kind remain in the prefix metadata and are not taken from candidate bytes.

These checks do not verify GitHub attestations, a provider-specific signature,
or an external package manager's signature. Downstream publication and signing
policy remains separate from the native update package format.

## Runtime Artifact Trust

Every staged build writes checksums and metadata:

- `.sha256`
- `.info.json`
- `.runtime.lock`
- `.packages.txt`
- `.cdx.json`

These files describe and verify what conda-ship produced. They are not a
replacement for signing.

The CycloneDX file inventories the resolved conda package graph. It is marked
incomplete because package records cannot reveal every operating-system,
vendored, or statically linked component. Downstream projects remain
responsible for evaluating the full product scope and for their CRA technical
documentation, access, publication, and retention policy.

## Downstream Signing And Attestation

The GitHub Action exposes `dist-path` so downstream workflows can attest the
exact conda-ship build output: the runtime binary, `.runtime.lock`,
`.packages.txt`, `.cdx.json`, `.info.json`, `.sha256`, and optional external
bundle. Verify `.sha256` before attesting that set.

Developer ID or Authenticode signing after the build changes the executable.
The conda-ship `.sha256` file and the binary checksum inside `.info.json` then
remain records of the earlier build output. Do not attest the post-sign
directory as one internally consistent conda-ship artifact set. Attest the
final executable separately, or have the downstream release workflow generate
and attest its own final manifest. For executable updates, sign the runtime
before running `cs package-update --binary`. That command snapshots and reports
the finalized executable bytes for the update package. It does not rewrite the
original build metadata.

Native macOS builds are ad hoc signed after conda-ship extends the Mach-O
`__LINKEDIT` segment over appended runtime data. Replace that temporary
signature with the downstream Developer ID signature. Cross-built macOS
artifacts remain unsigned until they are finalized on macOS. The temporary
signature does not preserve the identifier, designated requirements,
entitlements, library constraints, launch constraints, or hardened-runtime
flags from a custom input template. Apply the required policy during downstream
signing. Windows Authenticode signing must also run after stamping. The signed
PE `.cship` section contains the footer and its hashes of the appended JSON
header and optional bundle.

At runtime, conda-ship locates the Mach-O footer immediately before the code
signature. On PE, it reads the footer only from `.cship`, derives the appended
payload end from signed footer lengths, and requires any certificate table to
start at that offset. It does not search signature or certificate data.
Platform signature verification and runtime-data checksum verification remain
separate checks. Distribution workflows should require both.

The builder copies and stamps artifacts in a restricted staging directory on
the output filesystem, with mode `0700` on Unix. Native macOS builds also
receive a temporary ad hoc signature there. Hashing completes before
publication on every platform. Publication of one artifact stem is serialized.
The builder removes an old `.sha256` completion marker before it replaces
payload files, then publishes the new manifest last. Ordinary signing or rename
failures therefore cannot leave an old manifest beside a mixed set. This is not
a transaction across every sidecar file and it is not a power-loss durability
guarantee. Concurrent builds must use separate project roots because bundle and
lock intermediates are shared before publication. The output directory, its
parent chain, and the same operating-system account must remain trusted during
a build.

A custom runtime template is trusted executable input. For Mach-O and PE,
conda-ship requires an exact, versioned reader ABI declaration in a dedicated
linker-created image section and rejects unmodified templates from before the
signed-layout reader fix. The record is a self-declared compatibility check,
not proof of reader behavior or template provenance. Verify a downloaded
template against the matching conda-ship release attestation or checksum before
using it. A party able to replace a trusted template can forge the declaration
or replace the program it contains, regardless of the runtime-data format.

Good places for downstream release controls include:

- GitHub Release artifact attestations
- Sigstore signatures
- in-toto provenance for release workflows
- platform-specific installer signing
- package-manager-specific signatures or checksums

Signing belongs downstream because one runtime can be distributed through
several channels, and each channel has different trust requirements.
GitHub release immutability is useful downstream too, but it is not a
replacement for signing. It keeps a published asset set stable. Attestations and
signatures explain who produced that asset set and from which workflow.

## Authentication And Offline Updates

Stamped update channel URLs cannot contain credentials, a query, or a fragment.
HTTPS requests can read credentials from the explicit JSON file selected with
`RATTLER_AUTH_FILE`. This build does not enable keyring, netrc, or default
auth-file discovery. The runtime does not implement an interactive provider
login or call a provider API.

Offline mode can use a previously cached HTTPS channel only when both its
repodata and the selected package are already cached. A `file://` channel reads
repodata and packages directly and does not need a network cache. Cached data is
still checked against the same package and payload hashes.

## What conda-ship Does Not Promise

conda-ship does not:

- decide which channels are trusted for a downstream distribution
- apply a distribution identity or release signature to downstream runtime
  artifacts
- make a wrapper installer trustworthy by itself
- replace review of committed source lockfiles
- hide the need for package-manager or platform signing

It selects packages from a lockfile, checks package and runtime-data hashes,
and writes metadata that downstream release systems can sign. Set `SOURCE_DATE_EPOCH` when
reproducible SBOM timestamps are required.

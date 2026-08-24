# Verify Release Artifacts

Use this guide when you need to check conda-ship-built artifacts before
publishing or wrapping them.

Verification has two layers:

- verify the conda-ship tools used by the build
- verify the runtime artifacts produced by the build

## Verify conda-ship Release Tools In GitHub Actions

The composite action downloads `cs`, `cs-template`, and `SHA256SUMS`
from a tagged conda-ship release. It verifies GitHub artifact attestations and
then checks SHA256 sums before running `cs`.

Pin the action source by full commit SHA and pass the matching conda-ship
release version:

```yaml
- uses: jezdez/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
  with:
    conda-ship-version: "X.Y.Z"
```

When the action is invoked by an exact release tag, `conda-ship-version` can be
omitted for backwards compatibility. Release workflows should prefer full
commit SHA pins.

Self-hosted runners must provide the GitHub CLI because the action calls
`gh attestation verify`.

conda-ship releases are immutable after publication. If a released asset set is
wrong, use a newer release tag instead of expecting the existing tag or files to
change.

## Verify Staged Checksums

Every `cs build` writes a `.sha256` file next to the runtime and metadata:

```bash
shasum -a 256 --check dist/demo.sha256
```

On Linux, `sha256sum` works too:

```bash
sha256sum --check dist/demo.sha256
```

The checksum file covers the staged runtime, runtime lock, package list,
CycloneDX SBOM, info JSON, and external bundle when present.

## Inspect Artifact Metadata

Open the `.info.json` file:

```bash
python -m json.tool dist/demo.info.json
```

```{figure} ../../demos/verify.gif
:alt: Terminal recording of building release files, checking SHA256 sums, and inspecting artifact metadata.

Verify staged files and release metadata before publishing.
```

Check:

- `name`
- `layout`
- `platform`
- `binary`
- `bundle`
- `package_count`
- `checksums`

This file is intended for release tooling and package-manager wrappers. It
describes what conda-ship wrote, not what an external installer later did.

## Inspect The SBOM

Open the `.cdx.json` file:

```bash
python -m json.tool dist/demo.cdx.json
```

For strict schema validation, use the official CycloneDX CLI:

```bash
cyclonedx validate \
  --input-file dist/demo.cdx.json \
  --input-version v1_7 \
  --fail-on-errors
```

Check that the metadata component names the final runtime, every expected
conda package appears in `components`, and direct dependency relationships
appear in `dependencies`. The composition is intentionally `incomplete`
because conda metadata cannot account for every system, vendored, or statically
linked component.

## Inspect The Runtime Lock

The staged `.runtime.lock` is the lock the runtime will use during bootstrap.
It should be reproducible from the committed source lockfile and
`[tool.conda-ship]`.

Use it to answer release questions such as:

- Which concrete conda packages are shipped?
- Which channels are recorded?
- Which platforms are present?

Do not edit it by hand. Change the source manifest or source lockfile instead,
then rebuild.

## Verify Bundle Contents

For external bundles, extract into a temporary directory and check that it
contains only top-level package archives:

```bash
mkdir -p /tmp/demo-bundle
tar --zstd -xf dist/demo.bundle.tar.zst -C /tmp/demo-bundle
find /tmp/demo-bundle -maxdepth 2 -type f
```

The runtime verifies package archive hashes against the runtime lock before
installing. Embedded bundles are verified by the runtime before extraction.

## Preserve The Build Record Before Signing

conda-ship gives native macOS builds a temporary ad hoc signature so the
stamped Mach-O remains valid. It does not apply a downstream Developer ID or
Authenticode identity. The generated `.sha256` and the binary checksum in
`.info.json` describe the exact `cs build` output.

Verify that unchanged output first. In GitHub Actions, you can also attest the
complete pre-sign `dist-path`:

```{warning}
Use the latest reviewed `actions/attest` release in your workflow and pin it by
commit SHA. The SHA below is an example, not a recommendation to keep using that
exact revision indefinitely.
```

```yaml
permissions:
  contents: read
  id-token: write
  attestations: write
  artifact-metadata: write

steps:
  - uses: jezdez/conda-ship@FULL_RELEASE_COMMIT_SHA # X.Y.Z
    id: cs
    with:
      conda-ship-version: "X.Y.Z"

  - name: Attest unchanged conda-ship output
    uses: actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6 # v4.2.2
    with:
      subject-path: ${{ steps.cs.outputs.dist-path }}/*
```

That attests the runtime binary, `.runtime.lock`, `.packages.txt`, `.cdx.json`,
`.info.json`, `.sha256`, and the optional external bundle as the output of the
downstream workflow. Signing the runtime afterward changes its digest, so do
not present these sidecars as a checksum set for the signed executable.

## Sign And Verify A Finalized Runtime

Sign a copy so the original build record remains available. On macOS, replace
the ad hoc signature with the downstream identity and policy:

```bash
set -euo pipefail
: "${SIGNING_IDENTITY:?Set SIGNING_IDENTITY to the Developer ID identity}"
: "${APPLE_TEAM_ID:?Set APPLE_TEAM_ID to the expected Team ID}"
mkdir -p final
cp dist/demo final/demo
codesign --force --options runtime --timestamp \
  --sign "$SIGNING_IDENTITY" final/demo
developer_id_requirement="=anchor apple generic and "
developer_id_requirement+="certificate leaf[field.1.2.840.113635.100.6.1.13] exists and "
developer_id_requirement+="certificate leaf[subject.OU] = \"$APPLE_TEAM_ID\""
codesign --verify --deep --strict --verbose=4 \
  -R"$developer_id_requirement" final/demo
```

After notarization, assess the same final executable with Gatekeeper:

```bash
spctl --assess --type execute --verbose=4 final/demo
```

On Windows, sign the stamped executable, then require both SignTool and
PowerShell to report a valid Authenticode signature:

```powershell
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force final | Out-Null
Copy-Item dist\demo.exe final\demo.exe
signtool sign /fd SHA256 /td SHA256 /tr $env:TIMESTAMP_URL `
  /sha1 $env:SIGNING_CERTIFICATE_THUMBPRINT final\demo.exe
if ($LASTEXITCODE -ne 0) {
  throw "SignTool signing failed"
}
signtool verify /pa /v final\demo.exe
if ($LASTEXITCODE -ne 0) {
  throw "SignTool verification failed"
}
$signature = Get-AuthenticodeSignature -LiteralPath final\demo.exe
if ($signature.Status -ne "Valid") {
  throw "Authenticode verification failed: $($signature.StatusMessage)"
}
```

Run the downstream distribution's runtime smoke test against the finalized
file after native signature verification. Platform signature verification and
runtime-data checksum verification are separate requirements.

## Attest The Finalized Bytes

Generate a new checksum or manifest for the signed executable. Do not overwrite
the original conda-ship `.sha256` or `.info.json`. For example, on macOS:

```bash
(cd final && shasum -a 256 demo > SHA256SUMS)
```

Then attest the finalized executable and its new manifest as a separate output:

```yaml
- name: Attest finalized runtime
  uses: actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6 # v4.2.2
  with:
    subject-path: final/*
```

Verify the finalized executable against that workflow identity:

```bash
gh attestation verify final/demo \
  --repo OWNER/REPO \
  --signer-workflow OWNER/REPO/.github/workflows/release.yml
```

Good downstream controls include:

- GitHub Release artifact attestations
- GitHub release immutability
- Sigstore signing for uploaded artifacts
- in-toto provenance around the packaging workflow
- platform-specific signing for installer wrappers

Keep signing outside the generic runtime. A runtime built by conda-ship may be
wrapped by several downstream channels, and each channel owns its own trust
policy.

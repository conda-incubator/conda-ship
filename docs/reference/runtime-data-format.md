# Runtime Data Format

conda-ship stamps runtime data onto a copy of the generic runtime template.
This page describes the stored data and how runtimes read it. Only conda-ship
should write this format.

```{important}
Treat the stamped runtime data as private executable metadata. Release tooling
should read `.info.json`, `.runtime.lock`, `.packages.txt`, `.cdx.json`, and
`.sha256` instead of parsing or writing bytes inside the runtime binary. Fleet
callers adapting a selected stamped artifact should use
`RuntimeSpec::from_stamped_artifact`.
```

## Location

Runtime data contains a JSON header, an optional compressed bundle, and a
100-byte version-one footer. Its physical layout depends on the executable
format.

For signed macOS templates, conda-ship removes the existing
`LC_CODE_SIGNATURE` command and its signature bytes, then appends the runtime
data at that location. For unsigned templates, it verifies that the Mach-O
header already has room for the signer command and appends at the end of the
file. Both paths extend the Mach-O `__LINKEDIT` segment over the block. A native
macOS build is then ad hoc signed, which adds `LC_CODE_SIGNATURE`. Downstream
Developer ID signing replaces that temporary signature. A macOS artifact
cross-built on another operating system remains unsigned until it is finalized
on macOS.

On PE, conda-ship adds one read-only initialized-data section named `.cship`.
The footer occupies its first 100 bytes and the remaining raw section bytes are
zero. The JSON header and optional bundle follow the final section, with zero
padding before them so their derived end is eight-byte aligned. The footer
lengths and checksums therefore provide a signed anchor to that appended
payload even when an Authenticode implementation does not hash bytes past the
last declared section.

Windows templates must be unsigned and have no existing overlay when stamped.
Downstream release workflows apply any required Authenticode identity after
stamping. Unsigned local builds remain structurally readable. For an unsigned
stamped PE, the derived payload end is physical EOF. For a signed PE, the
Security Directory certificate offset must equal that same signed-derived end,
and the structurally valid certificate table must end at physical EOF.

The reader selects exactly one platform-defined footer. On Mach-O, it uses
`LC_CODE_SIGNATURE.dataoff` when a signature exists and permits only the
zero-filled alignment gap immediately before it. On PE, it reads only the
footer in the canonical final `.cship` section. Signature contents,
certificate-table contents, and other trailing regions are never searched for
a footer. On other executable formats, the footer must end at physical EOF.

This makes the selected footer directly covered by platform signing. On PE,
the signed footer transitively authenticates the appended header and bundle
through their SHA256 values. A second checksum-valid stamp in mutable signature
or certificate padding cannot replace it. Malformed platform structures are
rejected rather than falling back to a different footer.

## Header Fields

The stamped header records:

`schema_version`
: Runtime data schema version.

`artifact_name`
: Staged executable and artifact name.

`runtime_name`
: Base runtime identity. This is the value from `runtime-name`, independent of
  an optional `artifact-name`.

`runtime_version`
: Version written to runtime and prefix ownership metadata. This is independent
  from `runtime-name`. See {doc}`names`.

`artifact_layout`
: Staged artifact layout. Executable updates support `online` and `embedded`.

`platform`
: Native conda platform for the staged executable and any runtime update
  package.

`embedded_artifact_name`
: Artifact executable name used when the artifact carries an embedded bundle.
  This is explicit build metadata, not a derived suffix.

`delegate_executable`
: Executable inside the managed prefix that receives every runtime argument.

`install_scheme`
: Stamped install scheme, such as `conda-home` or `user-data`.

`install_name`
: Name used inside the install scheme.

`metadata_file`
: Ownership metadata filename written inside the managed prefix.

`bundle_env_var`
: Runtime-specific environment variable for an external bundle path.

`offline_env_var`
: Runtime-specific environment variable for offline bootstrap mode.

`docs_url`
: Documentation URL retained in stamped runtime metadata.

`installer`
: Optional package manager or installer metadata.

`update`
: Optional executable update configuration. It contains:

  - `channel`: absolute `https://` or `file://` conda channel URL
  - `package`: conda package used for update records
  - `build-number`: current executable build number

  Installed ownership is recorded in `.RUNTIME_NAME.json`, together with the
  stable executable path, installation kind, and optional external update
  instruction. Existing 0.6.x stamps containing `ownership` or `instruction`
  remain readable, but new builds do not write those fields.

`runtime_config`
: Resolved runtime channels and package names used for bootstrap metadata, plus
  optional stamped condarc text and the frozen-base policy.

`runtime_lock`
: Runtime lock used for bootstrap.

## Footer

The footer contains enough information for the runtime to find and verify the
runtime data:

- header length
- bundle length
- header SHA256
- bundle SHA256
- format version
- conda-ship magic bytes

If the footer or checksums are invalid, the runtime refuses to start.

## Compatibility Notes

Generated runtimes are expected to read the format written by the same
conda-ship release family. Downstream tools should treat the staged runtime as
an opaque executable plus documented artifact metadata files.

Starting with 0.9.0, builders require Mach-O and PE templates with a versioned
signed-layout reader ABI declaration emitted by the matching `cs-template`
build. Templates from 0.8.0 and earlier do not contain this declaration and are
rejected. Mach-O stores the exact record in the dedicated
`__TEXT,__cship_reader` section. PE stores it in the dedicated read-only
`.cscap` section. The builder rejects missing, duplicate, misplaced, malformed,
or unknown records. Linux templates do not use this declaration.

Existing artifacts retain their legacy reader, and 0.9.0 does not repair their
signing defect. Rebuild macOS and Windows runtimes with a matching 0.9.0 builder
and template, then apply the downstream platform signature. Re-signing an older
stamped macOS artifact does not repair its invalid native layout, and signing an
older Windows overlay does not cover its runtime data.

The capability record is a self-declared compatibility signal. It does not
prove the behavior or provenance of the executable that contains it. A party
that can modify a custom template can transplant or forge the declaration and
can also replace the template program itself. Treat the template as trusted
executable input and verify its release attestation or checksum before use.
Mach-O stamping also accepts only the bounded modern load-command shape used by
`cs-template`, validates its macOS target and entry point, and proves that every
supported link-edit range ends before the removed code signature. Unknown load
commands fail closed.

The reader retains compatibility with unsigned version-one PE artifacts whose
footer ends at EOF. It deliberately rejects signed legacy PE overlays because
their footer is outside declared sections and has no authenticated selection
anchor.

A Windows runtime built with 0.8.0 or earlier expects the legacy trailing
footer and cannot validate a 0.9.0 executable update candidate. An adopting
installer or package manager must record external ownership during replacement,
before the new executable's first normal invocation. An installation that must
remain directly managed needs a fresh install that does not reuse its old
direct-install metadata. After a 0.9.0 runtime is installed, publish update
packages made from finalized 0.9.0 artifacts for later native updates.

Use `.info.json`, `.runtime.lock`, `.packages.txt`, `.cdx.json`, and `.sha256`
for release automation instead of parsing the runtime data block directly.

The version-one update coordinator API does not expose the stored runtime
format. A coordinator invokes the stamped executable as a child process
and exchanges JSON through the environment-driven helper documented in
{doc}`runtime-cli`.

After bootstrap, executable update and recovery state is stored in the existing
`.RUNTIME_NAME.json` prefix metadata file. The update engine does not add a
second persistent receipt or state record.

# Errors

This page lists common conda-ship errors and the usual fix.

`cs` prints human-readable diagnostics by default. The `conda ship` adapter uses
an internal structured diagnostic mode so it can show the same errors through
conda without depending on terminal formatting.

## Project Input

`could not find project root containing conda.toml, pixi.toml, or supported pyproject.toml`
: Run from the project root or pass `--root PATH`.

`could not find conda.toml, pixi.toml, or supported pyproject.toml`
: Add a supported manifest to the selected root.

`lockfile not found`
: Refresh and commit the matching source lockfile with `conda workspace lock`
  or `pixi lock`.

`source environment is required`
: Set `[tool.conda-ship].source-environment`.

`source environment "NAME" not found`
: Add the environment to the source manifest and refresh the source lockfile.

`runtime version is required`
: Pass `--runtime-version`, set `[tool.conda-ship].runtime-version`, define
  static `[project].version` in the selected `pyproject.toml`, or opt into
  `runtime-version = { from = "project-metadata" }` through the Python
  `conda ship` adapter for dynamic Python project versions.

`failed to read condarc-file`
: Check `[tool.conda-ship].condarc-file`. Relative paths start from the selected
  manifest directory.

`failed to parse condarc-file as YAML`
: Fix the referenced YAML file.

`condarc-file must contain a YAML mapping`
: Use normal condarc key and value entries rather than a top-level scalar or
  sequence.

## Package Archives

`cannot bundle packages without SHA256 hashes`
: Refresh the source lockfile with package hash metadata before building
  `external` or `embedded` layouts.

`no default environment in ... runtime.lock`
: The derived runtime lock is malformed. Rebuild from the source lockfile.

## Template Selection

`runtime template not found`
: Install a conda-ship package that includes `cs-template`, set
  `CONDA_SHIP_TEMPLATE`, or pass `--template PATH`.

`cross-builds require --template`
: Pass a prebuilt runtime template for the requested target.

`runtime template is already stamped`
: `--template` points at a generated runtime instead of a generic template.

`runtime template is incompatible with authenticated runtime data`
: Mach-O and PE templates must contain the exact reader ABI declaration from
  the current format implementation in its dedicated image section. Use
  `cs-template` from the same conda-ship release as the builder. Unmodified
  pre-fix templates are rejected even when their executable layout could
  otherwise be stamped. The declaration is not proof of template provenance.

`universal Mach-O binaries are not supported`
: Use a thin macOS template for the selected target. The builder also rejects
  32-bit and big-endian Mach-O templates.

`Mach-O runtime template is not an executable image`
: Use a thin `MH_EXECUTE` template, not a dynamic library or another Mach-O
  file type.

`Mach-O runtime template CPU type ... with subtype ... does not match platform ...`
: Use an x86_64 template for `osx-64` and an arm64 template for `osx-arm64`.

`unsupported Mach-O load command: ...`
: Rebuild with the matching `cs-template`. conda-ship accepts the bounded set of
  modern load commands used by its runtime templates and fails closed on
  unknown commands that might reference removed signature bytes.

`Mach-O runtime template targets unsupported platform ...`
: Use a macOS template. iOS, Catalyst, simulator, and other Apple platform
  binaries are not accepted.

`Mach-O code signature overlaps referenced file data`
: Rebuild the template with a linker-generated signature after all link-edit
  tables. conda-ship will not truncate data referenced by another load command.

`Mach-O LC_MAIN entry point is outside executable file data`
: Rebuild the template as a normal modern macOS executable with one `LC_MAIN`
  entry point inside an executable segment.

`Mach-O header has no room for a code signature command`
: An unsigned custom template needs at least 16 bytes between its load-command
  table and first file-backed section so native `codesign` can add
  `LC_CODE_SIGNATURE`. Use a linker-generated signature or reserve that space.

`Mach-O code signature is not the final data in the file`
: Rebuild the custom template so its linker-generated signature is the final
  file data before conda-ship stamps it.

`Mach-O load command table is too large`
: Rebuild the custom template with a normal thin Mach-O load-command table.
  conda-ship rejects tables larger than 1 MiB before allocating memory for
  them.

`PE certificate table is not the final data in the file`
: Finalize the PE so its Security Directory certificate range ends at the
  physical end of the executable. Do not append data after Authenticode
  signing.

`signed PE templates must be unsigned before runtime stamping`
: Remove the existing Authenticode signature or rebuild an unsigned template.
  Sign the generated runtime after conda-ship adds its `.cship` anchor.

`PE runtime template has data outside its declared sections`
: Rebuild the template without an existing overlay. conda-ship creates the
  runtime overlay itself and binds it to the signed `.cship` footer.

`PE headers have no room for a runtime anchor section`
: Rebuild the template with at least one zero-filled 40-byte section-header slot
  before the first raw section.

`PE runtime template machine ... with image kind ... does not match platform ...`
: Use a PE32 x86 template for `win-32`, a PE32+ x86_64 template for `win-64`,
  and a PE32+ ARM64 template for `win-arm64`.

## Naming

`runtime name must start with an ASCII letter or digit`
: Use a filename-safe runtime name such as `demo` or `demo-runtime`.

`delegate executable may only contain ASCII letters, digits, dots, dashes, and underscores`
: Use an executable name, not a path.

`target triple may only contain ASCII letters, digits, dots, dashes, and underscores`
: Use a target triple string, not a path to a custom target file.

`installer may only contain ASCII letters, digits, dots, dashes, and underscores`
: Use a short installer name such as `homebrew`, `conda-package`, or `standalone`.

## Runtime Bootstrap

`runtime template, not a runnable runtime`
: Run a binary produced by `cs build`, not the generic runtime template.

`runtime has no stamped lockfile`
: The binary is not a properly stamped runtime. Rebuild it with `cs build`.

`offline bootstrap requires a stamped runtime lock`
: Offline bootstrap requires a runtime built by `cs build`.

`runtime bundle path is not a directory`
: Point the runtime-specific `_BUNDLE` environment variable at a directory
  containing package archive files, not the compressed `.bundle.tar.zst` file
  itself.

## Prefix Ownership

`refusing to bootstrap into existing non-empty path`
: Set `CONDA_SHIP_PREFIX` to another path or remove the existing directory
  yourself. Runtime-specific `_PREFIX` variables also work for names other than
  `conda`.

`refusing to use unmanaged install path`
: The prefix does not contain ownership metadata for this runtime.

`refusing to use install path with invalid bootstrap state`
: The internal installing marker is malformed or belongs to another runtime.
  A non-empty prefix without matching ownership state is rejected.

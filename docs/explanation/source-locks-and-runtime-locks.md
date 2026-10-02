# Source Locks And Runtime Locks

conda-ship uses two kinds of lockfiles. They have different owners and
different jobs.

## Source Lock

A source lock is the lockfile owned by the project environment tool:

- `conda.lock` for conda-workspaces input
- `pixi.lock` for Pixi input

It is committed project input. It records solved environments for the project
and can contain more than the runtime should ship: development environments,
test environments, multiple features, and package records for several platforms.

conda-ship does not replace the solver that created this lockfile. It reads the
lockfile after conda-workspaces or Pixi has solved it.

## Runtime Lock

A runtime lock is build output derived by conda-ship.

conda-ship reads the selected source environment:

```toml
[tool.conda-ship]
source-environment = "ship"
```

Then it:

1. selects that solved environment from the source lock and rejects locked
   PyPI packages
2. copies the concrete conda package records into a new lock
3. applies `[tool.conda-ship].exclude-packages`
4. validates the required runtime packages
5. stamps the derived lock into the generated runtime binary
6. writes `dist/RUNTIME.runtime.lock`
7. writes `dist/RUNTIME.cdx.json` from the target platform's resolved package
   graph

conda-ship installs conda packages during bootstrap. It rejects PyPI packages in
the selected environment instead of producing an incomplete runtime lock.
PyPI packages in unselected environments do not prevent a build.

The runtime lock is the lock the generated runtime uses during bootstrap.

## Why The Split Exists

The source lock answers:

- What did the project solve?
- Which environments does the project maintain?
- Which packages are available to development and release workflows?

The runtime lock answers:

- What will this runtime install into its managed prefix?
- Which package records should be verified during bootstrap?
- Which channels and packages were selected for the runtime artifact?

Keeping them separate lets a downstream project maintain normal workspace input
while shipping only the selected runtime environment.

## Reproducibility

The runtime lock should be reproducible from:

- the committed source manifest
- the committed source lockfile
- `[tool.conda-ship]`
- the `cs build` inputs used for that build

Do not edit a staged `.runtime.lock` by hand. If a package changes, update the
source manifest or source lockfile, then rebuild.

## Flow

```text
conda.toml / pixi.toml / pyproject.toml
        |
        | solved by conda-workspaces or Pixi
        v
conda.lock / pixi.lock          source lock
        |
        | read by conda-ship
        v
selected source-environment
        |
        | filtered, validated, and stamped
        v
dist/demo + dist/demo.runtime.lock
        |
        | component graph documented as dist/demo.cdx.json
        v
first demo invocation installs from that lock, then runs the delegate
```

"""Resolve Python project metadata for the conda-ship adapter."""

from __future__ import annotations

import tempfile
from dataclasses import dataclass
from email.parser import Parser
from pathlib import Path
from typing import TYPE_CHECKING

from pyproject_hooks import BuildBackendHookCaller, quiet_subprocess_runner

if TYPE_CHECKING:
    from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib


class ProjectMetadataError(Exception):
    """User-facing error while resolving Python project metadata."""


PROJECT_METADATA_SOURCE = "project-metadata"


@dataclass(frozen=True)
class ShipCommand:
    """Parsed ``cs build`` or ``cs run`` arguments."""

    argv: list[str]
    name: str
    options: list[str]
    passthrough_index: int | None

    @classmethod
    def parse(cls, argv: list[str]) -> ShipCommand | None:
        """Return a command when ``argv`` may need runtime version resolution."""
        if not argv or argv[0] not in {"build", "run"}:
            return None

        try:
            passthrough_index = argv.index("--", 1)
        except ValueError:
            options = argv[1:]
            passthrough_index = None
        else:
            options = argv[1:passthrough_index]

        return cls(
            argv=argv,
            name=argv[0],
            options=options,
            passthrough_index=passthrough_index,
        )

    @property
    def accepts_adapter_resolution(self) -> bool:
        """Return whether the adapter should inspect project metadata config."""
        return not (
            self.has_option("-h")
            or self.has_option("--help")
            or self.has_option("--runtime-version")
        )

    def has_option(self, option: str) -> bool:
        """Return whether command options include ``--x`` or ``--x=value``."""
        return any(arg == option or arg.startswith(f"{option}=") for arg in self.options)

    def root_override(self, cwd: Path) -> Path | None:
        """Return the configured project root from ``--root`` if present."""
        for index, arg in enumerate(self.options):
            if arg.startswith("--root="):
                return cli_path(arg.partition("=")[2], cwd)
            if arg == "--root" and index + 1 < len(self.options):
                return cli_path(self.options[index + 1], cwd)
        return None

    def with_runtime_version(self, version: str) -> list[str]:
        """Return ``argv`` with ``--runtime-version`` before run pass-through args."""
        insert_at = (
            self.passthrough_index
            if self.name == "run" and self.passthrough_index is not None
            else len(self.argv)
        )
        return [*self.argv[:insert_at], "--runtime-version", version, *self.argv[insert_at:]]


@dataclass(frozen=True)
class CondaShipProject:
    """A discovered project root plus its selected conda-ship manifest."""

    root: Path
    manifest_path: Path
    manifest_data: dict[str, Any]

    @classmethod
    def from_command(
        cls,
        command: ShipCommand,
        *,
        cwd: Path | None = None,
    ) -> CondaShipProject | None:
        """Discover the project selected by a command."""
        cwd = Path.cwd() if cwd is None else cwd
        root = command.root_override(cwd)
        if root is not None:
            return cls.from_root(root)
        return cls.discover(cwd)

    @classmethod
    def discover(cls, start: Path) -> CondaShipProject | None:
        """Find the nearest parent with a conda-ship-supported manifest."""
        for root in (start, *start.parents):
            if project := cls.from_root(root):
                return project
        return None

    @classmethod
    def from_root(cls, root: Path) -> CondaShipProject | None:
        """Discover the selected manifest using the same precedence as ``cs``."""
        for manifest_path in (root / "conda.toml", root / "pixi.toml"):
            if manifest_path.exists():
                return cls(
                    root=root,
                    manifest_path=manifest_path,
                    manifest_data=read_toml(manifest_path),
                )

        pyproject_path = root / "pyproject.toml"
        if not pyproject_path.exists():
            return None

        data = read_toml(pyproject_path)
        if not cls.supports_pyproject(data):
            return None
        return cls(root=root, manifest_path=pyproject_path, manifest_data=data)

    @staticmethod
    def supports_pyproject(data: dict[str, Any]) -> bool:
        """Return whether ``pyproject.toml`` contains a supported workspace."""
        tool = data.get("tool", {})
        if not isinstance(tool, dict):
            return False
        for name in ("conda", "pixi"):
            config = tool.get(name)
            if not isinstance(config, dict):
                continue
            workspace = config.get("workspace")
            if isinstance(workspace, dict) and workspace:
                return True
        return False

    @property
    def conda_ship_config(self) -> dict[str, Any]:
        """Return the selected manifest's ``[tool.conda-ship]`` table."""
        tool = self.manifest_data.get("tool", {})
        if not isinstance(tool, dict):
            return {}
        config = tool.get("conda-ship", {})
        return config if isinstance(config, dict) else {}

    @property
    def uses_project_metadata_version(self) -> bool:
        """Return whether runtime-version requests Python project metadata."""
        runtime_version = self.conda_ship_config.get("runtime-version")
        return (
            isinstance(runtime_version, dict)
            and runtime_version.get("from") == PROJECT_METADATA_SOURCE
        )


@dataclass(frozen=True)
class PythonBuildSystem:
    """PEP 517 build backend configuration from ``pyproject.toml``."""

    backend: str
    backend_path: list[str] | None

    @classmethod
    def from_pyproject(cls, pyproject: Path) -> PythonBuildSystem:
        """Read and validate ``[build-system]`` hook configuration."""
        data = read_toml(pyproject)
        build_system = data.get("build-system", {})
        if not isinstance(build_system, dict):
            build_system = {}

        backend = build_system.get("build-backend", "setuptools.build_meta:__legacy__")
        backend_path = build_system.get("backend-path", [])
        if not isinstance(backend, str):
            raise ProjectMetadataError("[build-system].build-backend must be a string")
        if not isinstance(backend_path, list) or not all(
            isinstance(entry, str) for entry in backend_path
        ):
            raise ProjectMetadataError("[build-system].backend-path must be a list of strings")

        return cls(
            backend=backend,
            backend_path=backend_path or None,
        )

    def hook_caller(self, root: Path) -> BuildBackendHookCaller:
        """Create the PEP 517 hook caller for this backend."""
        return BuildBackendHookCaller(
            str(root),
            self.backend,
            backend_path=self.backend_path,
            runner=quiet_subprocess_runner,
        )


def runtime_version_args(argv: list[str], *, cwd: Path | None = None) -> list[str]:
    """Return ``argv`` with a resolved ``--runtime-version`` when configured."""
    command = ShipCommand.parse(argv)
    if command is None or not command.accepts_adapter_resolution:
        return argv

    project = CondaShipProject.from_command(command, cwd=cwd)
    if project is None or not project.uses_project_metadata_version:
        return argv

    return command.with_runtime_version(resolve_project_metadata_version(project.root))


def cli_path(value: str, cwd: Path) -> Path:
    """Resolve a CLI path the same way users expect from the current directory."""
    path = Path(value)
    if path.is_absolute():
        return path
    return cwd / path


def resolve_project_metadata_version(root: Path) -> str:
    """Resolve the project version with the PEP 517 metadata hook."""
    pyproject = root / "pyproject.toml"
    if not pyproject.exists():
        raise ProjectMetadataError(
            "runtime-version requested project metadata, but pyproject.toml was not found"
        )

    caller = PythonBuildSystem.from_pyproject(pyproject).hook_caller(root)
    with tempfile.TemporaryDirectory() as metadata_dir:
        try:
            dist_info = caller.prepare_metadata_for_build_wheel(
                metadata_dir,
                _allow_fallback=False,
            )
        except Exception as error:
            raise ProjectMetadataError(
                f"PEP 517 prepare_metadata_for_build_wheel failed: {error}"
            ) from error

        if not dist_info or "/" in dist_info or "\\" in dist_info:
            raise ProjectMetadataError(
                f"PEP 517 metadata hook returned invalid dist-info directory: {dist_info!r}"
            )
        metadata_path = Path(metadata_dir, dist_info, "METADATA")
        try:
            metadata = metadata_path.read_text(encoding="utf-8")
        except OSError as error:
            raise ProjectMetadataError(f"failed to read {metadata_path}: {error}") from error

    version = metadata_version(metadata)
    if version is None:
        raise ProjectMetadataError("project metadata does not contain a Version field")
    return version


def read_toml(path: Path) -> dict[str, Any]:
    """Read a TOML document."""
    with path.open("rb") as file:
        data = tomllib.load(file)
    return data if isinstance(data, dict) else {}


def metadata_version(metadata: str) -> str | None:
    """Read ``Version`` from wheel metadata."""
    message = Parser().parsestr(metadata, headersonly=True)
    version = message.get("Version")
    if version is None:
        return None
    version = version.strip()
    return version or None

from __future__ import annotations

import subprocess
import sys
import textwrap

import pytest
from conda_ship import github_action
from conda_ship.project_metadata import ProjectMetadataError


def test_resolve_runtime_version_ignores_static_version(tmp_path) -> None:
    write_project(tmp_path, runtime_version='"1.2.3"')

    assert github_action.resolve_runtime_version(tmp_path) is None


def test_resolve_runtime_version_reads_project_metadata(tmp_path) -> None:
    write_project(tmp_path)
    write_backend(tmp_path, version="2.3.4")

    assert github_action.resolve_runtime_version(tmp_path) == "2.3.4"


@pytest.mark.parametrize("configured", [False, True])
def test_github_action_module_prints_only_requested_version(tmp_path, configured) -> None:
    if configured:
        write_project(tmp_path)
        write_backend(tmp_path, version="2.3.4")

    result = subprocess.run(
        [sys.executable, "-m", "conda_ship.github_action", str(tmp_path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
    )

    assert result.returncode == 0
    assert result.stdout == ("2.3.4\n" if configured else "")
    assert result.stderr == ""


def test_github_action_module_reports_missing_pyproject(tmp_path) -> None:
    write_project(tmp_path)
    (tmp_path / "pyproject.toml").unlink()

    result = subprocess.run(
        [sys.executable, "-m", "conda_ship.github_action", str(tmp_path)],
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
    )

    assert result.returncode == 1
    assert result.stdout == ""
    assert result.stderr == (
        "conda-ship: runtime-version requested project metadata, "
        "but pyproject.toml was not found\n"
    )


def test_resolve_runtime_version_reports_backend_failure(tmp_path) -> None:
    write_project(tmp_path)
    write_backend(tmp_path, version="2.3.4")
    (tmp_path / "backend" / "demo_backend.py").write_text(
        "def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):\n"
        "    raise RuntimeError('broken backend')\n",
        encoding="utf-8",
    )

    with pytest.raises(ProjectMetadataError, match="failed to resolve project metadata version"):
        github_action.resolve_runtime_version(tmp_path)


def test_resolve_runtime_version_requires_metadata_version(tmp_path) -> None:
    write_project(tmp_path)
    write_backend(tmp_path, version=None)

    with pytest.raises(ProjectMetadataError, match="project metadata does not contain a Version"):
        github_action.resolve_runtime_version(tmp_path)


def write_project(tmp_path, *, runtime_version: str = '{ from = "project-metadata" }') -> None:
    (tmp_path / "conda.toml").write_text(
        textwrap.dedent(
            f"""
            [tool.conda-ship]
            runtime-version = {runtime_version}
            """
        ),
        encoding="utf-8",
    )
    (tmp_path / "pyproject.toml").write_text(
        textwrap.dedent(
            """
            [project]
            name = "demo"
            dynamic = ["version"]

            [build-system]
            requires = []
            build-backend = "demo_backend"
            backend-path = ["backend"]
            """
        ),
        encoding="utf-8",
    )


def write_backend(tmp_path, *, version: str | None) -> None:
    backend_dir = tmp_path / "backend"
    backend_dir.mkdir()
    metadata = "Metadata-Version: 2.4\nName: demo\n"
    if version is not None:
        metadata += f"Version: {version}\n"
    (backend_dir / "demo_backend.py").write_text(
        textwrap.dedent(
            f"""
            import os


            def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):
                dist_info = "demo.dist-info"
                path = os.path.join(metadata_directory, dist_info)
                os.makedirs(path)
                with open(os.path.join(path, "METADATA"), "w", encoding="utf-8") as metadata:
                    metadata.write({metadata!r})
                return dist_info
            """
        ),
        encoding="utf-8",
    )

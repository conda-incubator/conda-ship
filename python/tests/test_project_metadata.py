from __future__ import annotations

import textwrap

import pytest
from conda_ship import project_metadata


def test_runtime_version_args_ignores_non_build_commands(tmp_path) -> None:
    assert project_metadata.runtime_version_args(["inspect"], cwd=tmp_path) == ["inspect"]


def test_runtime_version_args_ignores_cli_version(tmp_path) -> None:
    write_project(tmp_path)

    assert project_metadata.runtime_version_args(
        ["build", "--runtime-version", "1.2.3"],
        cwd=tmp_path,
    ) == ["build", "--runtime-version", "1.2.3"]


def test_runtime_version_args_appends_build_version(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    write_project(tmp_path)
    monkeypatch.setattr(
        project_metadata,
        "resolve_project_metadata_version",
        lambda root: "2.3.4",
    )

    assert project_metadata.runtime_version_args(["build"], cwd=tmp_path) == [
        "build",
        "--runtime-version",
        "2.3.4",
    ]


def test_runtime_version_args_inserts_run_version_before_separator(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    write_project(tmp_path)
    monkeypatch.setattr(
        project_metadata,
        "resolve_project_metadata_version",
        lambda root: "2.3.4",
    )

    assert project_metadata.runtime_version_args(["run", "--", "--version"], cwd=tmp_path) == [
        "run",
        "--runtime-version",
        "2.3.4",
        "--",
        "--version",
    ]


def test_runtime_version_args_uses_root_override(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    write_project(project)
    seen = []

    def fake_resolve(root):
        seen.append(root)
        return "2.3.4"

    monkeypatch.setattr(project_metadata, "resolve_project_metadata_version", fake_resolve)

    assert project_metadata.runtime_version_args(
        ["build", "--root", str(project)],
        cwd=tmp_path,
    ) == ["build", "--root", str(project), "--runtime-version", "2.3.4"]
    assert seen == [project]


def test_runtime_version_args_ignores_static_version(tmp_path) -> None:
    write_project(tmp_path, runtime_version='"1.2.3"')

    assert project_metadata.runtime_version_args(["build"], cwd=tmp_path) == ["build"]


def test_project_discovery_prefers_conda_toml(tmp_path) -> None:
    (tmp_path / "conda.toml").write_text("", encoding="utf-8")
    (tmp_path / "pixi.toml").write_text("", encoding="utf-8")

    project = project_metadata.CondaShipProject.from_root(tmp_path)

    assert project is not None
    assert project.manifest_path == tmp_path / "conda.toml"


@pytest.mark.parametrize("namespace", ["conda", "pixi"])
def test_project_discovery_accepts_embedded_workspace(tmp_path, namespace) -> None:
    (tmp_path / "pyproject.toml").write_text(
        f'[tool.{namespace}.workspace]\nname = "demo"\n', encoding="utf-8"
    )

    project = project_metadata.CondaShipProject.discover(tmp_path)

    assert project is not None
    assert project.manifest_path == tmp_path / "pyproject.toml"


@pytest.mark.parametrize(
    "config",
    [
        "[tool.conda]\n",
        "[tool.conda.workspace]\n",
        '[tool.conda.dependencies]\npython = "*"\n',
        '[tool.conda]\nworkspace = "invalid"\n',
        "[tool.pixi]\n",
        "[tool.pixi.workspace]\n",
        '[tool.pixi.dependencies]\npython = "*"\n',
        '[tool.pixi]\nworkspace = "invalid"\n',
    ],
)
def test_project_discovery_skips_pyproject_without_workspace(tmp_path, config) -> None:
    (tmp_path / "conda.toml").write_text("", encoding="utf-8")
    nested = tmp_path / "nested"
    nested.mkdir()
    (nested / "pyproject.toml").write_text(config, encoding="utf-8")

    project = project_metadata.CondaShipProject.discover(nested)

    assert project is not None
    assert project.manifest_path == tmp_path / "conda.toml"


def test_metadata_version_reads_version_header() -> None:
    assert (
        project_metadata.metadata_version("Metadata-Version: 2.4\nName: demo\nVersion: 1.2.3\n\n")
        == "1.2.3"
    )
    assert project_metadata.metadata_version("Metadata-Version: 2.4\nName: demo\n\n") is None


def test_resolve_project_metadata_version_uses_pep517_hook(tmp_path) -> None:
    backend_dir = tmp_path / "backend"
    backend_dir.mkdir()
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
    (backend_dir / "demo_backend.py").write_text(
        textwrap.dedent(
            """
            import os


            def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):
                dist_info = "demo-2.3.4.dist-info"
                path = os.path.join(metadata_directory, dist_info)
                os.makedirs(path)
                with open(os.path.join(path, "METADATA"), "w", encoding="utf-8") as metadata:
                    metadata.write("Metadata-Version: 2.4\\nName: demo\\nVersion: 2.3.4\\n")
                return dist_info
            """
        ),
        encoding="utf-8",
    )

    assert project_metadata.resolve_project_metadata_version(tmp_path) == "2.3.4"


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
            """
        ),
        encoding="utf-8",
    )

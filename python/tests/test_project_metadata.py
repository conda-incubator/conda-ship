from __future__ import annotations

import textwrap

import pytest
from conda_ship import project_metadata


def test_runtime_version_args_ignores_non_build_commands(tmp_path) -> None:
    assert project_metadata.runtime_version_args(["inspect"], cwd=tmp_path) == ["inspect"]


@pytest.mark.parametrize(
    "argv",
    [
        ["build", "--runtime-version", "1.2.3"],
        ["build", "--runtime-version=1.2.3"],
        ["build", "--help"],
        ["run", "-h"],
    ],
)
def test_runtime_version_args_ignores_cli_version_and_help(tmp_path, argv) -> None:
    write_project(tmp_path)

    assert project_metadata.runtime_version_args(argv, cwd=tmp_path) == argv


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


@pytest.mark.parametrize(
    "root_args", [["--root", "project"], ["--root=project"], ["--root", "{project}"]]
)
def test_runtime_version_args_uses_root_override(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    root_args,
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    write_project(project)
    root_args = [arg.format(project=project) for arg in root_args]
    seen = []

    def fake_resolve(root):
        seen.append(root)
        return "2.3.4"

    monkeypatch.setattr(project_metadata, "resolve_project_metadata_version", fake_resolve)

    assert project_metadata.runtime_version_args(
        ["build", *root_args],
        cwd=tmp_path,
    ) == ["build", *root_args, "--runtime-version", "2.3.4"]
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
    assert project_metadata.metadata_version("Version:   \n\n") is None


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


def test_resolve_project_metadata_version_requires_pyproject(tmp_path) -> None:
    with pytest.raises(
        project_metadata.ProjectMetadataError, match="pyproject.toml was not found"
    ):
        project_metadata.resolve_project_metadata_version(tmp_path)


@pytest.mark.parametrize(
    ("config", "message"),
    [
        ("build-backend = 42", "build-backend must be a string"),
        ('backend-path = "backend"', "backend-path must be a list of strings"),
        ("backend-path = [42]", "backend-path must be a list of strings"),
    ],
)
def test_resolve_project_metadata_version_rejects_invalid_backend_config(
    tmp_path, config, message
) -> None:
    (tmp_path / "pyproject.toml").write_text(f"[build-system]\n{config}\n", encoding="utf-8")

    with pytest.raises(project_metadata.ProjectMetadataError, match=message):
        project_metadata.resolve_project_metadata_version(tmp_path)


@pytest.mark.parametrize(
    ("backend", "message"),
    [
        pytest.param("", "PEP 517 prepare_metadata_for_build_wheel failed", id="missing-hook"),
        pytest.param(
            "raise RuntimeError('broken backend')",
            "PEP 517 prepare_metadata_for_build_wheel failed",
            id="backend-error",
        ),
        pytest.param("return ''", "invalid dist-info directory", id="empty-directory"),
        pytest.param(
            "return '../outside.dist-info'",
            "invalid dist-info directory",
            id="parent-directory",
        ),
        pytest.param(
            r"return 'nested\\metadata.dist-info'",
            "invalid dist-info directory",
            id="windows-directory",
        ),
        pytest.param("return 'missing.dist-info'", "failed to read", id="missing-metadata"),
        pytest.param(
            """
            from pathlib import Path
            path = Path(metadata_directory, "demo.dist-info")
            path.mkdir()
            (path / "METADATA").write_text("Name: demo\\n", encoding="utf-8")
            return path.name
            """,
            "project metadata does not contain a Version field",
            id="missing-version",
        ),
    ],
)
def test_resolve_project_metadata_version_reports_backend_failures(
    tmp_path, backend, message
) -> None:
    (tmp_path / "pyproject.toml").write_text(
        '[build-system]\nrequires = []\nbuild-backend = "demo_backend"\nbackend-path = ["."]\n',
        encoding="utf-8",
    )
    source = ""
    if backend:
        source = (
            "def prepare_metadata_for_build_wheel(metadata_directory, config_settings=None):\n"
            + textwrap.indent(textwrap.dedent(backend).strip(), "    ")
            + "\n"
        )
    (tmp_path / "demo_backend.py").write_text(source, encoding="utf-8")

    with pytest.raises(project_metadata.ProjectMetadataError, match=message):
        project_metadata.resolve_project_metadata_version(tmp_path)


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

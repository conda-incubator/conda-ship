from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import textwrap
from typing import TYPE_CHECKING

import pytest
from conda_ship import cli
from conda_ship.cli import configure_parser, execute, run_cs
from conda_ship.project_metadata import ProjectMetadataError

if TYPE_CHECKING:
    from collections.abc import Sequence


class FakeProcess:
    def __init__(self, returncode: int, stderr: Sequence[str] = ()) -> None:
        self.returncode = returncode
        self.stderr = iter(stderr)

    def wait(self) -> int:
        return self.returncode


def test_configure_parser_collects_ship_args() -> None:
    parser = argparse.ArgumentParser(prog="conda ship")
    configure_parser(parser)

    args = parser.parse_args(["build", "--artifact-layout", "online", "--runtime-name", "demo"])

    assert args.ship_args == ["build", "--artifact-layout", "online", "--runtime-name", "demo"]


@pytest.mark.parametrize(
    ("argv", "expected"),
    [
        pytest.param(
            ["build", "--runtime-name", "demo"],
            ["build", "--runtime-name", "demo"],
            id="args",
        ),
        pytest.param(["--"], ["--help"], id="separator-defaults-to-help"),
        pytest.param([], ["--help"], id="empty-defaults-to-help"),
    ],
)
def test_run_cs_delegates_to_executable(
    argv: Sequence[str],
    expected: list[str],
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cs = tmp_path / ("cs.exe" if cli.os.name == "nt" else "cs")
    cs.write_text("")
    cs.chmod(0o755)
    expected_command = [str(cs), *expected]
    calls: list[tuple[list[str], dict]] = []

    def fake_popen(args: list[str], **kwargs) -> FakeProcess:
        calls.append((args, kwargs))
        return FakeProcess(17)

    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    status = run_cs(argv, executable=str(cs))

    assert status == 17
    assert calls[0][0] == expected_command
    assert calls[0][1]["stderr"] == cli.subprocess.PIPE
    assert calls[0][1]["env"][cli.ERROR_FORMAT_ENV] == "json"


def test_run_cs_reports_missing_executable(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    python = bin_dir / "python"
    python.write_text("")
    monkeypatch.setattr(cli.sys, "executable", str(python))

    status = run_cs([])

    assert status == 127
    assert "could not find" in capsys.readouterr().err


def test_run_cs_prefers_current_environment_binary(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    python = bin_dir / "python"
    cs = bin_dir / ("cs.exe" if cli.os.name == "nt" else "cs")
    python.write_text("")
    cs.write_text("")
    cs.chmod(0o755)
    calls: list[list[str]] = []

    def fake_popen(args: list[str], **_kwargs) -> FakeProcess:
        calls.append(args)
        return FakeProcess(0)

    monkeypatch.setattr(cli.sys, "executable", str(python))
    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    assert run_cs(["inspect"]) == 0
    assert calls == [[str(cs), "inspect"]]


def test_run_cs_applies_runtime_version_args(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cs = tmp_path / ("cs.exe" if cli.os.name == "nt" else "cs")
    cs.write_text("")
    cs.chmod(0o755)
    calls: list[list[str]] = []

    def fake_runtime_version_args(args: list[str]) -> list[str]:
        assert args == ["build"]
        return ["build", "--runtime-version", "2.3.4"]

    def fake_popen(args: list[str], **_kwargs) -> FakeProcess:
        calls.append(args)
        return FakeProcess(0)

    monkeypatch.setattr(cli, "runtime_version_args", fake_runtime_version_args)
    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    assert run_cs(["build"], executable=str(cs)) == 0
    assert calls == [[str(cs), "build", "--runtime-version", "2.3.4"]]


def test_run_cs_reports_project_metadata_error(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    cs = tmp_path / ("cs.exe" if cli.os.name == "nt" else "cs")
    cs.write_text("")
    cs.chmod(0o755)

    def fake_runtime_version_args(_args: list[str]) -> list[str]:
        raise ProjectMetadataError("metadata unavailable")

    monkeypatch.setattr(cli, "runtime_version_args", fake_runtime_version_args)

    assert run_cs(["build"], executable=str(cs)) == 1
    assert "metadata unavailable" in capsys.readouterr().err


def test_run_cs_rejects_invalid_env_executable(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    python = bin_dir / "python"
    fallback_cs = bin_dir / ("cs.exe" if cli.os.name == "nt" else "cs")
    python.write_text("")
    fallback_cs.write_text("")
    fallback_cs.chmod(0o755)

    monkeypatch.setattr(cli.sys, "executable", str(python))
    monkeypatch.setenv("CONDA_SHIP_EXECUTABLE", str(tmp_path / "missing-cs"))

    status = run_cs([])

    assert status == 127
    assert "CONDA_SHIP_EXECUTABLE" in capsys.readouterr().err


def test_run_cs_rejects_directory_env_executable(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("CONDA_SHIP_EXECUTABLE", str(tmp_path))

    status = run_cs([])

    assert status == 126
    assert "directory" in capsys.readouterr().err


def test_run_cs_rejects_empty_env_executable(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("CONDA_SHIP_EXECUTABLE", "")

    status = run_cs([])

    assert status == 127
    assert "set but empty" in capsys.readouterr().err


@pytest.mark.parametrize(
    ("failure", "returncode", "expected_status", "expected_stderr"),
    [
        pytest.param(FileNotFoundError, None, 127, "no longer exists", id="file-not-found"),
        pytest.param(PermissionError, None, 126, "not executable", id="permission-error"),
        pytest.param(OSError, None, 126, "spawn failed", id="os-error"),
        pytest.param(None, -15, 143, None, id="signal-exit"),
    ],
)
def test_run_cs_normalizes_spawn_failures(
    failure: type[Exception] | None,
    returncode: int | None,
    expected_status: int,
    expected_stderr: str | None,
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    cs = tmp_path / ("cs.exe" if cli.os.name == "nt" else "cs")
    cs.write_text("")
    cs.chmod(0o755)

    def fake_popen(_args: list[str], **_kwargs) -> FakeProcess:
        if failure is None:
            assert returncode is not None
            return FakeProcess(returncode)
        if failure is OSError:
            raise failure("spawn failed")
        raise failure

    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    assert run_cs([], executable=str(cs)) == expected_status
    if expected_stderr is not None:
        assert expected_stderr in capsys.readouterr().err


def test_run_cs_formats_structured_diagnostic(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    cs = tmp_path / ("cs.exe" if cli.os.name == "nt" else "cs")
    cs.write_text("")
    cs.chmod(0o755)

    def fake_popen(_args: list[str], **_kwargs) -> FakeProcess:
        return FakeProcess(
            1,
            [
                "checking project\n",
                json.dumps(
                    {
                        "schema_version": 1,
                        "tool": "cs",
                        "command": "build",
                        "kind": "missing_lockfile",
                        "message": "lockfile not found",
                        "hint": "run pixi lock",
                        "exit_code": 1,
                        "causes": [],
                    }
                )
                + "\n",
            ],
        )

    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    assert run_cs(["build"], executable=str(cs)) == 1
    stderr = capsys.readouterr().err
    assert "checking project" in stderr
    assert "conda-ship: lockfile not found" in stderr
    assert "hint: run pixi lock" in stderr
    assert '"schema_version"' not in stderr


def test_run_cs_forwards_plain_failure_stderr(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    cs = tmp_path / ("cs.exe" if cli.os.name == "nt" else "cs")
    cs.write_text("")
    cs.chmod(0o755)

    def fake_popen(_args: list[str], **_kwargs) -> FakeProcess:
        return FakeProcess(1, ["plain failure\n"])

    monkeypatch.setattr(cli.subprocess, "Popen", fake_popen)

    assert run_cs(["build"], executable=str(cs)) == 1
    assert "plain failure" in capsys.readouterr().err


def test_execute_returns_cs_status(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[list[str]] = []

    def fake_run_cs(args: Sequence[str]) -> int:
        calls.append(list(args))
        return 3

    monkeypatch.setattr(cli, "run_cs", fake_run_cs)
    args = argparse.Namespace(ship_args=["inspect"])

    assert execute(args) == 3
    assert calls == [["inspect"]]


@pytest.mark.parametrize("returncode", [0, 17])
def test_cli_module_preserves_arguments_environment_and_output(
    returncode,
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("CONDA_SHIP_EXECUTABLE", sys.executable)
    monkeypatch.setenv("CONDA_SHIP_ERROR_FORMAT", "text")
    monkeypatch.setenv("CONDA_SHIP_TEST_VALUE", "inherited")
    diagnostic = json.dumps(
        {
            "schema_version": 1,
            "tool": "cs",
            "kind": "missing_lockfile",
            "message": "lockfile not found",
            "hint": "run pixi lock",
        }
    )
    script = textwrap.dedent(
        f"""
        import json
        import os
        import sys

        print(json.dumps({{
            "args": sys.argv[1:],
            "format": os.environ["CONDA_SHIP_ERROR_FORMAT"],
            "inherited": os.environ["CONDA_SHIP_TEST_VALUE"],
        }}))
        sys.stderr.write("checking project\\n" + {diagnostic!r} + "\\n")
        sys.exit({returncode})
        """
    )
    forwarded = ["value with spaces", "--name=two words", "--", "-x", ""]

    result = subprocess.run(
        [sys.executable, "-m", "conda_ship.cli", "--", "-c", script, *forwarded],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
    )

    assert result.returncode == returncode
    assert json.loads(result.stdout) == {
        "args": forwarded,
        "format": "json",
        "inherited": "inherited",
    }
    expected = "conda-ship: lockfile not found\nhint: run pixi lock" if returncode else diagnostic
    assert result.stderr == f"checking project\n{expected}\n"


@pytest.mark.parametrize(
    ("diagnostic", "expected"),
    [
        ([], "[]"),
        ({"schema_version": 2, "tool": "cs"}, None),
        ({"schema_version": 1, "tool": "another-tool"}, None),
        ({"kind": 3, "message": "failure"}, None),
        ({"kind": "failure", "message": []}, None),
        ({"kind": "failure", "message": "failure", "hint": []}, None),
        ({"kind": "failure", "message": "failure"}, "conda-ship: failure\n"),
    ],
)
def test_run_cs_handles_diagnostic_variants(
    diagnostic,
    expected,
    capsys: pytest.CaptureFixture[str],
) -> None:
    if isinstance(diagnostic, dict):
        diagnostic = {"schema_version": 1, "tool": "cs", **diagnostic}
    stderr = json.dumps(diagnostic)

    status = run_cs(
        ["-c", "import sys\nsys.stderr.write(sys.argv[1])\nsys.exit(1)", stderr],
        executable=sys.executable,
    )

    assert status == 1
    assert capsys.readouterr().err == (stderr if expected is None else expected)


@pytest.mark.skipif(os.name == "nt", reason="POSIX signal exit status")
def test_run_cs_normalizes_real_signal_exit() -> None:
    status = run_cs(
        ["-c", "import os\nimport signal\nos.kill(os.getpid(), signal.SIGTERM)"],
        executable=sys.executable,
    )

    assert status == 128 + signal.SIGTERM


@pytest.mark.skipif(os.name == "nt", reason="POSIX executable permissions")
def test_run_cs_rejects_nonexecutable_file(tmp_path, capsys: pytest.CaptureFixture[str]) -> None:
    cs = tmp_path / "cs"
    cs.write_text("", encoding="utf-8")
    cs.chmod(0o644)

    assert run_cs([], executable=str(cs)) == 126
    assert "file that is not executable" in capsys.readouterr().err

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import textwrap

from conda_ship.plugin import conda_subcommands


def test_registers_ship_subcommand() -> None:
    subcommands = list(conda_subcommands())

    assert [subcommand.name for subcommand in subcommands] == ["ship"]


def test_subcommand_has_summary_and_actions() -> None:
    (subcommand,) = conda_subcommands()

    assert "conda runtimes" in subcommand.summary
    assert callable(subcommand.action)
    assert callable(subcommand.configure_parser)


def test_subcommand_configures_parser() -> None:
    (subcommand,) = conda_subcommands()
    parser = argparse.ArgumentParser(prog="conda ship")

    subcommand.configure_parser(parser)

    args = parser.parse_args(["lock", "--check"])
    assert args.ship_args == ["lock", "--check"]


def test_plugin_defers_adapter_import_until_subcommands_are_requested() -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            textwrap.dedent(
                """
                import json
                import sys
                import conda_ship.plugin

                imported_before = "conda_ship.cli" in sys.modules
                list(conda_ship.plugin.conda_subcommands())
                print(json.dumps([imported_before, "conda_ship.cli" in sys.modules]))
                """
            ),
        ],
        capture_output=True,
        text=True,
        check=True,
        timeout=30,
    )

    assert json.loads(result.stdout) == [False, True]

"""Sphinx configuration for conda-ship documentation."""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.abspath(".."))

project = html_title = "conda-ship"
copyright = "2026, conda community"
author = "conda community"

extensions = [
    "myst_parser",
    "sphinx.ext.intersphinx",
    "sphinx_copybutton",
    "sphinx_design",
    "sphinxcontrib.mermaid",
    "sphinx_sitemap",
]

myst_enable_extensions = [
    "colon_fence",
    "deflist",
]

html_theme = "conda_sphinx_theme"
html_static_path = ["_static"]
html_css_files = ["css/custom.css"]

mermaid_height = "auto"
mermaid_fullscreen = False
mermaid_light_theme = "neutral"

html_theme_options = {
    "icon_links": [
        {
            "name": "GitHub",
            "url": "https://github.com/conda-incubator/conda-ship",
            "icon": "fa-brands fa-square-github",
            "type": "fontawesome",
        },
    ],
}

html_context = {
    "github_user": "conda-incubator",
    "github_repo": "conda-ship",
    "github_version": "main",
    "doc_path": "docs",
}

html_baseurl = "https://conda-incubator.github.io/conda-ship/"

intersphinx_mapping = {
    "conda-express": ("https://jezdez.github.io/conda-express/", None),
    "conda-workspaces": ("https://conda-incubator.github.io/conda-workspaces/", None),
}

exclude_patterns = ["_build"]

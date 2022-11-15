# Configuration file for the Sphinx documentation builder.
#
# For the full list of built-in configuration values, see the documentation:
# https://www.sphinx-doc.org/en/master/usage/configuration.html

import datetime
import os
import subprocess
import sys

sys.path.insert(0, os.path.abspath("../src"))

# -- Project information -----------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#project-information

year = datetime.datetime.now().year

project = "Predictable"
copyright = f"{year}, Ratul Maharaj"
author = "Ratul Maharaj"
release = (
    subprocess.run(
        ["git", "describe", "--tags", "--abbrev=0"], capture_output=True
    )
    .stdout.decode("utf-8")
    .replace("\n", "")
)

# -- General configuration ---------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#general-configuration

extensions = [
    "sphinx.ext.duration",
    "sphinx.ext.autodoc",
    "sphinx_copybutton",
    "sphinxext.opengraph",
    "myst_parser",
]

templates_path = ["_templates"]
exclude_patterns = ["_build", "Thumbs.db", ".DS_Store"]


# -- Options for HTML output -------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#options-for-html-output

html_title = "Predictable"
html_theme = "furo"
html_theme_options = {
    "source_repository": "https://github.com/RatulMaharaj/predictable/",
    "source_branch": "main",
    "source_directory": "docs/",
}
html_static_path = ["_static"]

# Configuration file for the Sphinx documentation builder.
#
# For the full list of built-in configuration values, see the documentation:
# https://www.sphinx-doc.org/en/master/usage/configuration.html

import pathlib
import sys

sys.path.insert(0, pathlib.Path(__file__).parents[2].resolve().as_posix())

# -- Project information -----------------------------------------------------
# https://www.sphinx-doc.org/en/master/usage/configuration.html#project-information

with open("../.version", "r") as f:
    version = f.read()

project = "Divinate"
copyright = "2022, Ratul Maharaj"
author = "Ratul Maharaj"
release = version

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

html_title = "Divinate"
html_theme = "furo"
html_theme_options = {
    "source_repository": "https://github.com/RatulMaharaj/divinate/",
    "source_branch": "main",
    "source_directory": "docs/",
}
html_static_path = ["_static"]

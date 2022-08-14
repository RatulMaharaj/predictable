import setuptools

with open("README.md", "r", encoding="utf-8") as fh:
    long_description = fh.read()

setuptools.setup(
    name="divination",
    version="0.0.0",
    author="Ratul Maharaj",
    author_email="ratulmaharaj@gmail.com",
    description="A framework for Actuarial modelling.",
    long_description=long_description,
    long_description_content_type="text/markdown",
    url="https://github.com/RatulMaharaj/divination",
    project_urls={
        "Bug Tracker": "https://github.com/RatulMaharaj/divination/issues",
    },
    classifiers=[
        "Programming Language :: Python :: 3",
        "License :: OSI Approved :: MIT License",
        "Operating System :: OS Independent",
    ],
    package_dir={"divination": "divination"},
    packages=["divination"],
    python_requires=">=3.10",
    install_requires=["pandas", "numpy"],
    tests_require=["pytest"],
    include_package_data=True,
    package_data={"": ["tables/*.csv"]},
)

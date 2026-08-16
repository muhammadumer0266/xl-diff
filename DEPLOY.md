# Publishing xl-diff to PyPI

1. Ensure package metadata in `pyproject.toml` is correct (name, version, description).
2. Build distributions:

```bash
python -m build
```

3. Test upload to TestPyPI (recommended):

```bash
python -m twine upload --repository testpypi dist/*
```

4. Upload to PyPI (use API token stored in `~/.pypirc` or env var `TWINE_PASSWORD`):

```bash
python -m twine upload dist/*
```

Notes

- Make sure your chosen project name `xl-diff` is unique on PyPI.
- Use TestPyPI to verify installation before the real upload.
- For CI, add steps to build wheels for manylinux using `cibuildwheel` or GitHub Actions.

# PiDI tests

Run from the deploy root (`apps/pidi` or `~/midi-tone`):

```bash
export PYTHONPATH="$PWD"
python -m unittest discover -s tests -p 'test_*.py'
```

On Windows PowerShell:

```powershell
$env:PYTHONPATH = (Resolve-Path .).Path
python -m unittest discover -s tests -p 'test_*.py'
```

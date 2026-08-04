#!/bin/sh
# data-science provisioning: the scipy-notebook image already ships
# JupyterLab, git, and the scientific-Python stack (pandas/numpy/scipy/
# scikit-learn/matplotlib), so setup is near-empty — mostly verification
# plus one small, guarded extra. Idempotent — re-runs are no-ops. Runs as
# the jovyan user; Jupyter listens on 8888.
set -eu

python -c 'import seaborn' 2>/dev/null || pip install --no-cache-dir seaborn

echo "data-science ready: $(python --version); $(jupyter --version 2>/dev/null | head -n1 || echo jupyter)"

#!/bin/sh
# ml-pytorch provisioning: the pytorch-notebook image already ships PyTorch,
# torchvision, git, and the scipy/Jupyter stack, so setup mostly verifies.
# Idempotent — guarded so re-runs are no-ops. CPU only (the default, non-cuda
# tag is correct for microVMs: libkrun has no GPU passthrough).
set -eu

python -c 'import torchmetrics' 2>/dev/null || pip install --no-cache-dir torchmetrics

echo "ml-pytorch ready: $(python -c 'import torch; print(torch.__version__)')"

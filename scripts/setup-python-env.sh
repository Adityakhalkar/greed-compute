#!/bin/bash
set -e

VENV_DIR="${1:-.venv}"

echo "Setting up Python ML environment in ${VENV_DIR}..."

python3 -m venv "${VENV_DIR}"
source "${VENV_DIR}/bin/activate"

pip install --upgrade pip --quiet
pip install numpy pandas scikit-learn matplotlib scipy --quiet

python3 -c "import numpy, pandas, sklearn, matplotlib, scipy; print('All libraries installed successfully')"

echo "Python ML sandbox environment is ready at ${VENV_DIR}."

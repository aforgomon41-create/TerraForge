"""Compare the native homogeneous-ocean model with actual climlab EBM_seasonal.

Use the optional environment described in science_reference.py. First run:
  cargo run -p tf-sim --example climate_reference
  artifacts/science-venv/Scripts/python scripts/science_ebm_reference.py

This benchmark excludes terrain, ice feedback and seasonality; it is a solver
comparison, not validation of a real Earth's climate or of rainfall.
"""
import argparse
import json
import os
from pathlib import Path
import warnings

# This small reference system is faster and reproducible without BLAS fan-out.
os.environ["OPENBLAS_NUM_THREADS"] = "1"
os.environ["OMP_NUM_THREADS"] = "1"
os.environ["MKL_NUM_THREADS"] = "1"
warnings.filterwarnings("ignore", message="Cannot import")
import climlab
import numpy as np

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--native", type=Path, default=Path("artifacts/climate-native-reference.json"))
parser.add_argument("--output", type=Path, default=Path("artifacts/climate-ebm-comparison.json"))
args = parser.parse_args()
native = json.loads(args.native.read_text(encoding="utf-8"))
p = native["parameters"]
model = climlab.EBM_seasonal(
    num_lat=180, water_depth=p["water_depth_m"], S0=p["solar_constant"],
    a0=p["albedo"], a2=0., ai=p["albedo"], A=p["A"], B=p["B"], D=p["D"],
    orb={"ecc": 0., "long_peri": 0., "obliquity": 0.},
)
model.integrate_years(40, verbose=False)
lat = np.asarray(model.lat)
temperature = np.asarray(model.Ts).ravel()
cells = native["cells"]
predicted = np.array([c["temperature_c"] for c in cells])
reference = np.interp([c["latitude_deg"] for c in cells], lat, temperature)
weights = np.array([c["area_m2"] for c in cells])
weights /= weights.sum()
error = predicted - reference
result = {
    "source": "climlab.EBM_seasonal", "climlab_version": climlab.__version__,
    "case": native["case"], "integration_years": 40, "latitude_bands": 180,
    "parameters": p,
    "rms_error_c": float(np.sqrt((weights * error**2).sum())),
    "max_error_c": float(np.abs(error).max()),
    "mean_bias_c": float((weights * error).sum()),
    "native_mean_c": float((weights * predicted).sum()),
    "climlab_mean_c": float((weights * reference).sum()),
    "native_min_c": float(predicted.min()),
    "reference_latitude_deg": lat.tolist(), "reference_temperature_c": temperature.tolist(),
}
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
print(json.dumps({k: v for k, v in result.items() if not k.startswith("reference_")}, indent=2))
if predicted.min() < 0. or not native["energy"]["annual_cycle_converged"]:
    raise SystemExit("Native benchmark must be warm and converged to exclude ice feedback.")
if result["rms_error_c"] > 0.5 or result["max_error_c"] > 1.2:
    raise SystemExit("Native EBM differs from the complete climlab reference beyond tolerance.")

"""Execute scientific upstream code; writes only a caller-selected reference file.

Optional validation environment (not a product dependency):
  py -3.14 -m venv artifacts/science-venv
  artifacts/science-venv/Scripts/python -m pip install climlab==0.9.2 scipy
  artifacts/science-venv/Scripts/python scripts/science_reference.py

The pinned Landlab Python module is fetched from its immutable Git revision;
its complete MIT licence and provenance are in third_party/landlab.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import urllib.request

import climlab
import numpy as np
from climlab.solar.insolation import daily_insolation
from climlab.utils.heat_capacity import ocean

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, default=root / "artifacts/science-reference.json")
args = parser.parse_args()
provenance = json.loads((root / "third_party/landlab/upstream.json").read_text())
module_path = root / "artifacts/science-upstream/landlab/src/landlab/components/flexure/funcs.py"
module_path.parent.mkdir(parents=True, exist_ok=True)
url = f"https://raw.githubusercontent.com/{provenance['repo']}/{provenance['commit']}/src/landlab/components/flexure/funcs.py"
source = urllib.request.urlopen(url, timeout=30).read()
module_path.write_bytes(source)
spec = importlib.util.spec_from_file_location("landlab_flexure_reference", module_path)
flexure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(flexure)
orbit = {"ecc": 0.0, "long_peri": 0.0, "obliquity": 23.44}
rows = []
for lat in [-89, -60, -23, 0, 23, 60, 89]:
    for longitude in [0, 90, 180, 270]:
        value = float(np.asarray(daily_insolation(lat, longitude, orb=orbit, S0=1361.0, day_type=2)).item())
        rows.append({"latitude_deg": lat, "solar_longitude_deg": longitude, "insolation_w_m2": value})
result = {
    "climlab_version": climlab.__version__, "insolation": rows,
    "heat_capacity_50m": float(ocean(50)),
    "landlab_flexure_length": float(flexure.get_flexure_parameter(65000., 7e10, 1)),
    "landlab_deflection": flexure.subside_point_load(1e9, 0., np.array([0., 10000., 100000.]), params={"eet":65000.,"youngs":7e10}).tolist(),
}
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
print(f"Scientific reference written: {args.output}")

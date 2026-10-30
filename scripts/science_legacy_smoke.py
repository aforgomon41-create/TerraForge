"""Check annual-only legacy imports using a downgraded disposable fixture.

Run scripts/smoke.py first, then pass its roundtrip.tfworld as --project.
This is a schema-compatibility fixture, not a claimed historical observation.
"""
import argparse
import array
import json
import math
from pathlib import Path
import time
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--base", default="http://127.0.0.1:18792/map")
parser.add_argument("--project", required=True, type=Path)
parser.add_argument("--report", type=Path, default=Path("artifacts/science-legacy-report.json"))
args = parser.parse_args()
base = args.base.rstrip("/") + "/api/v1"
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

def call(path, token=None, data=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    request = urllib.request.Request(base + path, headers=headers, data=json.dumps(data).encode() if data is not None else None)
    with opener.open(request, timeout=120) as response:
        return json.load(response)

project = json.loads(args.project.read_bytes())
project["generator_version"] = "tf-0.3"
for entry in project["maps"]:
    entry["node"]["generator_version"] = "tf-0.3"
    for key in ["climate_atlas", "climate_model", "tectonic_history"]:
        entry["metadata"].pop(key, None)
    for key in ["potential_evaporation", "aridity", "crustal_strain", "orogen_age"]:
        entry["channels"].pop(key, None)
token = call("/session", data={})["token"]
try:
    world = call("/projects/import", token, project)
    maps = call("/worlds/" + world["id"] + "/maps", token)
    root = next(m for m in maps if m["parent"] is None)
    assert call("/maps/" + root["id"] + "/climate?cell=0", token)["available"] is False
    # Remain within every source map and use its stored geographic centre.
    grid = root["grid"]
    lat, lon = grid.get("center_lat", 35.0), grid.get("center_lon", 0.0)
    result = call("/maps/" + root["id"] + "/children", token, {"kind": "continent", "center_lat": lat, "center_lon": lon, "width_m": 64000})
    for _ in range(300):
        job = call("/jobs/" + result["job_id"], token)
        if job["state"] in ["done", "failed", "cancelled"]:
            break
        time.sleep(0.2)
    assert job["state"] == "done", job
    request = urllib.request.Request(base + "/maps/" + result["map_id"] + "/field/potential_evaporation", headers={"Authorization": "Bearer " + token})
    with opener.open(request, timeout=30) as response:
        pet = array.array("f")
        pet.frombytes(response.read())
    assert max(pet) > 0 and all(math.isfinite(v) for v in pet)
    assert call("/maps/" + result["map_id"] + "/climate?cell=0", token)["available"] is False
    report = {"result": "passed", "fixture": "tf-0.3 annual-only schema", "monthly_state": "explicitly unavailable", "child_pet_max_mm": max(pet)}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))
finally:
    request = urllib.request.Request(base + "/session", headers={"Authorization": "Bearer " + token}, method="DELETE")
    opener.open(request, timeout=30).close()

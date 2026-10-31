"""Generate independent spherical projection reference data using installed PROJ via pyproj."""
import json, math
from pathlib import Path
import pyproj

SCALE = 2 * (math.sqrt(3) / 2) * 1.340264 / math.pi
output = {'pyproj_version': pyproj.__version__, 'proj_version': pyproj.proj_version_str, 'sphere_radius': 1, 'cases': []}
for name, code, scale in [('equirectangular', 'eqc', 2/math.pi), ('mercator', 'merc', 2/math.pi),
                           ('mollweide', 'moll', 1/math.sqrt(2)), ('equal-earth', 'eqearth', SCALE)]:
    projection = pyproj.Proj(proj=code, R=1)
    for lat, lon in [(-89.9,-179.9),(-80,179),(-45,-120),(-10,122),(0,0),(23.44,-80),(47,122),(80,-179),(89.9,179.9)]:
        if name == 'mercator' and abs(lat)>85: continue
        x,y = projection(lon,lat)
        output['cases'].append({'projection':name,'lat':lat,'lon':lon,'x':x*scale,'y':y*scale})
folder=Path('third_party/proj');folder.mkdir(parents=True,exist_ok=True)
(folder/'projection-reference.json').write_text(json.dumps(output,indent=2)+'\n',encoding='utf-8')
print(f"{len(output['cases'])} reference coordinates; pyproj {pyproj.__version__}, PROJ {pyproj.proj_version_str}")

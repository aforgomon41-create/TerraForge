//! Controlled replay of real saved elevation/weather through the current hydrology.
//! Input: descriptor.json, optional climate.json, and named f32le channel files.
use std::{fs, path::Path};
use tf_grid::{GridDescriptor, GridOps};
use tf_sim::{ecology::Biome, WorldFields};
fn read(path: &Path) -> Vec<f32> {
    fs::read(path)
        .unwrap()
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn write(path: &Path, data: &[f32]) {
    fs::write(
        path,
        data.iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
}
fn stats(grid: &impl GridOps, f: &WorldFields) -> serde_json::Value {
    let marine = tf_sim::lakes::marine_cells(grid, &f.elevation);
    let enclosed_subsea = (0..grid.cell_count())
        .filter(|&i| f.elevation[i] <= 0. && !marine[i])
        .count();
    let land = (0..grid.cell_count())
        .filter(|&i| f.elevation[i] > 0.)
        .collect::<Vec<_>>();
    let area: f64 = land.iter().map(|&i| grid.area_m2(i)).sum();
    let wet = land
        .iter()
        .copied()
        .filter(|&i| f.lake_depth[i] > 0.5)
        .collect::<Vec<_>>();
    let wet_area: f64 = wet.iter().map(|&i| grid.area_m2(i)).sum();
    let mut depth = wet.iter().map(|&i| f.lake_depth[i]).collect::<Vec<_>>();
    depth.sort_unstable_by(f32::total_cmp);
    let mut visited = vec![false; grid.cell_count()];
    let mut sizes = Vec::new();
    for &i in &wet {
        if visited[i] {
            continue;
        }
        let mut todo = vec![i];
        visited[i] = true;
        let mut count = 0;
        while let Some(c) = todo.pop() {
            count += 1;
            for &j in grid.neighbors(c) {
                let j = j as usize;
                if !visited[j] && f.elevation[j] > 0. && f.lake_depth[j] > 0.5 {
                    visited[j] = true;
                    todo.push(j);
                }
            }
        }
        sizes.push(count);
    }
    sizes.sort_unstable();
    serde_json::json!({"land_cells":land.len(),"enclosed_subsea_cells":enclosed_subsea,"lake_cells":wet.len(),"lake_area_fraction":wet_area/area.max(1.),"lake_components":sizes.len(),"one_cell_lakes":sizes.iter().filter(|&&s|s==1).count(),"median_lake_depth_m":depth.get(depth.len()/2),"maximum_depth_m":depth.last()})
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);
    fs::create_dir_all(output).unwrap();
    let descriptor: GridDescriptor =
        serde_json::from_slice(&fs::read(input.join("descriptor.json")).unwrap()).unwrap();
    let grid = descriptor.build();
    let mut f = WorldFields::with_capacity(grid.cell_count());
    for (name, values) in [
        ("elevation", &mut f.elevation),
        ("precipitation", &mut f.precipitation),
        ("temperature", &mut f.temperature),
        ("potential_evaporation", &mut f.potential_evaporation),
        ("lake_depth", &mut f.lake_depth),
        ("distance_to_sea", &mut f.distance_to_sea),
    ] {
        *values = read(&input.join(format!("{name}.bin")));
        assert_eq!(values.len(), grid.cell_count());
    }
    let climate = input.join("climate.json");
    if climate.is_file() {
        f.climate = Some(serde_json::from_slice(&fs::read(climate).unwrap()).unwrap());
    }
    let cryosphere = input.join("cryosphere.json");
    if cryosphere.is_file() {
        let atlas = serde_json::from_slice(&fs::read(cryosphere).unwrap()).unwrap();
        f.cryosphere = Some(tf_sim::cryosphere::sample(&grid, &f, atlas, 0));
    }
    let before = stats(&grid, &f);
    let height = f.elevation.clone();
    let start = std::time::Instant::now();
    let state = if args.get(3).is_some_and(|s| s == "erosion") {
        tf_sim::erosion::run_erosion(
            &grid,
            &mut f,
            &tf_sim::erosion::ErosionParams {
                iterations: 18,
                ..Default::default()
            },
        )
    } else {
        tf_sim::erosion::refresh_hydrology(&grid, &mut f)
    };
    tf_sim::cryosphere::update_water_surfaces(&grid, &mut f);
    let ecology = tf_sim::ecology::run_ecology(&grid, &f);
    let biomes = ecology.biome.iter().map(|&b| b as f32).collect::<Vec<_>>();
    let receivers = state
        .flow_dir
        .iter()
        .map(|&r| if r == u32::MAX { -1. } else { r as f32 })
        .collect::<Vec<_>>();
    for (name, values) in [
        ("elevation", &f.elevation),
        ("lake_depth", &f.lake_depth),
        ("flow_accumulation", &f.flow_accumulation),
        ("flow_receiver", &receivers),
        ("biome", &biomes),
    ] {
        write(&output.join(format!("{name}.bin")), values);
    }
    let report = serde_json::json!({"mode":args.get(3),"seconds":start.elapsed().as_secs_f64(),"height_unchanged":height==f.elevation,"before":before,"after":stats(&grid,&f),"water_budget":state.lake_budget,"sediment":state.sediment,"inland_water_cells":ecology.biome.iter().filter(|&&b|b==Biome::InlandWater as u8).count(),"wetland_cells":ecology.biome.iter().filter(|&&b|b==Biome::Wetland as u8).count()});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{report}");
}

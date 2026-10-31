//! Fixed-rock diagnostic: isolate interior detail and volcanic relief before erosion.
use tf_core::params::{GenerationParams, SimQuality, WorldShape};
use tf_grid::GridOps;

fn main() -> anyhow::Result<()> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "artifacts/relief-probe".into());
    let params = GenerationParams {
        seed: std::env::args()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(42),
        world_shape: WorldShape::Plane,
        quality: SimQuality::Economy,
        ..Default::default()
    };
    let descriptor = tf_server::api::root_descriptor(&params, 512).map_err(anyhow::Error::msg)?;
    let grid = descriptor.build();
    let mut rock = tf_sim::WorldFields::with_capacity(grid.cell_count());
    let state = tf_sim::evolution::generate(&grid, &params, &mut rock, &mut |_| {}, &|| false)
        .ok_or_else(|| anyhow::anyhow!("tectonic generation cancelled"))?;
    std::fs::create_dir_all(&output)?;
    for case in [
        "all",
        "no-interior",
        "no-volcanoes",
        "no-hotspots",
        "no-arcs",
    ] {
        let mut f = rock.clone();
        let mut p = params.clone();
        let mut s = state.clone();
        match case {
            "no-interior" => p.interior_relief = 0.,
            "no-volcanoes" => s.history.volcanoes.clear(),
            "no-hotspots" => s.history.volcanoes.retain(|v| v.origin != "hotspot"),
            "no-arcs" => s.history.volcanoes.retain(|v| v.origin != "subduction_arc"),
            _ => {}
        }
        tf_sim::evolution::landforms(&grid, &mut f, &s, &p);
        let png = tf_server::render::render_map_png(
            &f.elevation,
            &vec![11.; grid.cell_count()],
            &descriptor,
            1536,
        )?;
        std::fs::write(format!("{output}/{case}.png"), png)?;
    }
    std::fs::write(
        format!("{output}/volcanoes.json"),
        serde_json::to_vec_pretty(&state.history.volcanoes)?,
    )?;
    println!("Fixed-rock relief diagnostics: {output}");
    Ok(())
}

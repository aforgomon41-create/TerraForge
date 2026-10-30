# Scientific kernels used by TerraForge

TerraForge's native fast model adapts small, identified kernels from these MIT projects. It does not embed either complete Python framework or claim their complete models have been run in the product.

| Project | Pinned source | Adapted components |
| --- | --- | --- |
| climlab | `climlab/upstream.json` | Circular-orbit daily insolation; water-column heat capacity; A+B*T outgoing longwave loss; seasonal EBM architecture |
| Landlab | `landlab/upstream.json` | `get_flexure_parameter` and the 1-D `subside_point_load` Green function |

Full upstream MIT notices are in each directory's LICENSE. Native adaptations are in `crates/tf-sim/src/scientific.rs`; the two-dimensional implicit heat transport and the kinematic material evolution are TerraForge implementations. The normal product build requires no Python or Fortran.

`scientific-reference.json` was produced by actually executing climlab 0.9.2 and the pinned Landlab source in an isolated Python environment on 2026-10-30. It covers 28 circular-orbit insolation cases, 50 m water-column heat capacity, flexure length and three line-load deflections. Rust tests compare against these values. Reproduce with `scripts/science_reference.py`; optional validation dependencies are documented by that script.

`climlab/ebm-reference.json` was produced on 2026-10-31 by actually integrating the complete climlab 0.9.2 `EBM_seasonal` for 40 years on 180 latitude bands. The warm, zero-obliquity homogeneous ocean excludes ice feedback and seasonality. This checks the native spherical diffusion and radiative solver together, beyond the individual formula tests. Its area-weighted temperature RMS difference is 0.248 C and its largest difference is 0.857 C. Reproduce with `cargo run --locked -p tf-sim --example climate_reference`, then `artifacts/science-venv/Scripts/python scripts/science_ebm_reference.py`. The native regression test uses the checked-in upstream latitude/temperature values; Python remains optional. This controlled case does not validate generated-world rainfall, ice, real geography, or the climate of Earth.

GPlates is a methodological reference for rigid spherical Euler rotations, not a bundled or copied dependency. Cortial et al. (2019), https://doi.org/10.1111/cgf.13614, is a methodological reference for procedural crust history, not an assertion that the paper's engine is included.

Limits: the EBM is a reduced surface energy model, not a 3-D atmospheric/oceanic GCM. Monthly PET uses the empirical Thornthwaite method; it is a diagnostic of vegetation water demand, not a claim of exact latent-energy closure. Continental resistance, crustal redistribution and volcanic event lifetimes are documented kinematic parameterizations requiring calibration. Neither complete ExoPlaSim nor complete Landlab is shipped.

//! Bounded monthly/annual vector diagnostics; no full-resolution map allocation.
use crate::api::{ApiError, AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use glam::DVec3;
use serde::Deserialize;
use serde_json::{json, Value};
use tf_grid::{GridDescriptor, GridOps, MapGrid};

#[derive(Deserialize)]
pub struct VectorQuery {
    /// Zero is the annual vector mean; 1..12 are January..December.
    #[serde(default)]
    month: u8,
}
pub async fn get_vectors(
    State(st): State<AppState>,
    Path((id, kind)): Path<(String, String)>,
    Query(query): Query<VectorQuery>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(kind.as_str(), "wind" | "current") || query.month > 12 {
        return Err(ApiError::bad_request(
            "请选择风场或洋流；月份应为0～12，0表示年平均",
        ));
    }
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("环流资料读取繁忙"))?;
    let result=tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let _permit=permit;
        let Some(atlas)=crate::climatology::for_map(&st.store,&node)? else {
            return Ok(json!({"available":false,"reason":"旧工程没有风场和洋流资料，请重新生成"}));
        };
        let Some(c)=&atlas.circulation else {
            return Ok(json!({"available":false,"reason":"此工程尚未计算风场和洋流，请重新生成"}));
        };
        let source=atlas.grid.build();
        let descriptor=node.grid.as_ref().ok_or_else(||anyhow::anyhow!("缺少地图网格"))?;
        let target=match descriptor.clone() {
            GridDescriptor::Sphere{radius_m,..} => GridDescriptor::sphere(20,radius_m).build(),
            GridDescriptor::Plane{size,rows,width_m,center_lat,center_lon,radius_m,reference_lat,version} => {
                let cols=size.min(64).max((4.*size as f64/rows as f64).ceil() as u32);
                GridDescriptor::Plane{size:cols,rows:((rows as f64/size as f64*cols as f64).round() as u32).max(4),width_m,center_lat,center_lon,radius_m,reference_lat,version}.build()
            },
            GridDescriptor::Patch{center_lat,center_lon,width_m,size,radius_m,..} => GridDescriptor::patch(center_lat,center_lon,width_m,size.min(64),radius_m).build(),
        };
        let (u,v)=if kind=="wind" {(&c.wind_east_m_s,&c.wind_north_m_s)}else{(&c.current_east_m_s,&c.current_north_m_s)};
        let mean=|values:&[Vec<f32>],i:usize| -> f64 {
            if query.month==0 { values.iter().map(|v|v[i] as f64).sum::<f64>()/12. }
            else {values[query.month as usize-1][i] as f64}
        };
        let chart=target.extent_m().is_some();
        let spacing=target.extent_m().map_or(target.radius_m()*0.14,|(w,h)|w.min(h)/10.);
        let mut chosen:Vec<DVec3>=Vec::new();
        let mut samples=Vec::new();
        let mut vectors=Vec::new();
        let mut maximum=0_f64;
        for i in 0..target.cell_count() {
            let dir=target.center_dir(i);
            let physical=target.field_position(i);
            if chosen.iter().any(|q|q.distance(physical)<spacing) {continue;}
            let Some(j)=source.cell_at(dir) else {continue;};
            if kind=="current" && c.ocean_fraction[j]<0.6 {continue;}
            let (east,north)=(mean(u,j),mean(v,j));
            let speed=east.hypot(north);
            if speed<1e-6 {continue;}
            let p=if chart {let(x,y)=target.local_center_m(i).unwrap();DVec3::new(x,y,0.)}else{dir.as_vec()};
            let (e,n)=dir.tangent_frame();
            let velocity=e*east+n*north;
            let direction=if chart {
                match &target {
                    MapGrid::Patch(g)=>{
                        let next=tf_core::geo::UnitVec3::from_vec(dir.as_vec()+velocity/target.radius_m()).unwrap();
                        let(x,y)=g.project(next);(DVec3::new(x*target.radius_m(),y*target.radius_m(),0.)-p).normalize_or_zero()
                    },
                    _=>DVec3::new(east,north,0.).normalize_or_zero(),
                }
            }else{velocity.normalize_or_zero()};
            let length=if chart {spacing*0.40}else{spacing/target.radius_m()*0.40};
            let tip=p+direction*length;
            let side=if chart {DVec3::Z.cross(direction)}else{p.cross(direction).normalize_or_zero()};
            let surface=|q:DVec3|if chart{q}else{q.normalize()*1.0006};
            let points=[p,tip,tip,tip-direction*length*0.32+side*length*0.20,tip,tip-direction*length*0.32-side*length*0.20];
            vectors.push((speed,points.map(surface)));
            let(lat,lon)=dir.to_lat_lon_deg();
            samples.push(json!({"latitude":lat,"longitude":lon,"east_m_s":east,"north_m_s":north,"speed_m_s":speed}));
            maximum=maximum.max(speed);chosen.push(physical);
            if samples.len()>=700 {break;}
        }
        let mut groups:[Vec<f32>;4]=std::array::from_fn(|_|Vec::new());
        for (speed,points) in vectors {
            let group=((speed/maximum.max(1e-6)*3.999) as usize).min(3);
            for p in points {groups[group].extend(p.to_array().map(|x|x as f32));}
        }
        Ok(json!({"available":true,"kind":kind,"month":query.month,"model":c.model,
            "heat_coupled":c.heat_coupled,"units":"m/s","maximum_speed_m_s":maximum,
            "relative_solver_residual":c.relative_solver_residual,"solver_iterations":c.solver_iterations,
            "current_velocity_scale":c.current_velocity_scale,"forcing_wind_rms_difference_m_s":c.forcing_wind_rms_difference_m_s,
            "coupling":"one lagged wind/ocean/temperature feedback; not jointly converged",
            "groups":groups,"samples":samples,"reference_frame":"local east/north; annual means average vectors, not scalar speeds; arrow length is uniform"}))
    }).await??;
    Ok(Json(result))
}

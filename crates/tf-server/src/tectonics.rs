//! Bounded diagnostic geometry, separate from the terrain surface.
use crate::{
    api::{ApiError, AppState},
    store::Channel,
};
use axum::{
    extract::{Path, State},
    Json,
};
use glam::DVec3;
use serde_json::{json, Value};
use tf_grid::{GridDescriptor, GridOps, MapGrid};

pub async fn get_tectonics(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let node = st
        .store
        .get_map_node(&id)?
        .ok_or_else(|| ApiError::not_found("地图不存在"))?;
    let permit = st
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("板块图层正在读取，请稍后重试"))?;
    let result=tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let _permit=permit;
        let metadata=st.store.get_metadata(&id)?.unwrap_or_default();
        let plates:Vec<tf_sim::Plate>=serde_json::from_value(metadata.get("plates").cloned().unwrap_or(json!([])))?;
        if plates.is_empty() { return Ok(json!({"available":false,"reason":"此地图没有板块运动记录"})); }
        anyhow::ensure!(plates.len()<=64 && plates.iter().enumerate().all(|(i,p)|p.id as usize==i && p.omega.is_finite() && p.omega.abs()<=1. && (DVec3::from_array(p.euler_pole).length()-1.).abs()<0.001),"板块运动记录无效");
        let descriptor=node.grid.as_ref().ok_or_else(||anyhow::anyhow!("缺少网格"))?;
        let fine=descriptor.build();
        let coarse=match descriptor.clone() {
            GridDescriptor::Sphere{radius_m,..}=>GridDescriptor::sphere(48,radius_m).build(),
            GridDescriptor::Plane{size,rows,width_m,center_lat,center_lon,radius_m,reference_lat,version}=>{
                let cols=size.min(128).max((4.*size as f64/rows as f64).ceil() as u32);
                GridDescriptor::Plane{size:cols,rows:((rows as f64/size as f64*cols as f64).round() as u32).max(4),width_m,center_lat,center_lon,radius_m,reference_lat,version}.build()
            },
            GridDescriptor::Patch{center_lat,center_lon,width_m,size,radius_m,..}=>GridDescriptor::patch(center_lat,center_lon,width_m,size.min(96),radius_m).build(),
        };
        let ids=st.store.get_field(&id,Channel::PlateId)?.ok_or_else(||anyhow::anyhow!("缺少板块编号"))?;
        let crust=st.store.get_field(&id,Channel::CrustType)?.ok_or_else(||anyhow::anyhow!("缺少地壳类型"))?;
        anyhow::ensure!(ids.len()==fine.cell_count()&&crust.len()==fine.cell_count(),"板块字段长度无效");
        let planar:Option<Vec<DVec3>>=metadata.get("planar_velocities").filter(|v|!v.is_null()).map(|v|serde_json::from_value(v.clone())).transpose()?;
        if let Some(v)=&planar {anyhow::ensure!(v.len()==plates.len()&&v.iter().all(|p|p.is_finite()&&p.length()<=2e6),"平面运动速度无效");}
        if (metadata.get("motion_domain").and_then(Value::as_str)==Some("plane") || (node.parent.is_none() && matches!(descriptor,GridDescriptor::Plane{..}))) && planar.is_none() {
            return Ok(json!({"available":false,"reason":"旧平面地图没有平移速度记录"}));
        }
        let mut f=tf_sim::WorldFields::default();
        for i in 0..coarse.cell_count() {
            let j=fine.cell_at(coarse.center_dir(i)).ok_or_else(||anyhow::anyhow!("板块取样超出地图"))?;
            let plate=ids[j] as usize;
            anyhow::ensure!(ids[j].is_finite()&&ids[j]>=0.&&ids[j].fract()==0.&&plate<plates.len(),"板块编号无效");
            f.plate_id.push(plate as u16);f.crust_type.push(crust[j]);
        }
        let root_motion=tf_sim::TectonicState{plates:plates.clone(),planar_velocities:planar.clone(),..Default::default()};
        // In inherited local charts, project spherical velocities into the chart's
        // metric axes at its centre. Arrows below retain each point's exact speed.
        let mut boundary_motion=root_motion.clone();
        if coarse.field_is_planar() && planar.is_none() {
            let centre=coarse.center_dir(coarse.cell_count()/2);let (east,north)=centre.tangent_frame();
            boundary_motion.planar_velocities=Some((0..plates.len()).map(|p|{
                let v=tf_sim::evolution::velocity(&root_motion,p,centre.as_vec()*coarse.radius_m());
                DVec3::new(v.dot(east),v.dot(north),0.)
            }).collect());
        }
        let boundaries=tf_sim::plates::find_boundaries(&coarse,&f.plate_id,&boundary_motion,&f);
        let mut groups:[Vec<f32>;4]=std::array::from_fn(|_|Vec::new());
        let chart=coarse.extent_m().is_some();
        let position=|i:usize|coarse.local_center_m(i).map_or_else(||coarse.center_dir(i).as_vec(),|(x,y)|DVec3::new(x,y,0.));
        for b in boundaries {
            let (i,j)=(b.cell as usize,b.other as usize);
            let common:Vec<_>=coarse.neighbors(i).iter().copied().filter(|k|coarse.neighbors(j).contains(k)).collect();
            if common.len()!=2 {continue;}
            let group=match b.kind {tf_sim::BoundaryKind::Convergent=>0,tf_sim::BoundaryKind::Divergent=>1,_=>2};
            for k in common {
                let p=(position(i)+position(j)+position(k as usize))/3.;
                let p=if chart{p}else{p.normalize()};
                groups[group].extend(p.to_array().map(|x|x as f32));
            }
        }
        let mut arrows=Vec::new();
        let mut chosen:Vec<DVec3>=Vec::new();
        let mut per_plate=vec![0;plates.len()];
        let spacing=coarse.extent_m().map_or(coarse.radius_m()*0.40,|(w,h)|w.min(h)*0.22);
        for i in 0..coarse.cell_count() {
            let plate=f.plate_id[i] as usize; if per_plate[plate]>=4 {continue;}
            let physical=if chart{position(i)}else{coarse.field_position(i)};
            if chosen.iter().any(|q|physical.distance(*q)<spacing){continue;}
            let spherical=coarse.center_dir(i);
            let v=if planar.is_some(){tf_sim::evolution::velocity(&root_motion,plate,physical)}else{tf_sim::evolution::velocity(&root_motion,plate,spherical.as_vec()*coarse.radius_m())};
            let speed=v.length()*1e-4; // m/Myr to cm/year
            if speed<0.001 {continue;}
            let p=position(i);
            let direction=if chart && planar.is_none() {
                match &coarse {
                    MapGrid::Patch(g)=>{
                        let next=tf_core::geo::UnitVec3::from_vec(spherical.as_vec()+v/coarse.radius_m()*0.01).unwrap();
                        let(x,y)=g.project(next);(DVec3::new(x,y,0.)-p).normalize_or_zero()
                    },
                    _=>{let(e,n)=spherical.tangent_frame();DVec3::new(v.dot(e),v.dot(n),0.).normalize_or_zero()}
                }
            }else{v.normalize_or_zero()};
            let length=if chart{spacing*0.42}else{0.085};
            let tip=p+direction*length;
            let side=if chart{DVec3::Z.cross(direction)}else{p.cross(direction).normalize_or_zero()};
            let surface=|x:DVec3|if chart{x}else{x.normalize()};
            for point in [p,tip,tip,tip-direction*length*0.32+side*length*0.20,tip,tip-direction*length*0.32-side*length*0.20] {
                groups[3].extend(surface(point).to_array().map(|x|x as f32));
            }
            let(lat,lon)=spherical.to_lat_lon_deg();
            arrows.push(json!({"plate_id":plate,"latitude":lat,"longitude":lon,"speed_cm_year":speed}));
            chosen.push(physical);per_plate[plate]+=1;
        }
        Ok(json!({"available":true,"groups":groups,"plates":plates,"planar_velocities":planar,"arrows":arrows,"reference_frame":"生成器固定地幔参考系；箭头长度统一，速度请查看数值","diagnostic_cells":coarse.cell_count()}))
    }).await??;
    Ok(Json(result))
}

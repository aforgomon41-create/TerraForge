//! Cartographic river segments follow the saved hydrological receiver graph.
use crate::{
    api::{ApiError, AppState},
    store::Channel,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use tf_grid::GridOps;

pub async fn get_hydrology(
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
        .map_err(|_| ApiError::busy("水系正在读取，请稍后重试"))?;
    let result=tokio::task::spawn_blocking(move||->anyhow::Result<Value>{
        let _permit=permit;
        let Some(receivers)=st.store.get_field(&id,Channel::FlowReceiver)? else{return Ok(json!({"available":false,"reason":"旧工程未保存河道连接，仍显示地形与湖泊；重新生成可补齐河流"}));};
        let flow=st.store.get_field(&id,Channel::FlowAccumulation)?.ok_or_else(||anyhow::anyhow!("缺少汇流资料"))?;
        let height=st.store.get_field(&id,Channel::Elevation)?.ok_or_else(||anyhow::anyhow!("缺少高程"))?;
        let lake=st.store.get_field(&id,Channel::LakeDepth)?.unwrap_or_else(||vec![0.;height.len()]);
        let metadata=st.store.get_metadata(&id)?.unwrap_or_default();
        let model=metadata.get("hydrology_model").and_then(|v|v.get("name")).and_then(Value::as_str);
        let legacy=model!=Some("depression-hierarchy-annual-balance-v1");
        let grid=node.grid.as_ref().ok_or_else(||anyhow::anyhow!("缺少地图网格"))?.build();
        let n=grid.cell_count();
        anyhow::ensure!([receivers.len(),flow.len(),height.len(),lake.len()].iter().all(|&s|s==n),"水系资料尺寸无效");
        let mut threshold=(node.cell_size_m.powi(2)*40.).max(100.);
        let eligible=|i:usize,t:f64| height[i]>0. && flow[i] as f64>=t && lake[i]<0.5 && receivers[i]>=0.;
        while (0..n).filter(|&i|eligible(i,threshold)).count()>60000 {threshold*=2.;}
        let mut lines=Vec::new();
        let mut count=0;
        for i in 0..n {
            if !eligible(i,threshold){continue;}
            let r=receivers[i];let j=r as usize;
            anyhow::ensure!(r.is_finite() && r.fract()==0. && j<n,"水系下游编号无效");
            if i==j{continue;}
            // A saved legacy potential route may climb over a dry sill. It is
            // not a currently flowing river. Account for f32 water-level roundoff.
            let down=if height[j]<=0. {0.} else {height[j]+lake[j]};
            if down>height[i]+lake[i]+0.002 {continue;}
            for cell in [i,j]{let p=if grid.field_is_planar(){grid.field_position(cell)}else{grid.center_dir(cell).as_vec()*1.0006};lines.extend(p.to_array().map(|x|x as f32));}
            count+=1;
        }
        Ok(json!({"available":true,"groups":[lines],"segments":count,"minimum_upstream_area_m2":threshold,"source":"saved_flow_receivers","model":model,"legacy_lakes":legacy,"notice":if legacy {Some("此地图使用旧水文规则，重新生成后可应用湖泊修正。")} else {None},"limited":threshold>(node.cell_size_m.powi(2)*40.).max(100.)}))
    }).await??;
    Ok(Json(result))
}

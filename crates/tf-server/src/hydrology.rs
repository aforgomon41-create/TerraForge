//! Surface river segments follow saved receivers and actual liquid support.
use crate::{
    api::{ApiError, AppState},
    store::Channel,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde_json::{json, Value};
use tf_grid::GridOps;

pub async fn get_hydrology(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::cryosphere::IceQuery>,
) -> Result<Json<Value>, ApiError> {
    if query.month > 12 {
        return Err(ApiError::bad_request("水系月份应为0～12"));
    }
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
        let n=height.len();
        let optional=|ch|st.store.get_field(&id,ch);
        let lake=optional(Channel::LakeDepth)?.unwrap_or_else(||vec![0.;n]);
        let ice=optional(Channel::LandIce)?;let snow=optional(Channel::SnowCover)?;
        let mut discharge=optional(Channel::LiquidDischarge)?;let frost=optional(Channel::Permafrost)?;
        let biome=optional(Channel::Biome)?;
        let temp=optional(Channel::Temperature)?;let rain=optional(Channel::Precipitation)?;
        let metadata=st.store.get_metadata(&id)?.unwrap_or_default();
        let model=metadata.get("hydrology_model").and_then(|v|v.get("name")).and_then(Value::as_str);
        let legacy=model!=Some("depression-hierarchy-annual-balance-v1");
        let grid=node.grid.as_ref().ok_or_else(||anyhow::anyhow!("缺少地图网格"))?.build();
        anyhow::ensure!([receivers.len(),flow.len(),lake.len(),grid.cell_count()].iter().all(|&s|s==n),"水系资料尺寸无效");
        for value in [&ice,&snow,&discharge,&frost,&biome,&temp,&rain].into_iter().flatten(){anyhow::ensure!(value.len()==n,"冰雪水系尺寸无效");}
        let climate=crate::climatology::for_map(&st.store,&node)?;
        if query.month>0 && climate.is_none(){return Ok(json!({"available":false,"reason":"此工程缺少月气候，不能判断河面冻结，请重新生成"}));}
        let climate_grid=climate.as_ref().map(|a|a.grid.build());
        let mut monthly_liquid=false;
        if query.month>0 {
            if let (Some(atlas),Some(t),Some(p))=(crate::cryosphere::for_map(&st.store,&node)?,&temp,&rain) {
                let mut f=tf_sim::WorldFields{elevation:height.clone(),temperature:t.clone(),precipitation:p.clone(),lake_depth:lake.clone(),
                    flow_accumulation:vec![0.;n],potential_evaporation:vec![0.;n],climate:climate.clone(),..Default::default()};
                let shelf=optional(Channel::IceShelf)?;
                let canonical=if atlas.model==tf_sim::cryosphere::MODEL {ice.as_deref().zip(shelf.as_deref())} else {None};
                let Some(view)=tf_sim::cryosphere::sample(&grid,&f,atlas,query.month,canonical) else{return Ok(json!({"available":false,"reason":"缺少与地图匹配的冰厚字段，请重新生成"}));};
                f.cryosphere=Some(view);
                let directions=receivers.iter().map(|&r|if r == -1. {Ok(u32::MAX)}else if r.is_finite() && r>=0. && r.fract()==0. && (r as usize)<n {Ok(r as u32)}else{Err(anyhow::anyhow!("水系下游编号无效"))}).collect::<anyhow::Result<Vec<_>>>()?;
                let water=tf_sim::erosion::from_saved_receivers(&grid,&mut f,directions).map_err(anyhow::Error::msg)?;
                discharge=Some(tf_sim::cryosphere::annual_discharge(&grid,&f,&water));monthly_liquid=true;
            }
        }
        let frozen=|i:usize|ice.as_ref().is_some_and(|v|v[i]>=1.) || snow.as_ref().is_some_and(|v|v[i]>0.8)
            || (ice.is_none() && biome.as_ref().is_some_and(|v|v[i]==1.));
        let eligible=|i:usize,t:f64|height[i]>0. && flow[i] as f64>=t && lake[i]<0.5 && receivers[i]>=0.
            && !frozen(i) && discharge.as_ref().is_none_or(|q|q[i]>0.01);
        let mut threshold=(node.cell_size_m.powi(2)*40.).max(100.);
        while (0..n).filter(|&i|eligible(i,threshold)).count()>60000 {threshold*=2.;}
        let mut groups=[Vec::new(),Vec::new()];let mut counts=[0,0];let mut frozen_month=0;
        for i in 0..n {
            if !eligible(i,threshold){continue;}
            let r=receivers[i];let j=r as usize;
            anyhow::ensure!(r.is_finite() && r.fract()==0. && j<n,"水系下游编号无效");
            if i==j || frozen(j){continue;}
            let down=if height[j]<=0. {0.}else{height[j]+lake[j]};
            if down>height[i]+lake[i]+0.002 {continue;}
            let monthly=if let(Some(a),Some(g),Some(t),Some(p))=(&climate,&climate_grid,&temp,&rain){
                Some(a.sample(g,grid.center_dir(i),t[i],p[i]).0)
            }else{None};
            if query.month>0 && monthly.as_ref().is_some_and(|t|t[query.month as usize-1]<=0.){frozen_month+=1;continue;}
            let seasonal=frost.as_ref().is_some_and(|v|v[i]>0.1)
                || monthly.as_ref().is_some_and(|t|t.iter().any(|&v|v<0.));
            let group=usize::from(seasonal);
            for cell in [i,j]{let p=if grid.field_is_planar(){grid.field_position(cell)}else{grid.center_dir(cell).as_vec()*1.0006};groups[group].extend(p.to_array().map(|x|x as f32));}
            counts[group]+=1;
        }
        Ok(json!({"available":true,"groups":groups,"segments":counts.iter().sum::<i32>(),"perennial_segments":counts[0],"seasonal_segments":counts[1],
            "month":query.month,"frozen_month_segments":frozen_month,"minimum_upstream_area_m2":threshold,"source":"saved_flow_receivers","model":model,"legacy_lakes":legacy,
            "liquid_support":discharge.is_some(),"monthly_liquid":monthly_liquid,"flow_basis":if monthly_liquid {"selected-month rain/snowmelt/ice-melt support"}else{"annual liquid support; selected-month frozen surfaces hidden"},
            "notice":if legacy {Some("此地图使用旧水文规则，重新生成后可应用湖泊修正。")}else if ice.is_none(){Some("此工程未保存季节冰雪；重新生成后可按月查看海冰、冰架与河流冻结。")}else{None},
            "limited":threshold>(node.cell_size_m.powi(2)*40.).max(100.)}))
    }).await??;
    Ok(Json(result))
}

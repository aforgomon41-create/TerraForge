# IUCN生态分类名录

`source.json` 从 IUCN Red List of Ecosystems Science Team 的公开仓库提取英文名称、正式代码与层级关系；固定提交 `c245ee11f58528a9cfdcb44dc8fd75253f08ac15`。来源：https://github.com/red-list-ecosystem/typology-map-data/tree/c245ee11f58528a9cfdcb44dc8fd75253f08ac15/data/config ，MIT许可全文保存在 `LICENSE-IUCN.txt`。

只使用分类事实，没有复制地图、照片、图示或完整说明。中文是本项目译名，不声称IUCN官方译名。颜色由本项目定义。

`catalogue.json` 是运行时唯一的共享名录，包含25大类、110细类、中文名称和当前支持范围。离线运行 `scripts/build_ecosystem_catalogue.py` 生成；`--check` 核对生成结果。大类ID冻结为1～25；细类ID为 `大类ID×16＋细类序号`，因此必须用至少16位整数保存，不能转成Uint8。0表示未识别到细类，不能用0指代海洋或默认森林。

支持范围 `environment_estimate` 表示当前有环境诊断规则；`additional_process` 表示还需要潮汐、火、土壤化学、植被或水体过程；`subsurface` 表示缺少地下空间模拟；`human_activity` 需要人类活动系统。列入名录不表示已经具备全部类型的自动生成能力。

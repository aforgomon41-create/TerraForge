# 大陆与区域生成改进

第一轮已实现：范围／格距规划、连续采样、主河道约束、跨图来水、缓冲区裁切，以及独立区域的河网地貌。用户审阅布局并回复“ok”后，已接入原选区面板。保留既有世界生成流程与自然彩绘风格；全部源码、测试、接口与文档由主代理修改。结果与限制见 `regional-generation-acceptance-2026-11-06.md`。

第二轮接续实现局部地形降雨、出域再入域河流及压缩工程交换，不新增页面或布局。结果、方法依据、资源边界与未实现的外部输沙契约见 `regional-climate-and-returns-2026-11-06.md`。第一轮验收保留为当时记录。

## 第一轮契约

- 生成完整对象概览与指定格距细化分开；范围过大时不暗中截断、不以粗采样冒充高精度。实际计算格距与显示六角格距独立。
- 父图采样改用真实三角面内的连续插值；大尺度地形、主要河流与出入口传递到子图。细节以世界种子和根坐标确定，保留父图已有的尺度结构。
- 外部来水显式进入年度及月度水量账本；仅传递图外上游贡献，避免把图内降雨重复算作外来水。边界区分入流、出流与封闭地形，保留父图水面基准。旧图若缺少输沙通道，标明未知，不假定其有精确输沙资料。
- 局部侵蚀形成的河谷保留，不在结束时全域回拉至粗图。开放边界与海岸控制带有实际空间宽度，图内地形可随侵蚀和沉积变化。
- 工程保存／重新打开、按月水文、子图再次细化都必须读取相同的边界条件，禁止依赖临时进程内状态。

## 验证

使用山地流域、冲积平原、干旱盆地、海岸河口四类固定场景，对照高程、水系与自然渲染。检查河道顺坡、外来水与本地降雨不重复、出水与蒸发收支、父子山河锚点、重叠范围采样连续、同种子重现、旧工程兼容与资源上限。跨平台不要求浮点逐位相同，单个平台内保存／导入要求字段一致。

## 依据与边界

借鉴 [Landlab 外来流量示例](https://landlab.csdms.io/tutorials/landscape_evolution/river_input_lem/adding_discharge_point_source_to_a_lem.html)、[SPACE](https://landlab.csdms.io/tutorials/landscape_evolution/space/SPACE_large_scale_eroder_user_guide_and_examples.html) 以及 [Génevaux等（2013）](https://perso.liris.cnrs.fr/eric.galin/Articles/2013-river-networks.pdf) 的河网约束思路。代码仍采用项目内Rust实现；引用方法不等于运行完整Landlab或完整大气模型。界面布局需先通过 `regional-refinement-layout-preview.html` 审阅，再正式实现。

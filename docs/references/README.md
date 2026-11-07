# 自然彩绘参考与实际输出

- `natural-painted-target.png`：用户选择的第1张AI视觉概念。来自同一实际世界图的风格探索，不能视为地形或积雪的科学依据。
- `natural-painted-result.png`：655362格实际工程、实际水文、生产ES 3.00着色器和项目颜料材质在原生显卡离屏绘制的总图。1774×887，与参考同尺寸。
- `natural-painted-detail.png`：同一工程与生产着色器，4倍视图、中心地图坐标(0.25, 0.05)，用于查看地形光照、森林材质和水系。
- `regional-deployment-browser-2026-11-07.jpg`：修正内存配置后的正式HTTPS站点，Edge还原独立测试工程，显示80km自定范围、512×512点和156.25m实际格距。实际浏览器截图，未修改图像内容；详见对应发布记录。

当前结果的着色器SHA-256为 `4402f0e3aa35699fd578fbedd66a601a599609dad2e322bc35a7a480bc9d135f`；工程SHA-256为 `3b2ba08d17be710704c8acb6d581c05a517f0999003121a51da320c5f3d96d4a`。绘制脚本为 `scripts/render_map_qa.py`，实际JSON报告在 `artifacts/shader-qa/natural-painted-*/`。这些是实际着色器输出，不是浏览器或手机截图；浏览器验收缺口见项目根目录 `design-qa.md`。

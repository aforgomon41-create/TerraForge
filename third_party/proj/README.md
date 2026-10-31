# 投影公式的独立数值参考

`projection-reference.json` 由 `scripts/projection_reference.py` 实际调用 pyproj 3.8.0 / PROJ 9.8.1 生成，球半径取 1，再统一缩放到前端地图坐标。共 34 组经纬点，包括接近日期变更线和极区的位置。

正式前端不依赖 Python 或 pyproj，采用球面数学公式，自行实现正向与逆向变换。没有把 PROJ 完整引擎或源文件复制进程序。Equal Earth 多项式系数是其数学定义的一部分。

方法来源：

- [等距圆柱](https://proj.org/en/stable/operations/projections/eqc.html)
- [墨卡托](https://proj.org/en/stable/operations/projections/merc.html)
- [莫尔威德](https://proj.org/en/stable/operations/projections/moll.html)
- [Equal Earth](https://proj.org/en/stable/operations/projections/eqearth.html)，Savric、Patterson、Jenny（2018），DOI: 10.1080/13658816.2018.1504949。

pyproj / PROJ 均为 MIT 许可。此处保存的是执行后的坐标参考数据，不是上游库代码。可在隔离 Python 环境安装 pyproj 后运行生成脚本重现；固定版本记录在 JSON 中。

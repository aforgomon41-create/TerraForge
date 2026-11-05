# TerraForge 版权与许可说明

Copyright (C) 2026 kamisama and TerraForge contributors.

除另有明确声明的第三方部分外，本项目原创代码及随附原创材料按照 GNU Affero General Public License 第三版或后续版本（SPDX：`AGPL-3.0-or-later`）提供。完整英文条款见根目录 [LICENSE](LICENSE)。程序按现状提供，不提供许可证所排除的保证。

## 第三方来源

| 来源 | 仓库内的用途 | 独立许可与来源记录 |
| --- | --- | --- |
| climlab | 部分日照、热容量及季节能量模型内核的适配与验证 | [MIT 全文](third_party/climlab/LICENSE)、[固定来源](third_party/climlab/upstream.json) |
| Landlab | 部分岩石圈挠曲内核的适配与验证 | [MIT 全文](third_party/landlab/LICENSE)、[固定来源](third_party/landlab/upstream.json) |
| IUCN Red List of Ecosystems Science Team 的 typology-map-data | 生态分类代码、层级和英文名称；中文译名与调色由本项目提供 | [MIT 全文](data/ecosystems/LICENSE-IUCN.txt)、[提取边界及来源](data/ecosystems/README.md) |

上述第三方部分的原始版权与许可声明继续有效，不以项目总许可替换。Rust 与前端依赖保留各自的许可证，依赖版本记录在锁文件中。适配范围和未包含的完整科学框架见 [third_party/README.md](third_party/README.md)。

PROJ、GPlates 等在项目文档中作为数学或方法参考，不表示包含其完整软件或取得其背书。自然彩绘纹理的生成来源和使用边界见 [素材说明](apps/web/src/assets/README.md)。

## 中文摘要

- 允许商业使用，AGPL 不是非商用许可证。
- 分发软件及提供修改版网络服务时，应履行对应的源码提供、许可与版权声明义务；具体范围以英文许可证为准。
- 本文件是项目许可及来源说明，不修改 AGPL 的原文，也不替代各第三方的独立许可。

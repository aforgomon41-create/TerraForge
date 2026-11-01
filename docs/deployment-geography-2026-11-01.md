# 2026-11-01 自动地理目录发布

应用：<https://aforgomon.win/terraforge/>。刷新后，在工程的“地理目录”展开陆块、汇水区域和显著自然地点，点击可定位并高亮实际范围。旧工程首次读取时补做识别。原手动裁图入口改为“提高此处精度”，保存的采样子图标为精细图。地点暂为自然地物，湖盆只代表地形洼地。

实现提交 `e6e095f57f1647bd84835e6c1665d0287c3c1771`，分支 `codex/automatic-geography`。原理及接口见 [自动地理对象](geographic-recognition.md)，设计变动沿用已批准的纸张工作台布局。

## 发布与回退

发布目录 `/www/terraforge/releases/20261101-geography-e6e095f`；保留原服务 `terraforge-candidate.service`、8792端口、`/terraforge`、数据目录 `data-v05`，600 MiB内存及150% CPU限制。切换前确认无活动生成任务，备份服务配置，检查新配置后仅替换本服务的三个发布目录引用。运行进程 PID 1466562 的二进制哈希与发布文件一致。

保留前版 `/www/terraforge/releases/20261101-live-9c180ee`。回退时等待活动生成结束，恢复 `/www/terraforge/backups/20261101-geography-e6e095f/terraforge-candidate.service.before`，然后执行 `systemctl daemon-reload` 和重启该服务。没有修改数据库结构；地理目录是可重建的 metadata 缓存。

Nginx、证书、Cloudflare、其他服务和原 `/map/` 没有更改；外网两个应用入口均返回200。没有打包新的桌面安装程序。

## 来源校验

- Git源码归档 SHA-256：`77f784c601f902644d4c11b60deb7d86d0716271f9a000a664fa304e1e3927d7`。
- 发布二进制 SHA-256：`38813d1db2716ee55e42a51a1e680d0427310e09ba10bf80f03eb9d8ed0d2533`。
- 前端 JS `index-DduPrWBv.js`，CSS `index-CAKoAG9W.css`；外网下载的 JS、CSS、图标哈希与清单一致。HTML返回200，受现有防护注入影响，不以完整HTML字节一致为验证依据。
- 服务器保留 `source.tar.gz`、`source-commit.txt`、`manifest.json`；构建目录 `/www/terraforge/builds/20261101-geography-e6e095f`。

使用既有依赖缓存，锁定依赖、离线单任务构建；75% CPU、450 MiB内存上限，约1分37秒完成，CPU约63.7秒，内存峰值450 MiB，无交换空间使用。沿用部署时关闭LTO和16代码生成单元的覆盖设置，没有修改仓库全局构建配置。

## 验证与局限

136项Rust检查、27项前端检查、类型检查、正式构建及格式检查通过。本地默认球形655,362格和平面189,440格均完成真实HTTP识别及工程往返验证，临时会话正常清理。

上线后经过现有Nginx及HTTPS源站路径，使用正常主机名和证书校验生成种子42的平面世界：

- 189,440格，约147.43秒完成。
- 识别6个被边缘截断的陆地片区、78座岛、271个汇水区域、67个山峰、41个山口、189个河口、11个出界水流口、118个候选湖盆。
- 地理资料约2.52MB，缓存读取约0.06–0.07秒；导入后补算约0.20秒。
- 陆海归属、对象父子关系、定位点、重复读取、跨会话隔离、metadata剔除大缓存、对象不增加采样地图数量，以及保存导入后的归属一致性均通过。
- 完整测试期间服务未自动重启，服务内存峰值约169.3 MiB。单个样本不能保证所有参数的耗时和内存。

源站报告 `builds/20261101-geography-e6e095f/acceptance/plane.json`，本地副本 `artifacts/geography-deploy/server-plane.json`。测试只将测试进程中的域名解析指向本机，相当于 `curl --resolve`；没有关闭TLS校验或修改网站防护。

Computer Use因无法可靠识别浏览器网址而停止，本轮没有完成WebGL画面和移动触控的肉眼验收。HTTP及算法检查不能代替该结论。DSH调研超时且追问未进入执行，已核实取消，没有将其记为通过的审查；实现和验证由主代理完成。

GitHub remote及代表GPT的已核实邮箱仍未配置，本次为本地Git提交和服务器部署；没有猜测共同作者邮箱，也没有声称GitHub Contributors或头像已生效。

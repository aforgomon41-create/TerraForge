# 2026-10-30 部署记录

## 当前版本

已更新现有阿里云服务器上的 TerraForge，公开入口为 `https://aforgomon.win/map/`。未改动同机其他应用。

- 源码提交：`9b97c7c`（包含核心修复 `c15dec9`）。
- 运行目录：`/www/terraforge/releases/20261030-9b97c7c`。
- 服务：`terraforge.service`；通过独立 `20-release.conf` 覆盖项指向本次程序及静态页面，原基础服务文件保留。
- 服务仅监听 `127.0.0.1:8791`；公开请求由现有 Nginx HTTPS 代理。
- 模式：web / economy；并发 1，Rayon 与 Tokio 各 2 线程，内存限制沿用 900 MiB。
- 新会话数据库目录：`/www/terraforge/data-v2`。旧版没有会话归属的数据继续保留在 `/www/terraforge/data`，未删除或当作新会话公开。
- Nginx 仅把 `/map/api/` 的上传限制由 16 MiB 调整到 64 MiB，保留关闭缓冲的 SSE 配置；`nginx -t` 通过。

服务器程序 SHA-256：

`e3b60c3b91814873fb16aca1d326cfd7dbff7a71a01cee1a24411c9d5d18eabb`

公开浏览器加载脚本为 `index-o3H_74-L.js`，与本地生产构建一致；SHA-256：

`acc305d2d766bd3b7f6cec3806f71de56fbb3dc9997f6bd22b6bf4b2cc5152f6`

## 备份与回退

备份目录为 `/www/terraforge/backups/20261030-9b97c7c`，目录仅 root 可访问。包括旧二进制、旧页面压缩包、原服务配置、Nginx 配置和约 95 MiB 的旧数据库压缩包。数据库在旧服务停止后备份，避免复制写入中的数据库。原旧程序、旧页面和旧数据路径也保留。

若需回退，应先停止服务并备份新的 `data-v2`，再移除本次创建的服务覆盖项 `/etc/systemd/system/terraforge.service.d/20-release.conf`、恢复备份中的 Nginx 配置，执行服务配置重载、Nginx 检查及重启。基础服务会重新指向未覆盖的旧程序及旧数据。回退不会自动转换新旧工程格式，因此新版本工程应先导出为 `.tfworld` 留存。该回退路径由部署脚本提供，成功部署后没有为了演练而再次切换生产版本。

## 验收

- 本地同一源码通过 75 项 Rust 测试、12 项前端测试和正式构建。
- 服务器使用单编译任务构建 release 版成功，避免占满小内存机器。
- 源站健康版本 `0.2.0`，systemd 状态 active。
- 源站直连完成四层、工程往返、PNG、取消、删除及跨身份负向检查。
- 通过本机解析到 Nginx 的 HTTPS 域名再跑完整检查；TLS 证书仍按 `aforgomon.win` 正常校验，没有禁用证书校验。该检查验证反向代理，不用于冒充外网访问。
- 外网另以真实浏览器访问公开域名，建立临时会话、生成并显示完整星球；截图保存在 `artifacts/screenshots/online-workbench.jpg`。普通命令行探测仍会被 Cloudflare 1010 拒绝，未更改这项边缘保护。
- HTTPS 接口检查：世界 655,362 格、10.036 秒；大陆/区域/地点各 16,384 格，分别为 1,000/100/1 米格距；中间帧 5，PNG 1,024×512，工程往返逐项一致。
- 同时进行浏览器验收期间，服务内存峰值 413,380,608 字节，约 394 MiB；不代表高并发压测结果。
- 脚本创建的临时会话已清理；保留公开浏览器中的一份体验星球供用户查看，关闭页面或失联后按会话规则回收。

本地原始记录：`artifacts/acceptance/server-direct-report.json` 和 `server-nginx-report.json`。服务器记录位于本次运行目录的 `acceptance/` 和 `acceptance-nginx/`。这些运行产物没有加入 Git。

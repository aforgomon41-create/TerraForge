# 2026-11-01 实时演化与投影发布

应用：<https://aforgomon.win/terraforge/>。刷新后，球形世界可从顶部原视图选择框切换球面、等距圆柱、墨卡托、莫尔威德和 Equal Earth；生成过程中也可切换。旧工程直接使用新投影，新的实时演化需发起一次生成。没有重打桌面安装包。

## 发布来源与隔离

实现提交 `9c180ee62df72568d095ceba83c876abe4055906`，分支 `codex/live-evolution-projections`。说明与本地验证见 [实时演化与投影记录](live-evolution-projections-2026-11-01.md)。GitHub remote 和代表 GPT 的已核实邮箱仍未配置，因此仅创建本地提交，没有猜测共同作者邮箱，也没有声称 GitHub Contributors 或头像已经生效。

发布目录：`/www/terraforge/releases/20261101-live-9c180ee`。继续使用 `terraforge-candidate.service`、127.0.0.1:8792、`/terraforge` 和 `data-v05`，只修改该服务的工作目录、执行文件和前端目录。保留原单作业、600 MiB 内存、150% CPU 和会话清理设置。切换前确认无活动生成作业，备份原服务文件，并验证新服务配置。

没有修改 Nginx、证书、Cloudflare、原 `/map/` 对应的 geogen 应用、8791 旧服务或旧数据。旧发布 `/www/terraforge/releases/20261031-morphology` 保留。

## 构建与一致性

源码使用 Git 归档上传，SHA-256：

`c504d637853efbc05269db4cf9ef43d0665a5e4cd7b7ffb4224c6a13cdfb3138`

构建单任务、75% CPU，内存最初限制 350 MiB；因持续内存回收导致变慢，运行中上调到 450 MiB。总耗时约 3 分 56 秒，CPU 约 80.5 秒，内存峰值 450 MiB，未使用交换空间。继续使用发布环境覆盖关闭 LTO、16 个代码生成单元，没有修改仓库的全局 release 配置。

新二进制 SHA-256：

`c28fae00712e5ff2ef27fd42147712579233cac1f2624d06a1092a7098809f6f`

对照运行进程 `/proc/MainPID/exe` 与新发布文件，哈希一致。前端发布前逐一核对 4 个文件；从普通外网下载的 JS、CSS、图标与正式构建一致。HTML 返回成功，但 Cloudflare 会注入防护内容，因此不使用完整 HTML 字节一致作为证据。现有 `/map/` 外网检查仍为 HTTP 200。

发布目录保存源码归档、`source-commit.txt` 与前端哈希清单 `manifest.json`。服务器构建与测试目录为 `/www/terraforge/builds/20261101-live-9c180ee`。

## 验收

本地 Rust 合计 130 项与前端 25 项检查通过，类型检查、格式检查、正式构建通过。球形、平面、子图实时帧、压缩兼容、重连、取消及终态关闭通过。投影与独立运行的 PROJ 坐标对照通过。

HTTPS 源站验收使用正常证书和主机名验证，仅把测试进程的域名解析指向本机，等同 `curl --resolve`，经过现有 Nginx 位置。不关闭任何证书或浏览器防护。

实际生成经济档、种子 42 的平面世界（189,440 格）：约 160.69 秒完成，共 287 帧，其中 38 帧为构造状态；首帧约 0.086 秒，帧间隔中位数 0.548 秒，最长 2.030 秒。至少三组构造高度不同，时间顺序递增，并在气候求解内部持续收到年月状态。原始通道合计约 105.8 MB，gzip 数据约 54.9 MB，不含 JSON/base64/HTTP 开销；该采样较频繁，仍有传输成本。

断线后以未压缩兼容格式恢复最新状态约 0.043 秒。子图预览、跨会话隔离、运行中取消、完成／取消后回连返回终态及 EOF 均通过。验收脚本正常退出，服务没有自动重启；该轮服务内存峰值约 89.7 MiB。这是一个样本，不能保证所有参数下的帧率、带宽或内存。

服务器原始报告位于构建目录的 `acceptance/plane.json`，本地副本为 `artifacts/live-server-acceptance/plane.json`。测试采用临时会话，结束时请求清理。普通外网应用入口返回 HTTP 200，前端资源校验通过；外网浏览器交互结论仍受下面的工具限制约束。

浏览器连接不可用，桌面工具又因无法可靠识别当前网址自动停止，尚未完成 WebGL 画面和移动触控实机验收。不能用 HTTP、数学检查或 PNG 代替该结论。

## 回退

服务原文件保存在 `/www/terraforge/backups/20261101-live-9c180ee/terraforge-candidate.service.before`。先等待活动作业结束，再恢复该文件、执行 `systemctl daemon-reload` 并重新启动 `terraforge-candidate.service`，即可回到前一发布。保留当前数据和新旧发布目录；不需要覆盖 Nginx 或操作 `/map/`。

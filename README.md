# TerraForge · 世界生成器

以板块、侵蚀和气候的简化模型生成虚构星球，再从选中的位置逐层细化到大陆、区域与地点。界面是浅色纸张感的全屏地图工作台。

[在线体验](https://aforgomon.win/map/) · [审计报告](docs/audit-2026-10-29.md) · [改进与验证记录](docs/implementation-status.md) · [前端设计](docs/frontend-design.md)

## 正在开发的 0.3 分支

`codex/generation-fields` 已加入连贯的大陆地壳、连续断层场、水汽输运、共享顶点的地图表面和独立显示格网。种子留空可自动随机并保存，API 可以直接创建球形或平面世界，以及无父图的大陆、区域、地点；独立区域/地点使用局部地貌与气候设置。新创建面板仍等待布局审阅，新增参数入口尚未正式接入。此分支未替换线上 0.2，也未制作新安装包。[进度、实测与限制](docs/generation-v03-progress.md)。

## 已发布 0.2 的能力

- 四层地图树、真实位置选择、球面/平面查看、缩放与平移。
- 地形、温度、降水、生态、高程与水文图层；生成进度和阶段预览。
- 保存整个地图树为 `.tfworld` 工程，重新导入后保留字段；导出 PNG 地图。
- 网页端固定经济档（655,362 格），按会话隔离，离开后清理；异常断开默认 5 分钟失联后回收。重要数据请先保存工程。
- Windows 桌面版默认标准档（2,621,442 格），工程保存在本机，可离线生成与查看。
- AI 助手支持自填 OpenAI 兼容地址、模型和密钥，查询地图、切换视图及按要求生成。密钥默认只在内存中；只有主动勾选才保存到当前浏览器。AI 联网调用取决于供应商的协议和跨域支持。

这些模型用于创作地图，尚未经过地球科学预测验证。板块位置和热点是静态近似；不模拟真实板块输运、完整岛链或季节天气。局部细化继承父图的连续地形与气候，并做侵蚀调整，不能凭 1 米格宽推断已有同等精度的实测细节。当前没有地形编辑、人文推演或完整地下/覆盖层。

## 开发与运行

需要近期稳定版 Rust、Node.js 22.18+、pnpm 11.28.6。Windows 构建 Rust 需要 Visual Studio C++ Build Tools 和 WebView2；首次构建需要联网下载依赖。锁文件纳入版本管理。

```sh
# 项目根目录：后端默认只监听本机 8790
cargo run --release -p tf-server --locked

# 另一个终端
cd apps/web
pnpm install --frozen-lockfile
pnpm dev
```

打开 `http://localhost:5173`。开发代理保持浏览器原始来源，API 使用服务端颁发的会话凭据。

正式网页先在 `apps/web` 执行 `pnpm build`，再设置 `TF_STATIC_DIR` 为 `apps/web/dist` 的绝对路径，运行后端。页面同时支持根路径和默认 `/map/` 路径。服务器前应使用 HTTPS 反向代理，API/SSE 关闭代理缓冲，上传限制至少 64 MiB；服务本身仍只绑定回环地址。

## Windows 桌面打包

```sh
cargo install tauri-cli --version 2.12.4 --locked
# 先在 apps/web 完成 pnpm install，然后回到根目录
cargo tauri build --bundles nsis --ci
```

构建自动先检查并编译前端。安装包输出到 `target/release/bundle/nsis/`；也可直接运行 `target/release/terraforge-desktop.exe`。程序启动内嵌的本地服务并从同一端口提供打包资源，不依赖开发服务器。默认数据位于系统应用数据目录中的 `win.aforgomon.terraforge` 文件夹。更换版本前可额外保存 `.tfworld` 备份。

## 配置

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `TF_BIND` | `127.0.0.1` | 服务监听地址；桌面端强制回环 |
| `TF_PORT` | `8790` | 独立服务端口；桌面自动分配 |
| `TF_DATA_DIR` | `./data` | 独立服务数据库目录 |
| `TF_STATIC_DIR` | 无 | 网页构建产物目录 |
| `TF_BASE_PATH` | `/map` | 网页入口前缀；根入口仍可用 |
| `TF_MODE` | `web` | 网页会话临时保存；`desktop` 是本地持久化部署，不能用于公开网页 |
| `TF_QUALITY` | `economy` | 网页始终锁定经济档；本地独立服务可选 `standard` / `high` |
| `TF_MAX_CONCURRENT_JOBS` | `1` | 服务作业并发，普通用户不能修改 |
| `TF_MAX_QUEUE` | `16` | 等待队列上限 |
| `TF_SESSION_TTL_SECS` | `300` | 网页失联回收时间，范围 60～86,400 秒 |
| `TF_ADMIN_TOKEN` | 无 | 可选管理凭据，至少 32 字符；未配置时关闭管理操作 |
| `TF_DESKTOP_DATA_DIR` | 系统应用数据目录 | 桌面测试或自定义存储目录 |
| `TF_DESKTOP_QUALITY` | `standard` | 桌面生成档位，可选 `economy` / `high`；高档需要更多内存 |

局部地图格宽固定为大陆 1 km、区域 100 m、地点 1 m。请求覆盖宽度必须满足真实格数预算；经济档边长最多 512 格、标准档 1,024 格、高档 2,048 格，超过预算会明确拒绝，不会暗中降低分辨率。网页每会话最多 4 个世界，另有总量、读取与队列限制。重启后未完成任务会标记失败，可重新生成，尚不支持中途断点续算。

## 验证

```sh
cargo fmt --all --check
cargo test --workspace --locked
cd apps/web
pnpm format:check
pnpm test
pnpm build
```

0.2 发布时，Windows 本地通过 75 项 Rust 测试、12 项前端测试，并实际启动桌面包生成标准档星球。Linux CI 只检查四个核心 crate，避免把桌面 GTK 依赖带入服务端测试。

`scripts/smoke.py` 是需要运行服务的独立验收脚本，Python 3 标准库即可执行。请指向独立测试服务；脚本只创建、检查和清理自己持有的临时会话，不使用现有世界：

```sh
python scripts/smoke.py --base http://127.0.0.1:18790/map --out artifacts/acceptance
```

它验证四层继承、稳定种子、SSE、跨会话隔离、取消、完整工程往返及 PNG 尺寸。详细实测与局限见改进记录。

## 项目结构

| 目录 | 职责 |
| --- | --- |
| `crates/tf-core` | 球面几何、随机数、参数与通道 |
| `crates/tf-grid` | 球面/局部网格、版本描述和投影 |
| `crates/tf-sim` | 构造、侵蚀、水文、气候、生态与细化 |
| `crates/tf-server` | 会话、作业、存储、工程交换与 HTTP API |
| `apps/web` | Vue 3、WebGL2、AI 协议客户端 |
| `src-tauri` | Windows 桌面封装与本地持久化服务 |

AI 使用项目内有长度限制、参数校验和取消能力的协议客户端，已移除未使用的 Pi 依赖。真实地图坐标统一由后端提供；前端不再用另一套球面生成算法解释正式数据。

## Git 与许可

已保存导入基线与审计标签 `audit-baseline-2026-10-29`。构建产物、运行数据库、密钥和验收输出不进入 Git。当前未配置 GitHub 远程仓库。

AGPL-3.0-or-later。

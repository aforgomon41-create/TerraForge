# TerraForge · 世界生成器

以板块、侵蚀和气候的简化模型生成虚构星球，再从选中的位置逐层细化到大陆、区域与地点。界面是浅色纸张感的全屏地图工作台。

[在线体验](https://aforgomon.win/terraforge/) · [审计报告](docs/audit-2026-10-29.md) · [最新冰雪与生态验收](docs/ice-and-ecosystem-rendering-2026-11-05.md) · [前端设计](docs/frontend-design.md)

2026-11-07，大陆／区域两轮改进已部署：完整大陆概览、自定范围、自动／指定采样精度、父子山河与图外来水继承、独立区域河网地貌，以及局部地形降雨、河流出域再入域连接和压缩工程保存。[发布与恢复记录](docs/deployment-regional-2026-11-07.md) · [第一轮验收](docs/regional-generation-acceptance-2026-11-06.md) · [第二轮验收与限制](docs/regional-climate-and-returns-2026-11-06.md)。

## 当前网页版本（2026-11-07）

网页已部署当前生成引擎与自然彩绘地图。仓库保留从原型到当前版本的提交历史；旧的开发与部署文档记录对应日期的状态，以最新验收文档为准。

小内存Linux部署还需在进程启动时加载[内存回收参数示例](deploy/linux-hosted-memory.env.example)，并保留生成／重型读写的共享准入和单作业限制；只限制进程内存、但让系统持续保留已释放的大数组，连续细化仍可能触顶。具体失败及复验数据见发布记录。

- 随机或固定种子；可创建球形、独立平面世界，也可直接创建大陆、区域和地点，使用各自的生成设置。
- 球面可切换等距圆柱、墨卡托、莫尔威德和 Equal Earth 投影；支持缩放、平移、位置选择与显示网格。
- 板块平移、球面旋转、碰撞与裂解、板块生长和消亡；默认演化46亿年、输出步长1000万年，参数可调整。预览推送实际演化状态，初始化和最终资料计算仍可能有较长间隔。
- 风场、洋流、逐月气候、液态水文、冰雪与冻土、板块运动、高程、自然地形及自动分区图层；大陆、汇水区域和显著地点自动识别。
- 生态名录包含25大类、110细类；目前25个细类可作环境估计，其余保留上层分类及“细类待定”。自然彩绘按已识别细类、地方干湿环境、植被疏密与纹理绘制，叠加地形、湖泊及河道。
- 冰雪在实际地图网格上计算，分别保存接地冰、冰架、积雪、海冰、湖冰和冻土。旧v1工程可读取，但保存的错误冰厚需重新生成才能更新。
- 保存整个地图树为 `.tfworld` 工程，重新导入后保留字段；渲染地图导出当前彩绘视图 PNG。
- 网页端固定经济档，球形世界为655,362个采样点，地图显示格距独立于底层采样。工程按会话隔离，失联后清理，重要数据请先保存。在线部署采用10分钟失联回收，代码默认5分钟。
- 保留 Windows 桌面封装及本地持久工程；本轮网页更新未制作新安装包，旧0.2安装包不代表当前网页能力。
- AI 助手支持自填 OpenAI 兼容地址、模型和密钥。密钥默认只在内存中；只有主动勾选才保存到当前浏览器。AI 联网调用取决于供应商的协议和跨域支持。

这些模型用于创作地图，尚未经过地球科学预测验证。项目移植并比对了 climlab／Landlab 的部分科学内核，但没有运行完整地球系统模型；构造、气候、侵蚀与冰雪仍有明确近似。局部细化继承父图的连续地形与气候，不能凭1米格宽推断已有同等精度的实测细节。当前没有地形编辑、人文推演或完整地下／覆盖层。[科学依据](docs/scientific-model-review-2026-10-30.md) · [内核来源与许可](third_party/README.md)。

## 开源许可

本项目原创部分采用 **AGPL-3.0-or-later**（GNU Affero General Public License，第三版或后续版本），允许使用、修改、分享和商用。分发程序或修改版时，须按许可证提供对应源码；将修改版作为网络服务运行时，也须向远程交互用户提供获取该版本对应源码的机会。完整条件以 [LICENSE](LICENSE) 为准，第三方来源及独立许可见 [NOTICE.md](NOTICE.md)。

公开仓库保留完整提交历史，默认分支 `main` 对应当前代码。在线部署的具体验收版本和已知近似见最新验收记录；公开源码不代表所有旧安装包都包含当前功能。

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

正式网页先在 `apps/web` 执行 `pnpm build`，再设置 `TF_STATIC_DIR` 为 `apps/web/dist` 的绝对路径，运行后端。页面同时支持根路径和默认 `/map/` 路径；线上实例设置 `TF_BASE_PATH=/terraforge`。服务器前应使用 HTTPS 反向代理，API/SSE 关闭代理缓冲，上传限制至少64 MiB；服务本身仍只绑定回环地址。

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

局部细化可选择自动采样或指定计算格距。完整对象概览会按范围与格数预算规划实际格距；指定格距超预算时明确拒绝。经济档边长最多512格、标准档1024格、高档2048格。显示六角格距与实际计算格距独立。网页每会话最多4个世界，另有总量、读取与队列限制。新保存的`.tfworld`采用有长度上限的压缩封装，仍可导入旧JSON工程；网页上传仍最多64MiB，解压后最多128MiB，地图与元数据限制仍生效。重启后未完成任务会标记失败，可重新生成，尚不支持中途断点续算。

## 验证

```sh
cargo fmt --all --check
cargo test --workspace --locked
cd apps/web
pnpm format:check
pnpm test
pnpm build
```

2026-11-05冰雪与生态发布前，模拟111项、服务端27项、前端53项测试通过，并通过前端类型检查、构建、原生GPU与Edge浏览器验收。Linux线上默认完整球形世界生成约401秒，工程往返、季节冰雪、水文与生态检查通过。桌面0.2安装包的历史验收记录不代替当前源码的桌面打包验证。Linux CI 检查四个核心 crate，避免把桌面GTK依赖带入服务端测试。

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

已保存导入基线与审计标签 `audit-baseline-2026-10-29`。构建产物、运行数据库、密钥和验收输出不进入 Git。公开仓库为 [aforgomon41-create/TerraForge](https://github.com/aforgomon41-create/TerraForge)；大陆与区域改进位于 `codex/regional-generation` 分支。

AGPL-3.0-or-later。

# 2026-10-31 独立结果页发布记录

内核提交 `0090dc5`。本次仅发布气候对照结果，没有部署新的应用程序或桌面安装包。

- 新结果页：<https://aforgomon.win/terraforge-previews/science-v041-20261031/index.html>。
- 待审阅新建面板：<https://aforgomon.win/terraforge-previews/generation-v04.html>。
- 上一候选版：<https://aforgomon.win/terraforge-previews/science-v04-20261030/index.html>。

## 现场发现

现有 Nginx `/map/` 代理到 `127.0.0.1:8790/`，该端口由 `geogen:20261022c` 容器使用，静态目录来自 `/opt/geogen/web/dist`。页面标题为“地图生成 · 阿福公益站”。这与 2026-10-30 TerraForge 部署记录中的入口已不一致。

TerraForge 的 `terraforge.service` 仍使用 `/www/terraforge/releases/20261030-9b97c7c/tf-server`，监听 8791；其静态目录为同一发布目录下的 `web`，基路径 `/map`。本次没有修改上述应用、容器、数据或 `/map/` 代理。

## 实际变更

15 个 HTML、PNG 和 JSON 文件上传到：

`/www/terraforge/releases/20261030-9b97c7c/web/previews/science-v041-20261031`

文件 SHA-256 逐项与本地 `artifacts/science-v041-review/manifest.sha256.json` 核对。原 `/map/previews/` 现由另一应用处理并返回 404，因此在现有 HTTPS 虚拟主机内增加独立的静态位置：

```nginx
location ^~ /terraforge-previews/ {
    alias /www/terraforge/releases/20261030-9b97c7c/web/previews/;
    autoindex off;
    add_header X-Content-Type-Options nosniff always;
}
```

修改前的完整配置备份在 `/www/terraforge/backups/20261031-climate-preview/wbhub.before.conf`，目录仅 root 可访问。实际配置为 `/etc/nginx/sites-enabled/wbhub` 的解析目标；写入前核对原内容未被并发改变。`nginx -t` 通过后重新加载，保留所有既有代理位置。

## 验证与限制

通过域名正常校验 TLS，并解析到本机 Nginx，15 个资源全部返回 HTTPS 200；新建布局、上一候选版页面以及既有 `/map/` 也返回 200。该检查验证源站反向代理和静态文件，不能替代外网浏览器、WebGL 或移动端交互验收。

若需撤回本次预览，移除上述独立 `location`，检查并重新加载 Nginx 即可。不要直接用整份旧配置覆盖现行配置，以免覆盖此后其他修改。静态结果可以保留归档，不影响任何应用数据。

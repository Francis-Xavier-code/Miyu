# Miyu WebUI × matugen

WebUI 的全部颜色都建立在 [MD3 system token](https://m3.material.io/styles/color/roles)
(`--md-sys-color-*`)之上,默认值是从 miyu-logo 采样派生的「晨光 / 夜阑」两套配色。
用 [matugen](https://github.com/InioX/matugen) 可以让 WebUI 跟随桌面壁纸取色。

## 配置

在 `~/.config/matugen/config.toml` 中加入:

```toml
[templates.miyu]
input_path = "~/Documents/github/Miyu/extra/matugen/miyu-theme.css"
output_path = "~/.config/miyu/webui-theme.css"
```

然后正常运行 matugen(例如 `matugen image /path/to/wallpaper.png`)。

## 工作方式

- miyud 在 `/theme.css` 提供 `~/.config/miyu/webui-theme.css`(如果存在);
- WebUI 在 `styles.css` 之后加载 `/theme.css`,同名 token 覆盖默认值;
- 文件不存在时该请求 404,浏览器静默忽略,WebUI 使用默认的 logo 派生配色。

删除 `~/.config/miyu/webui-theme.css` 并刷新页面即可恢复默认配色。

## 注意

- 模板同时生成亮暗两套 token,WebUI 内的主题切换(晨光 / 夜阑)依然有效;
- WebUI 自有的扩展色(在线状态绿 `--md-ext-color-online`)不随壁纸变化,保持语义稳定。

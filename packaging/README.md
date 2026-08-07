# packaging

各发行版/平台的打包适配，每个平台一个子目录：

- `arch/` — Arch Linux（PKGBUILD，`miyu-git` VCS 包）

## 系统资产约定

除 `/usr/bin/miyu` 外，Miyu 运行时按固定路径查找以下系统资产，
打包时需要一并安装：

| 路径 | 内容 | 来源 | 缺失时的行为 |
|---|---|---|---|
| `/usr/share/miyu/fonts/` | 长回复转图片的渲染字体 | `assets/fonts/` | 长文转图静默退化为纯文本 |
| `/usr/share/miyu/memes/miyu/` | 内置表情库 | `src/memes/miyu/` | 默认人格无内置表情 |
| `/usr/share/miyu/default-kb/` | 默认知识库 | 外部 wiki 仓库（非本仓库），运行时 `miyu update-default-kb` 更新 | 默认知识库为空 |
| `/usr/share/miyu/scripts/` | 系统级脚本 | 目前无打包内容 | 无 |

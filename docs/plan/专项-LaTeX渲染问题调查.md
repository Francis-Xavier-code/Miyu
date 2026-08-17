# 专项 · LaTeX 渲染问题调查（独立于拆分方案）

> 本文件是**独立 bug 调查/修复专项**，不属于 `README.md` 中 Phase 0–9 的代码结构性拆分方案。
> 不要把它混入拆分 Phase；可以独立评审、独立提交、独立回滚。

## 1. 问题描述

报道：除 kitty 外，其他终端的 LaTeX 公式渲染都有问题；表现为**像素块化**，并且**大小限制好像消失了**。

疑问：终端图片渲染不是走 chafa 吗？为什么只有 kitty 正常？

## 2. 结论

LaTeX 公式渲染**不走 chafa**。项目里有两条独立的图片渲染链路：

| 能力 | 路径 |
|---|---|
| 普通图片（`print_image`、`show_meme`、搜图预览） | `src/tools/vision.rs` 的 `print_image_file()`：kitty 原生走 Kitty 图形协议；否则调用外部 `chafa` |
| 块级 LaTeX（`$$...$$` / `\[...\]`） | `src/render/mod.rs` 的 `render_display_math()`：kitty/ghostty 走 Kitty 图形协议；其他终端走**手写半块渲染**（`▀/▄`），不调用 chafa |

所以“其他终端 LaTeX 渲染有问题”和 chafa 无关。

## 3. 根因

### 3.1 非 kitty 路径把公式行数写死为 9

`src/render/mod.rs`：

```rust
math::render_math(tex, math::MathMode::Block, 9, max_cols)
//                                        ^^^ 固定 target_rows = 9
```

`src/render/math.rs` 的 `halfblock_art()`：

```rust
let mut height_px = target_rows * 2;          // 固定 18 像素高
let mut width_px = ...;
if width_px > max_cols.max(4) {
    width_px = max_cols.max(4);               // 只约束宽度
    ...
}
```

造成：

1. **只有宽度上限，没有高度/面积上限**。终端高度、剩余可用行数、`print_image` 插件的 `width_percent/height_percent` 配置全部不参与。
2. **kitty 路径按内容自适应，非 kitty 路径固定 9 行**。
   - kitty：`render_math_kitty()` 按 `raster.width/2`、`raster.height/2` 计算自然大小，再 clamp 到 1..8 行。
   - 其他终端：所有公式都奔着 9 行去。
3. **半块渲染本身颗粒化**。每个字符只能表达 2 个垂直像素，字形只有 `▀ / ▄ / 空格`，没有 chafa 的符号选择和抖动。把 RaTeX 高清 PNG 压进固定 18 像素高，细笔画必然碎成块。

### 3.2 实测尺寸不一致

用当前代码临时探针（未修改仓库）得到：

| 公式 | RaTeX PNG | kitty 路径行数 | 非 kitty 半块行数 |
|---|---|---|---|
| `E=mc^2` | 232×65 | 2 | 9 |
| `q=\frac{1+\sqrt5}{2}\approx 1.618` | 505×144 | 4 | 9 |
| Attention 完整公式 | 1052×185 | 5 | 7 |

`E=mc^2` 这种简单公式在非 kitty 终端被固定放大到 9 行，就是“大小限制没了”的直接来源。宽度其实仍受 `max_cols` 限制（24..110），但垂直方向没有任何约束。

### 3.3 附带问题：kitty 协议检测过窄

`src/render/math.rs` 的 `kitty_graphics_supported()` 只认：

- `TERM == xterm-kitty`
- `TERM` 包含 `ghostty`
- 存在 `GHOSTTY_RESOURCES_DIR`

像 WezTerm 这类**支持 Kitty 图形协议**但 `TERM` 不为 `xterm-kitty` 的终端，也会掉进半块路径。

## 4. 修复方案（三选一或组合）

### 方案 A：非 kitty 也改用 chafa（推荐）

复用普通图片的成熟路径：

1. `render_display_math()` 生成 RaTeX PNG 后，非 kitty 分支调用 `chafa`。
2. 使用 `chafa --format symbols --size <max_cols>x<max_rows> -`，从 stdin 读 PNG，避免临时文件。
3. `max_cols/max_rows` 由终端尺寸和 `print_image` 插件百分比计算，复用 `configured_print_size()` 的思路。
4. `chafa` 不存在时回退到现有半块渲染。

优点：

- 非 kitty 也有真正的宽高限制。
- 显示质量与 `print_image` 一致。
- 不改变 kitty 路径。

风险：

- 每渲染一个公式多一次外部进程调用；需要确认流式渲染/raw mode 下输出兼容。
- `chafa` 是运行时依赖，打包仍按现有方式提供。

### 方案 B：最小纯 Rust 修复

保留半块，但把 `target_rows` 从固定 9 改为终端相关：

```text
natural_rows = clamp(raster.height / 2 / cell_height, min_rows, max_rows)
max_rows     = clamp(terminal_rows - 预留行, 2, 8)
```

并修改 `render_math()` 与 `halfblock_art()` 同时接受 `max_rows`。

优点：无新增外部进程。
缺点：半块颗粒感仍在，只是尺寸合理了。

### 方案 C：扩大 kitty 协议检测

把支持 Kitty 图形协议的终端纳入高清路径，例如 WezTerm（`TERM=wezterm` 或存在 `WEZTERM_PANE`）。

优点：能走高清的终端不再误入半块。
缺点：不能解决真正不支持图形协议的终端。

推荐组合：**C + A**（能走 Kitty 协议的走协议；其他终端走 chafa；chafa 缺失才半块兜底）。

## 5. 验证清单

- [ ] 在 kitty 中渲染简单/复杂公式，行为与修复前一致。
- [ ] 在 `xterm-256color`/`tmux`/WezTerm 中渲染同一公式：
  - [ ] 输出行数随内容与终端大小变化；
  - [ ] 不超过配置的宽高限制；
  - [ ] 不再出现简单公式 9 行、复杂公式被压成碎块。
- [ ] `cargo test` 全绿；`render/math.rs` 现有 PTY 安全测试继续通过。
- [ ] raw mode / 流式渲染 / REPL 重绘 / tmux 滚动回看各跑一遍。
- [ ] chafa 缺失时回退路径不 panic。

## 6. 边界声明

- 本专项**不改** `docs/plan/README.md` 的拆分阶段顺序。
- 若与拆分并行执行，本专项在独立分支独立提交，禁止和模块拆分混在同一个 PR。
- 修复完成后从 `todolist.md` 移除“latex 渲染问题”条目。

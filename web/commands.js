// WebUI 的斜杠命令层。
//
// 单独一个文件而不是塞进 app.js：后者已经 9500 行，再往里长就没人找得到东西了。
// 这里自己管住命令目录、`/` 补全菜单、以及「这一行是不是命令」的判定；app.js
// 只留三个挂钩（启动时 load、输入时 onInput、提交时 tryRun）。
//
// 命令清单来自 `GET /api/commands`，服务端从 REPL 那张同一张表里按 `web` 标记
// 过滤。前端**不**维护第二份——两份清单迟早分叉（加一条命令忘了改另一边）。
window.MiyuCommands = (() => {
  "use strict";

  let catalog = [];
  let menu = null;
  let highlighted = 0;
  let onPick = null;

  // 与 Rust 侧 `split_repl_command` 同语义：按第一个空白切成 (名字, 参数)。
  function split(input) {
    const text = String(input ?? "");
    const at = text.search(/\s/);
    if (at < 0) return [text, ""];
    return [text.slice(0, at), text.slice(at + 1)];
  }

  // 只认**完整**命令名。不命中就不是命令，照常发给模型——与 REPL 同一语义
  // （`slash_commands::parse_repl_input`）。两个界面在这件事上分叉的话，同一
  // 句 `/home/x 这是什么` 在一边能发出去、在另一边被吞掉。
  function match(input) {
    const text = String(input ?? "").trim();
    if (!text.startsWith("/")) return null;
    const [name] = split(text);
    const lowered = name.toLowerCase();
    return catalog.find((spec) => spec.name === lowered) || null;
  }

  // 补全候选：前缀匹配，只在菜单里用。回车执行走 match()，不做前缀展开。
  function suggestions(input) {
    const text = String(input ?? "");
    if (!text.startsWith("/") || /\s/.test(text)) return [];
    const lowered = text.toLowerCase();
    return catalog.filter((spec) => spec.name.startsWith(lowered));
  }

  async function load(apiRequest) {
    try {
      const response = await apiRequest("/api/commands");
      const payload = await response.json();
      catalog = Array.isArray(payload?.commands) ? payload.commands : [];
    } catch (_) {
      // 拿不到目录就退化成「没有命令」：所有 / 开头的输入照常发给模型。
      catalog = [];
    }
    return catalog.length;
  }

  function ensureMenu(anchor) {
    if (menu) return menu;
    menu = document.createElement("div");
    menu.className = "commandMenu";
    menu.hidden = true;
    menu.setAttribute("role", "listbox");
    anchor.appendChild(menu);
    return menu;
  }

  function hide() {
    if (menu) menu.hidden = true;
    highlighted = 0;
  }

  function visibleItems() {
    return menu && !menu.hidden ? Array.from(menu.children) : [];
  }

  function render(items) {
    if (!menu) return;
    if (!items.length) {
      hide();
      return;
    }
    highlighted = Math.min(highlighted, items.length - 1);
    menu.replaceChildren();
    items.forEach((spec, index) => {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "commandMenuItem";
      row.dataset.name = spec.name;
      if (index === highlighted) row.classList.add("isActive");
      const label = document.createElement("span");
      label.className = "commandMenuName";
      label.textContent = spec.arg_hint ? `${spec.name} ${spec.arg_hint}` : spec.name;
      const help = document.createElement("span");
      help.className = "commandMenuHelp";
      help.textContent = spec.help || "";
      row.append(label, help);
      // mousedown 而不是 click：click 之前输入框会先失焦，菜单已经关掉了。
      row.addEventListener("mousedown", (event) => {
        event.preventDefault();
        if (onPick) onPick(spec.name);
        hide();
      });
      menu.appendChild(row);
    });
    menu.hidden = false;
  }

  // 输入变化时刷新菜单。`anchor` 是菜单挂靠的容器，`pick` 是选中后回填输入框。
  function onInput(value, anchor, pick) {
    onPick = pick;
    ensureMenu(anchor);
    render(suggestions(value));
  }

  // 菜单开着时接管上下键与 Tab/Enter。返回 true 表示这次按键已被吃掉。
  function handleKey(event) {
    const items = visibleItems();
    if (!items.length) return false;
    if (event.key === "Escape") {
      hide();
      return true;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const step = event.key === "ArrowDown" ? 1 : -1;
      highlighted = (highlighted + step + items.length) % items.length;
      items.forEach((item, index) => item.classList.toggle("isActive", index === highlighted));
      return true;
    }
    // Tab 补全；Enter 只在菜单开着且候选唯一/已选中时补全，不直接执行——
    // 与 REPL 一致：补全后再按一次回车才执行，用户有机会反悔。
    if (event.key === "Tab" || event.key === "Enter") {
      const picked = items[highlighted];
      if (!picked) return false;
      if (onPick) onPick(picked.dataset.name);
      hide();
      return true;
    }
    return false;
  }

  // 执行一条命令。`ctx` 提供 { apiRequest, sessionId, mode, notify, confirm, clearView }。
  // 返回 true 表示已处理（调用方应当清空输入框、不再当消息发）。
  async function tryRun(input, ctx) {
    const spec = match(input);
    if (!spec) return false;
    const [, args] = split(String(input).trim());
    if (!spec.arg_hint && args.trim()) {
      ctx.notify(`${spec.name} 不接受参数`, "error");
      return true;
    }
    try {
      if (spec.name === "/compact") {
        await ctx.apiRequest("/api/conversation/compact", {
          method: "POST",
          body: JSON.stringify({ session_id: ctx.sessionId }),
        });
        ctx.notify("已压缩当前会话上下文");
        return true;
      }
      if (spec.name === "/reset-memory") {
        if (!(await ctx.confirm("清空当前模式的长期记忆？此操作不可撤销。"))) return true;
        await ctx.apiRequest("/api/memory/reset", {
          method: "POST",
          body: JSON.stringify({ mode: ctx.mode }),
        });
        ctx.notify("已清空长期记忆");
        return true;
      }
    } catch (error) {
      ctx.notify(error?.message || "命令执行失败", "error");
      return true;
    }
    // 目录里有、这里却没实现：说明服务端开了 web 标记但前端没接上。
    // 当成命令吃掉并报错，比静默发给模型强——后者会让用户以为命令生效了。
    ctx.notify(`${spec.name} 在 WebUI 里还没有实现`, "error");
    return true;
  }

  return { load, split, match, suggestions, onInput, handleKey, hide, tryRun };
})();

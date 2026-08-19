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
    const line = String(input).trim();
    const [, args] = split(line);
    const done = (text, tone) => {
      note(line, text, tone, ctx.anchorTurnId);
      ctx.redraw();
      return true;
    };
    if (!spec.arg_hint && args.trim()) {
      return done(`${spec.name} 不接受参数`, "error");
    }
    try {
      if (spec.name === "/compact") {
        const response = await ctx.apiRequest("/api/conversation/compact", {
          method: "POST",
          body: JSON.stringify({ session_id: ctx.sessionId }),
        });
        // compact_now 返回 Option。它**不是**被水位线拦着——手动 /compact
        // 已经是 force（agent/mod.rs 跳过了「折叠够不够本」那道闸）。真正的
        // None 出口在 compact.rs 的 `cut == 0`：压缩是把**比保留尾巴更老**的
        // 回合折成摘要，整段对话都还在尾巴以内时，没有更老的可折。
        // 尾巴预算 = min(16384, 窗口/4)，1M 窗口下就是 16k。
        const compacted = (await response.json())?.result?.compacted === true;
        return done(compacted ? "上下文已压缩" : "当前上下文过少");
      }
      if (spec.name === "/reset") {
        await ctx.apiRequest("/api/conversation/reset", {
          method: "POST",
          body: JSON.stringify({ session_id: ctx.sessionId }),
        });
        // 清空之后对话流整段作废，让调用方重新拉一次；回执本身靠锚点回合定位，
        // 而那个回合刚被删掉，anchorOf 找不到就退回末尾，正好。
        await ctx.reload();
        return done("已清空当前会话");
      }
      if (spec.name === "/goal") {
        const response = await ctx.apiRequest("/api/goal", {
          method: "POST",
          body: JSON.stringify({ session_id: ctx.sessionId, input: args }),
        });
        // 服务端把整段人类可读的回执拼好（和 REPL 同一份实现），这里原样贴出，
        // 不在前端二次拼装——两边分叉的话，同一个 /goal 在两个界面说法不一样。
        const text = (await response.json())?.text || "";
        return done(text || "目标命令已执行");
      }
      if (spec.name === "/reset-memory") {
        await ctx.apiRequest("/api/memory/reset", {
          method: "POST",
          body: JSON.stringify({ mode: ctx.mode }),
        });
        return done("已清空长期记忆");
      }
    } catch (error) {
      return done(error?.message || "命令执行失败", "error");
    }
    // 目录里有、这里却没实现：说明服务端开了 web 标记但前端没接上。
    // 当成命令吃掉并报错，比静默发给模型强——后者会让用户以为命令生效了。
    return done(`${spec.name} 在 WebUI 里还没有实现`, "error");
  }

  // ── 命令回执：像消息一样留在对话流里 ─────────────────────────────
  //
  // 命令不是回合，不能落库、更不能进模型上下文（它是客户端操作）。但只弹一个
  // 转瞬即逝的 toast 也不对：用户敲了字、按了回车，对话流里却什么都没发生，
  // 看起来就像没生效。REPL 那边是 `repl_note` 写进滚动区，这里对齐。
  //
  // 存成数组而不是直接往 DOM 里塞：`renderConversation` 每次都从 state.turns
  // 重建整个 timeline，直接塞的节点会被冲掉——而 /compact 成功后正好会触发
  // 一次重建。换会话时清空（`clearNotices`），与 REPL 滚动区同寿命。
  const notices = [];

  // `anchorTurnId` = 敲这条命令时对话流里最后一个回合。渲染时插到那个回合
  // **之后**，而不是无脑 append 到末尾——否则之后来的新回合会把回执顶下去，
  // 看起来像是「先说话后执行的命令」，时间顺序全乱。
  function note(command, text, tone, anchorTurnId) {
    notices.push({
      command,
      text,
      tone: tone || "info",
      anchorTurnId: anchorTurnId ? String(anchorTurnId) : "",
    });
  }

  function clearNotices() {
    notices.length = 0;
  }

  function renderNotices(timeline) {
    if (!timeline || !notices.length) return;
    // 锚点回合的最后一个 DOM 节点；找不到（回合被 undo/pop 掉了，或者当时
    // 对话是空的）就退回末尾。
    const anchorOf = (turnId) => {
      if (!turnId) return null;
      const nodes = timeline.querySelectorAll(`[data-turn-id="${CSS.escape(turnId)}"]`);
      return nodes.length ? nodes[nodes.length - 1] : null;
    };
    for (const entry of notices) {
      const echo = document.createElement("article");
      echo.className = "message user-message commandEcho";
      echo.dataset.role = "user";
      const bubble = document.createElement("div");
      bubble.className = "user-bubble";
      const line = document.createElement("p");
      line.textContent = entry.command;
      bubble.appendChild(line);
      echo.appendChild(bubble);

      // 复用系统事件那套（后台任务完成用的就是它）：居中、带底色的小条，
      // 而不是一行裸文字——它不是谁说的话，是一次操作的回执。
      const reply = document.createElement("div");
      reply.className = "system-event is-command-result";
      if (entry.tone === "error") reply.classList.add("is-error");
      const label = document.createElement("span");
      label.textContent = entry.text;
      reply.appendChild(label);

      const anchor = anchorOf(entry.anchorTurnId);
      if (anchor) {
        anchor.after(echo, reply);
      } else {
        timeline.append(echo, reply);
      }
    }
    timeline.hidden = false;
  }

  return {
    load,
    split,
    match,
    suggestions,
    onInput,
    handleKey,
    hide,
    tryRun,
    renderNotices,
    clearNotices,
  };
})();

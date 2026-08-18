"use strict";

/*
 * 任务列表面板。
 *
 * `todowrite` / `todoupdate` 的输出是一坨 JSON。REPL 那边有 `write_todo_table`
 * 专门画成表格,WebUI 一直没有对应的东西——列表折在工具签里,不展开根本看不见,
 * 而 AI 列的待办本来就是给人看的产出,不是调试信息。
 *
 * 这里把那坨 JSON 画成一张卡片,挂在工具签下方(收起态也可见)。
 * 单独成文件:app.js 已经九千多行。
 */
window.MiyuTodos = (() => {
  const STATUS_ORDER = ["in_progress", "pending", "completed", "cancelled"];
  const STATUS_LABEL = {
    pending: "待办",
    in_progress: "进行中",
    completed: "已完成",
    cancelled: "已取消",
  };

  function isTodoTool(name) {
    const value = String(name || "").toLowerCase();
    return value === "todowrite" || value === "todoupdate";
  }

  /// 从工具输出里取出待办数组。取不到就返回 null,调用方照常走原来的路。
  function parse(output) {
    const text = String(output || "").trim();
    if (!text.startsWith("{")) return null;
    let payload;
    try {
      payload = JSON.parse(text);
    } catch (_) {
      return null;
    }
    if (!Array.isArray(payload?.todos)) return null;
    const todos = payload.todos.flatMap((item) => {
      const content = String(item?.content ?? item?.task ?? "").trim();
      if (!content) return [];
      const status = String(item?.status || "pending").toLowerCase();
      return [{ content, status, priority: String(item?.priority || "").trim() }];
    });
    if (!todos.length) return null;
    return todos;
  }

  function statusRank(status) {
    const index = STATUS_ORDER.indexOf(status);
    return index === -1 ? STATUS_ORDER.length : index;
  }

  function render(output) {
    const todos = parse(output);
    if (!todos) return null;

    const panel = document.createElement("div");
    panel.className = "todo-panel";

    const head = document.createElement("div");
    head.className = "todo-panel-head";
    const done = todos.filter((todo) => todo.status === "completed").length;
    const title = document.createElement("strong");
    title.textContent = "任务列表";
    const count = document.createElement("small");
    count.textContent = `${done} / ${todos.length}`;
    head.append(title, count);
    panel.appendChild(head);

    const list = document.createElement("ol");
    list.className = "todo-list";
    // 进行中的排最前——那是「现在在干什么」,列表存在的意义。
    for (const todo of [...todos].sort((a, b) => statusRank(a.status) - statusRank(b.status))) {
      const item = document.createElement("li");
      item.className = `todo-item is-${todo.status}`;
      const mark = document.createElement("span");
      mark.className = "todo-mark";
      mark.setAttribute("aria-hidden", "true");
      const text = document.createElement("span");
      text.className = "todo-text";
      text.textContent = todo.content;
      const status = document.createElement("small");
      status.className = "todo-status";
      status.textContent = STATUS_LABEL[todo.status] || todo.status;
      item.append(mark, text, status);
      list.appendChild(item);
    }
    panel.appendChild(list);
    return panel;
  }

  return { isTodoTool, parse, render };
})();

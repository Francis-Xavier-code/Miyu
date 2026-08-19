"use strict";

/*
 * 文件分享面板。
 *
 * 与 artifact 预览区是两个独立概念:artifact 是「演示回合产出」,这里是
 * 「把本机文件递给局域网里的人」的持续清单。数据全部来自 /api/shared,
 * 凭 WebUI 登录态访问;视频/音频/图片行内预览,其余只给下载。
 * 单独成文件:app.js 已经九千多行。
 */
window.MiyuShared = (() => {
  const KIND_ICON = { video: "🎬", audio: "🎵", image: "🖼", other: "📄" };
  const MODE_LABEL = { reference: "引用", snapshot: "快照" };
  let panel = null;
  let listBox = null;

  function formatSize(bytes) {
    const value = Number(bytes) || 0;
    if (value >= 1024 * 1024 * 1024) return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
    if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
    if (value >= 1024) return `${(value / 1024).toFixed(1)} KiB`;
    return `${value} B`;
  }

  function downloadUrl(id) {
    return `${location.origin}/api/shared/${encodeURIComponent(id)}?download=1`;
  }

  /* http 局域网源没有 navigator.clipboard,退回 execCommand。 */
  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    const scratch = document.createElement("textarea");
    scratch.value = text;
    scratch.style.position = "fixed";
    scratch.style.opacity = "0";
    document.body.appendChild(scratch);
    scratch.select();
    try {
      document.execCommand("copy");
    } finally {
      scratch.remove();
    }
    return Promise.resolve();
  }

  function ensurePanel() {
    if (panel) return panel;
    panel = document.createElement("div");
    panel.className = "shared-files-overlay";
    panel.hidden = true;
    panel.innerHTML = `
      <div class="shared-files-panel" role="dialog" aria-label="分享文件">
        <header class="shared-files-header">
          <strong>分享文件</strong>
          <span class="shared-files-hint">局域网内能打开本 WebUI 的人都可下载</span>
          <button type="button" class="shared-files-refresh" title="刷新">↻</button>
          <button type="button" class="shared-files-close" title="关闭">×</button>
        </header>
        <div class="shared-files-list"></div>
      </div>`;
    panel.addEventListener("click", (event) => {
      if (event.target === panel) hide();
    });
    panel.querySelector(".shared-files-close").addEventListener("click", hide);
    panel.querySelector(".shared-files-refresh").addEventListener("click", refresh);
    listBox = panel.querySelector(".shared-files-list");
    document.body.appendChild(panel);
    return panel;
  }

  function hide() {
    if (panel) panel.hidden = true;
  }

  async function refresh() {
    ensurePanel();
    listBox.textContent = "加载中…";
    let payload;
    try {
      const response = await fetch("/api/shared");
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      payload = await response.json();
    } catch (error) {
      listBox.textContent = `加载失败:${error.message || error}`;
      return;
    }
    render(Array.isArray(payload?.shares) ? payload.shares : []);
  }

  function render(shares) {
    listBox.textContent = "";
    if (!shares.length) {
      const empty = document.createElement("p");
      empty.className = "shared-files-empty";
      empty.textContent = "还没有分享任何文件。让 AI 调用 share_file 即可分享。";
      listBox.appendChild(empty);
      return;
    }
    for (const share of shares) listBox.appendChild(renderRow(share));
  }

  function renderRow(share) {
    const row = document.createElement("div");
    row.className = "shared-files-row";
    const url = downloadUrl(share.share_id);

    const head = document.createElement("div");
    head.className = "shared-files-row-head";
    const icon = document.createElement("span");
    icon.className = "shared-files-icon";
    icon.textContent = KIND_ICON[share.kind] || KIND_ICON.other;
    const name = document.createElement("span");
    name.className = "shared-files-name";
    name.textContent = share.title || share.file_name;
    name.title = share.file_name;
    const meta = document.createElement("span");
    meta.className = "shared-files-meta";
    meta.textContent = `${formatSize(share.size_bytes)} · ${MODE_LABEL[share.mode] || share.mode}`;
    head.append(icon, name, meta);

    const actions = document.createElement("div");
    actions.className = "shared-files-actions";
    if (share.kind === "video" || share.kind === "audio" || share.kind === "image") {
      actions.appendChild(actionButton("预览", () => togglePreview(row, share)));
    }
    actions.appendChild(actionButton("复制链接", async (button) => {
      await copyText(url);
      const label = button.textContent;
      button.textContent = "已复制";
      setTimeout(() => { button.textContent = label; }, 1200);
    }));
    actions.appendChild(actionButton("下载", () => { window.open(url, "_blank"); }));
    actions.appendChild(actionButton("删除", async () => {
      if (!window.confirm(`删除分享「${share.file_name}」?`)) return;
      await fetch(`/api/shared/${encodeURIComponent(share.share_id)}`, { method: "DELETE" });
      refresh();
    }, "danger"));

    row.append(head, actions);
    return row;
  }

  function actionButton(label, onClick, extraClass) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `shared-files-action${extraClass ? ` is-${extraClass}` : ""}`;
    button.textContent = label;
    button.addEventListener("click", () => onClick(button));
    return button;
  }

  /* 行内预览:再点一次收起。src 不带 download=1,Range 由后端支持,可拖进度条。 */
  function togglePreview(row, share) {
    const existing = row.querySelector(".shared-files-preview");
    if (existing) {
      existing.remove();
      return;
    }
    const box = document.createElement("div");
    box.className = "shared-files-preview";
    const src = `/api/shared/${encodeURIComponent(share.share_id)}`;
    let media;
    if (share.kind === "video") {
      media = document.createElement("video");
      media.controls = true;
      media.preload = "metadata";
    } else if (share.kind === "audio") {
      media = document.createElement("audio");
      media.controls = true;
      media.preload = "metadata";
    } else {
      media = document.createElement("img");
      media.alt = share.file_name;
      media.loading = "lazy";
    }
    media.src = src;
    box.appendChild(media);
    row.appendChild(box);
  }

  function toggle() {
    ensurePanel();
    if (panel.hidden) {
      panel.hidden = false;
      refresh();
    } else {
      panel.hidden = true;
    }
  }

  function isShareTool(name) {
    const value = String(name || "").toLowerCase();
    return value === "share_file" || value.startsWith("share_file:");
  }

  /*
   * 气泡内附件卡片:share_file 成功后直接挂在工具签下方(收起态也可见)。
   * 视频/音频/图片在气泡里内联预览;所有类型点击文件行即直接下载,
   * 不需要让 AI 转述链接再复制访问。面板列表只服务批量管理场景。
   */
  function renderCard(output) {
    const text = String(output || "").trim();
    if (!text.startsWith("{")) return null;
    let payload;
    try {
      payload = JSON.parse(text);
    } catch (_) {
      return null;
    }
    if (payload?.status !== "ok" || !payload.share_id || !payload.file_name) return null;
    const card = document.createElement("div");
    card.className = "shared-attachment";
    const kind = String(payload.kind || "other");
    const src = `/api/shared/${encodeURIComponent(payload.share_id)}`;
    if (kind === "video" || kind === "audio" || kind === "image") {
      const box = document.createElement("div");
      box.className = "shared-attachment-preview";
      let media;
      if (kind === "video") {
        media = document.createElement("video");
        media.controls = true;
        media.preload = "metadata";
      } else if (kind === "audio") {
        media = document.createElement("audio");
        media.controls = true;
        media.preload = "metadata";
      } else {
        media = document.createElement("img");
        media.alt = payload.file_name;
        media.loading = "lazy";
      }
      media.src = src;
      box.appendChild(media);
      card.appendChild(box);
    }
    const row = document.createElement("a");
    row.className = "shared-attachment-row";
    row.href = `${src}?download=1`;
    row.setAttribute("download", payload.file_name);
    row.title = "下载";
    const icon = document.createElement("span");
    icon.className = "shared-attachment-icon";
    icon.textContent = KIND_ICON[kind] || KIND_ICON.other;
    const name = document.createElement("span");
    name.className = "shared-attachment-name";
    name.textContent = payload.file_name;
    const meta = document.createElement("span");
    meta.className = "shared-attachment-meta";
    meta.textContent = `${formatSize(payload.size_bytes)} · 点击下载`;
    row.append(icon, name, meta);
    card.appendChild(row);
    return card;
  }

  document.addEventListener("DOMContentLoaded", () => {
    const button = document.getElementById("sharedFilesButton");
    if (button) button.addEventListener("click", toggle);
  });

  return { toggle, refresh, isShareTool, renderCard };
})();

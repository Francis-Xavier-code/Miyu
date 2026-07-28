(() => {
  "use strict";

  const MAX_CONTENT_CHARS = 20_000;
  const MAX_CUSTOM_ANSWER_CHARS = 4_000;
  const MAX_TOOL_OUTPUT_CHARS = 200_000;
  const NEAR_BOTTOM_PX = 120;
  const DEFAULT_BOARD_TITLE = "今天想聊些什么？";
  const DEFAULT_BOARD_SUBTITLE = "从一个问题、计划或此刻的想法开始。";
  const DEFAULT_STARTER_PROMPTS = ["查询今天的天气", "分析一个问题", "发表情包打个招呼吧", "搜索一张图片"];

  const SVG_NS = "http://www.w3.org/2000/svg";
  const ICONS = {
    "arrow-down": [["path", { d: "M12 5v14" }], ["path", { d: "m19 12-7 7-7-7" }]],
    "arrow-up": [["path", { d: "m5 12 7-7 7 7" }], ["path", { d: "M12 19V5" }]],
    atom: [["circle", { cx: "12", cy: "12", r: "1" }], ["path", { d: "M20.2 20.2c2.04-2.03.02-7.37-4.5-11.9-4.52-4.52-9.87-6.54-11.9-4.5-2.04 2.03-.02 7.37 4.5 11.9 4.52 4.52 9.87 6.54 11.9 4.5Z" }], ["path", { d: "M15.7 15.7c4.52-4.52 6.54-9.87 4.5-11.9-2.03-2.04-7.37-.02-11.9 4.5-4.52 4.52-6.54 9.87-4.5 11.9 2.03 2.04 7.37.02 11.9-4.5Z" }]],
    check: [["path", { d: "M20 6 9 17l-5-5" }]],
    "chevron-down": [["path", { d: "m6 9 6 6 6-6" }]],
    "chevron-right": [["path", { d: "m9 18 6-6-6-6" }]],
    "circle-alert": [["circle", { cx: "12", cy: "12", r: "10" }], ["line", { x1: "12", x2: "12", y1: "8", y2: "12" }], ["line", { x1: "12", x2: "12.01", y1: "16", y2: "16" }]],
    "circle-help": [["circle", { cx: "12", cy: "12", r: "10" }], ["path", { d: "M9.09 9a3 3 0 1 1 5.83 1c0 2-3 3-3 3" }], ["path", { d: "M12 17h.01" }]],
    "circle-stop": [["circle", { cx: "12", cy: "12", r: "10" }], ["rect", { width: "6", height: "6", x: "9", y: "9", rx: "1" }]],
    "cloud-sun": [["path", { d: "M12 2v2" }], ["path", { d: "m4.93 4.93 1.41 1.41" }], ["path", { d: "M20 12h2" }], ["path", { d: "m19.07 4.93-1.41 1.41" }], ["path", { d: "M16 6a4 4 0 0 0-3.46 6" }], ["path", { d: "M17.5 19H9a4 4 0 1 1 3.68-5.57A3 3 0 1 1 17.5 19Z" }]],
    copy: [["rect", { width: "14", height: "14", x: "8", y: "8", rx: "2", ry: "2" }], ["path", { d: "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" }]],
    download: [["path", { d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" }], ["polyline", { points: "7 10 12 15 17 10" }], ["line", { x1: "12", x2: "12", y1: "15", y2: "3" }]],
    ellipsis: [["circle", { cx: "12", cy: "12", r: "1" }], ["circle", { cx: "19", cy: "12", r: "1" }], ["circle", { cx: "5", cy: "12", r: "1" }]],
    "external-link": [["path", { d: "M15 3h6v6" }], ["path", { d: "M10 14 21 3" }], ["path", { d: "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" }]],
    folder: [["path", { d: "M3 6a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" }]],
    "trash-2": [["path", { d: "M3 6h18" }], ["path", { d: "M8 6V4h8v2" }], ["path", { d: "M19 6 18 20H6L5 6" }], ["path", { d: "M10 11v5" }], ["path", { d: "M14 11v5" }]],
    fileTerminal: [["path", { d: "M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" }], ["polyline", { points: "14 2 14 8 20 8" }], ["path", { d: "m8 13 2 2-2 2" }], ["path", { d: "M12 17h4" }]],
    lightbulb: [["path", { d: "M9 18h6" }], ["path", { d: "M10 22h4" }], ["path", { d: "M15.09 14c.18-.59.59-1.05 1.05-1.52A6 6 0 1 0 7.86 12.5c.45.44.85.9 1.03 1.5" }], ["path", { d: "M9 14h6v1a3 3 0 0 1-6 0v-1Z" }]],
    "list-todo": [["rect", { x: "3", y: "5", width: "6", height: "6", rx: "1" }], ["path", { d: "m3 17 2 2 4-4" }], ["path", { d: "M13 6h8" }], ["path", { d: "M13 12h8" }], ["path", { d: "M13 18h8" }]],
    "loader-circle": [["path", { d: "M21 12a9 9 0 1 1-6.219-8.56" }]],
    "lock-keyhole": [["circle", { cx: "12", cy: "16", r: "1" }], ["rect", { x: "3", y: "10", width: "18", height: "12", rx: "2" }], ["path", { d: "M7 10V7a5 5 0 0 1 10 0v3" }]],
    "log-in": [["path", { d: "M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4" }], ["polyline", { points: "10 17 15 12 10 7" }], ["line", { x1: "15", x2: "3", y1: "12", y2: "12" }]],
    "message-circle": [["path", { d: "M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z" }]],
    "messages-square": [["path", { d: "M14 9a2 2 0 0 1-2 2H6l-4 4V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2z" }], ["path", { d: "M18 9h2a2 2 0 0 1 2 2v10l-4-4h-6a2 2 0 0 1-2-2v-1" }]],
    moon: [["path", { d: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" }]],
    "image-search": [["rect", { x: "3", y: "3", width: "14", height: "14", rx: "2" }], ["circle", { cx: "11", cy: "9", r: "2" }], ["path", { d: "m3 15 4-4 5 5" }], ["circle", { cx: "18", cy: "18", r: "3" }], ["path", { d: "m20.2 20.2 1.8 1.8" }]],
    "panel-left": [["rect", { width: "18", height: "18", x: "3", y: "3", rx: "2" }], ["path", { d: "M9 3v18" }]],
    "refresh-cw": [["path", { d: "M21 12a9 9 0 0 0-15.35-6.35L3 8" }], ["path", { d: "M3 3v5h5" }], ["path", { d: "M3 12a9 9 0 0 0 15.35 6.35L21 16" }], ["path", { d: "M16 16h5v5" }]],
    route: [["circle", { cx: "6", cy: "19", r: "3" }], ["path", { d: "M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15" }], ["circle", { cx: "18", cy: "5", r: "3" }]],
    "settings-2": [["path", { d: "M20 7h-9" }], ["path", { d: "M14 17H5" }], ["circle", { cx: "17", cy: "17", r: "3" }], ["circle", { cx: "7", cy: "7", r: "3" }]],
    "sliders-horizontal": [["line", { x1: "21", x2: "14", y1: "4", y2: "4" }], ["line", { x1: "10", x2: "3", y1: "4", y2: "4" }], ["line", { x1: "21", x2: "12", y1: "12", y2: "12" }], ["line", { x1: "8", x2: "3", y1: "12", y2: "12" }], ["line", { x1: "21", x2: "16", y1: "20", y2: "20" }], ["line", { x1: "12", x2: "3", y1: "20", y2: "20" }], ["line", { x1: "14", x2: "14", y1: "2", y2: "6" }], ["line", { x1: "8", x2: "8", y1: "10", y2: "14" }], ["line", { x1: "16", x2: "16", y1: "18", y2: "22" }]],
    sparkles: [["path", { d: "m12 3-1.9 5.8a2 2 0 0 1-1.3 1.3L3 12l5.8 1.9a2 2 0 0 1 1.3 1.3L12 21l1.9-5.8a2 2 0 0 1 1.3-1.3L21 12l-5.8-1.9a2 2 0 0 1-1.3-1.3Z" }], ["path", { d: "M5 3v4" }], ["path", { d: "M19 17v4" }], ["path", { d: "M3 5h4" }], ["path", { d: "M17 19h4" }]],
    smile: [["circle", { cx: "12", cy: "12", r: "9" }], ["path", { d: "M8 14s1.5 2 4 2 4-2 4-2" }], ["path", { d: "M9 9h.01" }], ["path", { d: "M15 9h.01" }]],
    "stop-square": [["rect", { x: "6", y: "6", width: "12", height: "12", rx: "2", fill: "currentColor", stroke: "none" }]],
    "square-pen": [["path", { d: "M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" }], ["path", { d: "M18.37 2.63a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4Z" }]],
    sun: [["circle", { cx: "12", cy: "12", r: "4" }], ["path", { d: "M12 2v2" }], ["path", { d: "M12 20v2" }], ["path", { d: "m4.93 4.93 1.42 1.42" }], ["path", { d: "m17.66 17.66 1.41 1.41" }], ["path", { d: "M2 12h2" }], ["path", { d: "M20 12h2" }], ["path", { d: "m6.34 17.66-1.41 1.41" }], ["path", { d: "m19.07 4.93-1.41 1.41" }]],
    "sun-moon": [["path", { d: "M12 8a2.83 2.83 0 0 0 4 4 4 4 0 1 1-4-4" }], ["path", { d: "M12 2v2" }], ["path", { d: "M12 20v2" }], ["path", { d: "m4.9 4.9 1.4 1.4" }], ["path", { d: "m17.7 17.7 1.4 1.4" }], ["path", { d: "M2 12h2" }], ["path", { d: "M20 12h2" }], ["path", { d: "m6.3 17.7-1.4 1.4" }], ["path", { d: "m19.1 4.9-1.4 1.4" }]],
    "triangle-alert": [["path", { d: "m21.73 18-8-14a2 2 0 0 0-3.46 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" }], ["path", { d: "M12 9v4" }], ["path", { d: "M12 17h.01" }]],
    wrench: [["path", { d: "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94z" }]],
    x: [["path", { d: "M18 6 6 18" }], ["path", { d: "m6 6 12 12" }]]
  };

  const EVENT_NAMES = [
    "run.started",
    "turn.started",
    "assistant.delta",
    "reasoning.start",
    "reasoning.reset",
    "reasoning.part_start",
    "reasoning.part_end",
    "reasoning.title",
    "reasoning.delta",
    "tool.started",
    "tool.progress",
    "tool.output",
    "tool.image",
    "tool.finished",
    "question.requested",
    "question.answered",
    "context.compact_start",
    "context.compact_delta",
    "context.compact_end",
    "context.pop_start",
    "context.pop_end",
    "context.error",
    "queue.added",
    "queue.removed",
    "queue.consumed",
    "run.completed",
    "run.cancelled",
    "run.failed",
    "conversation.reset",
    "conversation.pop",
    "session.created",
    "session.renamed",
    "session.archived",
    "session.deleted",
    "session.current_changed",
    "session.updated",
    "resync_required"
  ];

  const RUN_EVENTS = new Set(EVENT_NAMES.filter((name) => !name.startsWith("session.") && !["conversation.reset", "conversation.pop", "resync_required", "queue.added", "queue.removed"].includes(name)));

  const elements = {
    body: document.body,
    sidebar: document.getElementById("sidebar"),
    sidebarScrim: document.getElementById("sidebarScrim"),
    sidebarClose: document.getElementById("sidebarClose"),
    mobileMenuButton: document.getElementById("mobileMenuButton"),
    sidebarStatusDot: document.getElementById("sidebarStatusDot"),
    sidebarConnectionStatus: document.getElementById("sidebarConnectionStatus"),
    newChatButton: document.getElementById("newChatButton"),
    matugenThemeLink: document.getElementById("matugenThemeLink"),
    reasoningExpandToggle: document.getElementById("reasoningExpandToggle"),
    toolExpandToggle: document.getElementById("toolExpandToggle"),
    sessionList: document.getElementById("sessionList"),
    sessionItems: document.getElementById("sessionItems"),
    archivedSection: document.getElementById("archivedSection"),
    archivedToggle: document.getElementById("archivedToggle"),
    archivedList: document.getElementById("archivedList"),
    contextNumbers: document.getElementById("contextNumbers"),
    contextTrack: document.getElementById("contextTrack"),
    contextBar: document.getElementById("contextBar"),
    settingsButton: document.getElementById("settingsButton"),
    sidebarThemeButton: document.getElementById("sidebarThemeButton"),
    brandAvatar: document.getElementById("brandAvatar"),
    brandName: document.getElementById("brandName"),
    conversationTitle: document.getElementById("conversationTitle"),
    conversationMeta: document.getElementById("conversationMeta"),
    modeSwitch: document.getElementById("modeSwitch"),
    modelMenuWrap: document.getElementById("modelMenuWrap"),
    modelButton: document.getElementById("modelButton"),
    modelMark: document.getElementById("modelMark"),
    modelLabel: document.getElementById("modelLabel"),
    modelMenu: document.getElementById("modelMenu"),
    themeButton: document.getElementById("themeButton"),
    topbarSettingsButton: document.getElementById("topbarSettingsButton"),
    errorRegion: document.getElementById("errorRegion"),
    chatScroll: document.getElementById("chatScroll"),
    loadingState: document.getElementById("loadingState"),
    blockedState: document.getElementById("blockedState"),
    blockedTitle: document.getElementById("blockedTitle"),
    blockedMessage: document.getElementById("blockedMessage"),
    loginForm: document.getElementById("loginForm"),
    loginPassword: document.getElementById("loginPassword"),
    loginError: document.getElementById("loginError"),
    loginSubmit: document.getElementById("loginSubmit"),
    loginSubmitLabel: document.getElementById("loginSubmitLabel"),
    retryBootstrapButton: document.getElementById("retryBootstrapButton"),
    timeline: document.getElementById("timeline"),
    emptyState: document.getElementById("emptyState"),
    emptyVisual: document.getElementById("emptyVisual"),
    emptyBoardImage: document.getElementById("emptyBoardImage"),
    emptyKickerName: document.getElementById("emptyKickerName"),
    emptyTitle: document.getElementById("emptyTitle"),
    emptySubtitle: document.getElementById("emptySubtitle"),
    promptGrid: document.getElementById("promptGrid"),
    jumpBottomButton: document.getElementById("jumpBottomButton"),
    composerDock: document.getElementById("composerDock"),
    liveStopRail: document.getElementById("liveStopRail"),
    questionDock: document.getElementById("questionDock"),
    composerForm: document.getElementById("composerForm"),
    composerInput: document.getElementById("composerInput"),
    queueTray: document.getElementById("queueTray"),
    composerState: document.getElementById("composerState"),
    characterCount: document.getElementById("characterCount"),
    sendButton: document.getElementById("sendButton"),
    drawerScrim: document.getElementById("drawerScrim"),
    settingsDrawer: document.getElementById("settingsDrawer"),
    settingsClose: document.getElementById("settingsClose"),
    settingsNav: document.querySelector(".settings-nav"),
    settingsPanels: Array.from(document.querySelectorAll("[data-settings-panel]")),
    settingsModelMark: document.getElementById("settingsModelMark"),
    settingsModelName: document.getElementById("settingsModelName"),
    settingsModelProvider: document.getElementById("settingsModelProvider"),
    capabilityList: document.getElementById("capabilityList"),
    versionLabel: document.getElementById("versionLabel"),
    generalConfigForm: document.getElementById("generalConfigForm"),
    providerEditor: document.getElementById("providerEditor"),
    addProviderButton: document.getElementById("addProviderButton"),
    modelPoolEditor: document.getElementById("modelPoolEditor"),
    pluginEditor: document.getElementById("pluginEditor"),
    promptEditor: document.getElementById("promptEditor"),
    advancedConfigEditor: document.getElementById("advancedConfigEditor"),
    applyAdvancedConfigButton: document.getElementById("applyAdvancedConfigButton"),
    reloadConfigButton: document.getElementById("reloadConfigButton"),
    saveConfigButton: document.getElementById("saveConfigButton"),
    settingsStatus: document.getElementById("settingsStatus"),
    toastRegion: document.getElementById("toastRegion"),
    resetDialog: document.getElementById("resetDialog"),
    resetCancelButton: document.getElementById("resetCancelButton"),
    resetConfirmButton: document.getElementById("resetConfirmButton")
  };

  const state = {
    bootId: null,
    latestEventId: 0,
    lastEventId: 0,
    replayRunIds: null,
    replayCutoff: 0,
    turns: [],
    queuedPrompts: [],
    models: [],
    persona: {
      name: "Miyu",
      avatar_url: "/assets/miyu-logo.png",
      board_image_url: "/assets/miyuwallpaper.png",
      board_title: DEFAULT_BOARD_TITLE,
      board_subtitle: DEFAULT_BOARD_SUBTITLE,
      starter_prompts: DEFAULT_STARTER_PROMPTS
    },
    sessions: [],
    currentSessionId: null,
    viewSessionId: null,
    viewRunningTurnId: null,
    viewLoading: false,
    viewSyncTimer: null,
    runsBySession: new Map(),
    liveRuns: new Map(),
    archivedSessions: [],
    archivedOpen: false,
    archivedLoading: false,
    sessionMenuFor: null,
    sessionRenaming: null,
    sessionBusy: false,
    display: {
      reasoning: "summary",
      tool_calls: "summary",
      readable_tool_names: true,
      command_output_lines: 10,
      mixed_model_endpoint_display: "interactive",
      show_mixed_model_endpoint: false
    },
    context: { tokens: 0, window: null },
    usage: {},
    capabilities: {},
    version: null,
    eventSource: null,
    connection: "connecting",
    blocked: false,
    adminBusy: false,
    loginSubmitting: false,
    modelSelectionSubmitting: false,
    stagedModelKeys: null,
    modelMenuError: "",
    submitting: false,
    pendingSubmission: null,
    colorScheme: null,
    matugenAvailable: null,
    reasoningExpanded: false,
    toolExpanded: false,
    finishedTurnArticles: new Map(),
    bootstrapPromise: null,
    resyncing: false,
    nearBottom: true,
    followOutput: true,
    scrollRequestId: 0,
    programmaticScroll: false,
    settingsOpener: null,
    sidebarOpener: null,
    toastTimer: null,
    healthTimer: null,
    terminalRunIds: new Set(),
    mode: "normal",
    composing: false,
    settingsView: "interface",
    configLoaded: false,
    configLoading: false,
    configSaving: false,
    configDirty: false,
    configDraft: null,
    configOriginal: null,
    promptDraft: null,
    promptOriginal: null,
    secretStates: {},
    secretChanges: {},
    providerSecretStates: [],
    configMultimodalModels: [],
    configInferredImageModels: [],
    invalidConfigFields: new Map()
  };

  class ApiError extends Error {
    constructor(message, status) {
      super(message);
      this.name = "ApiError";
      this.status = status;
    }
  }

  function createIcon(name, className = "") {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "2");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("stroke-linejoin", "round");
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    if (className) svg.setAttribute("class", className);
    const definition = ICONS[name] || ICONS["circle-alert"];
    for (const [tag, attributes] of definition) {
      const node = document.createElementNS(SVG_NS, tag);
      for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
      svg.appendChild(node);
    }
    return svg;
  }

  function renderIconSlots(root = document) {
    const slots = [];
    if (root instanceof Element && root.matches("[data-icon]")) slots.push(root);
    slots.push(...root.querySelectorAll("[data-icon]"));
    for (const slot of slots) {
      slot.replaceChildren(createIcon(slot.dataset.icon));
    }
  }

  function makeIconSlot(name, className = "") {
    const slot = document.createElement("span");
    slot.className = `icon-slot${className ? ` ${className}` : ""}`;
    slot.setAttribute("aria-hidden", "true");
    slot.appendChild(createIcon(name));
    return slot;
  }

  function safeStorageGet(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (_) {
      return null;
    }
  }

  function safeStorageSet(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch (_) {
      // Storage can be unavailable in hardened browser profiles.
    }
  }

  function setTheme(theme, persist = true) {
    const selected = theme === "linen" ? "linen" : "graphite";
    elements.body.dataset.theme = selected;
    document.querySelectorAll("[data-theme-choice]").forEach((button) => {
      button.classList.toggle("selected", button.dataset.themeChoice === selected);
      button.setAttribute("aria-pressed", String(button.dataset.themeChoice === selected));
    });
    const nextIcon = selected === "graphite" ? "sun" : "moon";
    for (const button of [elements.themeButton, elements.sidebarThemeButton]) {
      const slot = button.querySelector(".icon-slot");
      slot.replaceChildren(createIcon(nextIcon));
      button.title = selected === "graphite" ? "切换到晨光主题" : "切换到夜阑主题";
      button.setAttribute("aria-label", button.title);
    }
    const themeColor = document.querySelector('meta[name="theme-color"]');
    if (themeColor) themeColor.content = selected === "graphite" ? "#171821" : "#f6f0e2";
    if (persist) safeStorageSet("miyu.web.theme", selected);
  }

  /*
   * 配色方案(与明暗正交):
   * - madobe  窗边预设(logo 派生 token,styles.css 内置)
   * - matugen 壁纸取色(后端 /theme.css 输出整套 MD3 token)
   * 通过禁用 /theme.css 的 <link> 切换,不改后端与 matugen 模板。
   */
  function setColorScheme(scheme, persist = true) {
    const requested = scheme === "madobe" ? "madobe" : "matugen";
    const selected = requested === "matugen" && state.matugenAvailable === false ? "madobe" : requested;
    state.colorScheme = selected;
    if (elements.matugenThemeLink) elements.matugenThemeLink.disabled = selected !== "matugen";
    document.querySelectorAll("[data-scheme-choice]").forEach((button) => {
      const active = button.dataset.schemeChoice === selected;
      button.classList.toggle("selected", active);
      button.setAttribute("aria-pressed", String(active));
      // 探测不到 matugen 输出时,「壁纸取色」整个选项不显示。
      if (button.dataset.schemeChoice === "matugen") button.hidden = state.matugenAvailable !== true;
    });
    if (persist) safeStorageSet("miyu.web.colorScheme", requested);
  }

  async function probeMatugenTheme() {
    try {
      const response = await fetch("/theme.css", { method: "HEAD", cache: "no-store" });
      state.matugenAvailable = response.ok;
    } catch (_) {
      state.matugenAvailable = false;
    }
    // 无持久化记录时:matugen 可用则维持现状(matugen),否则窗边。默认值不写入存储。
    setColorScheme(safeStorageGet("miyu.web.colorScheme") || (state.matugenAvailable ? "matugen" : "madobe"), false);
  }

  /* 仅 WebUI 的本地显示偏好(localStorage,不写入 config) */
  const CHAT_FONT_SIZES = ["14px", "15px", "16px"];

  function setChatFontSize(size, persist = true) {
    const selected = CHAT_FONT_SIZES.includes(size) ? size : "15px";
    document.documentElement.style.setProperty("--fs-chat", selected);
    document.querySelectorAll("[data-chat-font]").forEach((button) => {
      const active = button.dataset.chatFont === selected;
      button.classList.toggle("active", active);
      button.setAttribute("aria-pressed", String(active));
    });
    if (persist) safeStorageSet("miyu.web.chatFontSize", selected);
  }

  function setReasoningExpanded(value, persist = true) {
    state.reasoningExpanded = Boolean(value);
    elements.reasoningExpandToggle?.setAttribute("aria-checked", String(state.reasoningExpanded));
    // 对已渲染的思考块即时生效
    document.querySelectorAll(".reasoning-block").forEach((block) => {
      block.open = state.reasoningExpanded;
    });
    if (persist) safeStorageSet("miyu.web.reasoningExpanded", String(state.reasoningExpanded));
  }

  function setToolExpanded(value, persist = true) {
    state.toolExpanded = Boolean(value);
    elements.toolExpandToggle?.setAttribute("aria-checked", String(state.toolExpanded));
    // 对已渲染的工具签即时生效
    document.querySelectorAll(".tool-card").forEach((card) => {
      card.classList.toggle("collapsed", !state.toolExpanded);
      card.querySelector(".tool-head")?.setAttribute("aria-expanded", String(state.toolExpanded));
    });
    if (persist) safeStorageSet("miyu.web.toolExpanded", String(state.toolExpanded));
  }

  function setMode(mode, persist = true) {
    const selected = ["normal", "plan", "chat"].includes(mode) ? mode : "normal";
    state.mode = selected;
    elements.modeSwitch.querySelectorAll("[data-mode]").forEach((button) => {
      const active = button.dataset.mode === selected;
      button.classList.toggle("active", active);
      button.setAttribute("aria-pressed", String(active));
    });
    if (persist) safeStorageSet("miyu.web.mode", selected);
  }

  function closeSidebar() {
    elements.sidebar.classList.remove("open");
    elements.sidebarScrim.classList.remove("visible");
    elements.sidebarScrim.tabIndex = -1;
  }

  function openSidebar(opener = document.activeElement) {
    state.sidebarOpener = opener;
    elements.sidebar.classList.add("open");
    elements.sidebarScrim.classList.add("visible");
    elements.sidebarScrim.tabIndex = 0;
  }

  function getFocusable(container) {
    return Array.from(container.querySelectorAll("button:not(:disabled), input:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex='-1'])"))
      .filter((node) => !node.hidden && node.getClientRects().length > 0);
  }

  function openSettings(opener = document.activeElement) {
    state.settingsOpener = opener;
    closeModelMenu();
    elements.settingsDrawer.classList.add("open");
    elements.settingsDrawer.setAttribute("aria-hidden", "false");
    elements.drawerScrim.classList.add("visible");
    elements.drawerScrim.tabIndex = 0;
    window.requestAnimationFrame(() => elements.settingsClose.focus());
    if (!state.configLoaded && !state.configLoading) loadConfigDraft();
  }

  function closeSettings({ restoreFocus = true } = {}) {
    if (!elements.settingsDrawer.classList.contains("open")) return;
    elements.settingsDrawer.classList.remove("open");
    elements.settingsDrawer.setAttribute("aria-hidden", "true");
    elements.drawerScrim.classList.remove("visible");
    elements.drawerScrim.tabIndex = -1;
    if (restoreFocus && state.settingsOpener instanceof HTMLElement) state.settingsOpener.focus();
    state.settingsOpener = null;
  }

  function openModelMenu() {
    if (elements.modelButton.disabled || state.models.length === 0) return;
    state.stagedModelKeys = new Set(activeModels().map(modelKey));
    state.modelMenuError = "";
    renderModelMenu();
    elements.modelMenu.hidden = false;
    elements.modelButton.setAttribute("aria-expanded", "true");
    const selected = elements.modelMenu.querySelector(".model-menu-item.selected:not(:disabled)");
    const first = elements.modelMenu.querySelector(".model-menu-item:not(:disabled)");
    window.requestAnimationFrame(() => (selected || first)?.focus());
  }

  function closeModelMenu({ restoreFocus = false, discard = true } = {}) {
    if (elements.modelMenu.hidden) return;
    elements.modelMenu.hidden = true;
    elements.modelButton.setAttribute("aria-expanded", "false");
    if (discard) {
      state.stagedModelKeys = null;
      state.modelMenuError = "";
    }
    if (restoreFocus) elements.modelButton.focus();
  }

  function showToast(message, type = "info") {
    const toast = document.createElement("div");
    toast.className = `toast${type === "error" ? " is-error" : ""}`;
    toast.textContent = String(message || "操作未完成");
    elements.toastRegion.replaceChildren(toast);
    if (state.toastTimer) window.clearTimeout(state.toastTimer);
    state.toastTimer = window.setTimeout(() => {
      if (toast.isConnected) toast.remove();
    }, type === "error" ? 6000 : 3000);
  }

  function showInlineError(message) {
    const text = String(message || "操作未完成").trim();
    elements.errorRegion.textContent = text;
    elements.errorRegion.hidden = !text;
  }

  function clearInlineError() {
    elements.errorRegion.textContent = "";
    elements.errorRegion.hidden = true;
  }

  function deepClone(value) {
    if (typeof structuredClone === "function") return structuredClone(value);
    return JSON.parse(JSON.stringify(value));
  }

  function normalizePersona(value) {
    const name = String(value?.name || "").trim() || "Miyu";
    const avatarUrl = typeof value?.avatar_url === "string" && value.avatar_url ? value.avatar_url : null;
    const boardImageUrl = typeof value?.board_image_url === "string" && value.board_image_url
      ? value.board_image_url
      : null;
    const boardTitle = String(value?.board_title || "").trim() || DEFAULT_BOARD_TITLE;
    const boardSubtitle = String(value?.board_subtitle || "").trim() || DEFAULT_BOARD_SUBTITLE;
    const configuredPrompts = Array.isArray(value?.starter_prompts) ? value.starter_prompts : [];
    const starterPrompts = DEFAULT_STARTER_PROMPTS.map((fallback, index) => String(configuredPrompts[index] || "").trim() || fallback);
    return {
      name,
      avatar_url: avatarUrl,
      board_image_url: boardImageUrl,
      board_title: boardTitle,
      board_subtitle: boardSubtitle,
      starter_prompts: starterPrompts,
      revision: `${Date.now()}`
    };
  }

  function setPersonaAvatar(image) {
    const url = state.persona?.avatar_url;
    image.hidden = !url;
    if (!url) {
      image.removeAttribute("src");
      return;
    }
    image.hidden = false;
    const separator = url.includes("?") ? "&" : "?";
    image.src = `${url}${separator}v=${encodeURIComponent(state.persona?.revision || "1")}`;
    image.onerror = () => {
      image.hidden = true;
      image.removeAttribute("src");
    };
  }

  function applyPersona(value) {
    state.persona = normalizePersona(value);
    elements.brandName.textContent = state.persona.name;
    elements.brandAvatar.alt = state.persona.name;
    setPersonaAvatar(elements.brandAvatar);
    elements.emptyKickerName.textContent = state.persona.name;
    elements.emptyTitle.textContent = state.persona.board_title;
    elements.emptySubtitle.textContent = state.persona.board_subtitle;
    const boardImageUrl = state.persona.board_image_url;
    elements.emptyVisual.hidden = !boardImageUrl;
    elements.emptyBoardImage.alt = `${state.persona.name} 看板图片`;
    if (boardImageUrl) {
      elements.emptyBoardImage.onerror = () => {
        elements.emptyBoardImage.removeAttribute("src");
        elements.emptyVisual.hidden = true;
      };
      elements.emptyBoardImage.src = `${boardImageUrl}${boardImageUrl.includes("?") ? "&" : "?"}v=${encodeURIComponent(state.persona.revision)}`;
    } else {
      elements.emptyBoardImage.removeAttribute("src");
    }
    elements.promptGrid.querySelectorAll("[data-prompt]").forEach((button, index) => {
      const prompt = state.persona.starter_prompts[index] || DEFAULT_STARTER_PROMPTS[index];
      button.dataset.prompt = prompt;
      const label = button.querySelector("span:last-child");
      if (label) label.textContent = prompt;
    });
    const refreshAssistant = (root) => root.querySelectorAll(".assistant-label").forEach((label) => {
      const name = label.querySelector("strong");
      const avatar = label.querySelector("img");
      if (name) name.textContent = state.persona.name;
      if (avatar) setPersonaAvatar(avatar);
    });
    refreshAssistant(elements.timeline);
    for (const articles of state.finishedTurnArticles.values()) {
      for (const entry of articles) refreshAssistant(entry.article);
    }
  }

  function setSettingsView(view) {
    const selected = ["interface", "prompts", "general", "providers", "models", "plugins", "advanced"].includes(view) ? view : "interface";
    state.settingsView = selected;
    elements.settingsNav.querySelectorAll("[data-settings-view]").forEach((button) => {
      const active = button.dataset.settingsView === selected;
      button.classList.toggle("active", active);
      button.setAttribute("aria-current", active ? "page" : "false");
    });
    elements.settingsPanels.forEach((panel) => {
      panel.hidden = panel.dataset.settingsPanel !== selected;
    });
  }

  function configValue(path, fallback = undefined) {
    let value = state.configDraft;
    for (const key of path.split(".")) {
      if (value == null || typeof value !== "object" || !(key in value)) return fallback;
      value = value[key];
    }
    return value;
  }

  function setConfigValue(path, value) {
    if (!state.configDraft) return;
    const keys = path.split(".");
    let target = state.configDraft;
    for (const key of keys.slice(0, -1)) {
      if (!target[key] || typeof target[key] !== "object") target[key] = {};
      target = target[key];
    }
    target[keys[keys.length - 1]] = value;
    markConfigDirty();
  }

  function clearConfigFieldError(input) {
    const message = state.invalidConfigFields.get(input);
    if (message) message.remove();
    state.invalidConfigFields.delete(input);
    input.classList.remove("is-invalid");
  }

  function setConfigFieldError(input, message) {
    clearConfigFieldError(input);
    const error = document.createElement("small");
    error.className = "config-field-error";
    error.textContent = message;
    input.classList.add("is-invalid");
    input.closest(".config-field")?.appendChild(error);
    state.invalidConfigFields.set(input, error);
  }

  function parseConfigInput(input, current) {
    clearConfigFieldError(input);
    if (input.dataset.valueType === "boolean") return input.checked;
    const raw = input.value;
    if (input.dataset.valueType === "number") {
      const number = Number(raw);
      if (!Number.isFinite(number)) throw new Error("请输入有效数字");
      return input.dataset.integer === "true" ? Math.trunc(number) : number;
    }
    if (input.dataset.valueType === "json") {
      if (!raw.trim()) return input.dataset.nullable === "true" ? null : {};
      try {
        return JSON.parse(raw);
      } catch (_) {
        throw new Error("请输入有效 JSON");
      }
    }
    if (input.dataset.valueType === "lines") {
      return raw.split(/\r?\n|,/).map((item) => item.trim()).filter(Boolean);
    }
    if (input.dataset.valueType === "numbers") {
      return raw.split(/[\s,;，；]+/).filter(Boolean).map((item) => {
        const number = Number(item);
        if (!Number.isSafeInteger(number)) throw new Error(`无效号码：${item}`);
        return number;
      });
    }
    return raw;
  }

  function bindConfigInput(input, path, options = {}) {
    input.dataset.configPath = path;
    input.dataset.valueType = options.type || "string";
    if (options.integer) input.dataset.integer = "true";
    if (options.nullable) input.dataset.nullable = "true";
    const eventName = input.tagName === "SELECT" || input.type === "checkbox" ? "change" : "input";
    input.addEventListener(eventName, () => {
      try {
        const value = parseConfigInput(input, configValue(path));
        setConfigValue(path, value);
        updateAdvancedConfigEditor();
        if (options.rerender) renderConfigEditors();
      } catch (error) {
        setConfigFieldError(input, error.message);
        updateSettingsControls();
      }
    });
    return input;
  }

  function configField(labelText, input, description = "") {
    const label = document.createElement("label");
    label.className = "config-field";
    const heading = document.createElement("span");
    heading.className = "config-field-label";
    heading.textContent = labelText;
    label.append(heading, input);
    if (description) {
      const hint = document.createElement("small");
      hint.className = "config-field-hint";
      hint.textContent = description;
      label.appendChild(hint);
    }
    return label;
  }

  function textConfigField(label, path, options = {}) {
    const current = configValue(path, options.defaultValue ?? "");
    const input = options.multiline ? document.createElement("textarea") : document.createElement("input");
    input.className = "config-input";
    if (!options.multiline) input.type = options.inputType || "text";
    if (options.multiline) input.rows = options.rows || 3;
    input.value = options.type === "json"
      ? (current == null ? "" : JSON.stringify(current, null, 2))
      : options.type === "lines"
        ? (Array.isArray(current) ? current.join("\n") : "")
        : options.type === "numbers"
          ? (Array.isArray(current) ? current.join(", ") : "")
          : String(current ?? "");
    if (options.placeholder) input.placeholder = options.placeholder;
    if (options.min != null) input.min = String(options.min);
    if (options.max != null) input.max = String(options.max);
    if (options.step != null) input.step = String(options.step);
    bindConfigInput(input, path, options);
    return configField(label, input, options.description || "");
  }

  function selectConfigField(label, path, choices, description = "") {
    const select = document.createElement("select");
    select.className = "config-input";
    const current = String(configValue(path, ""));
    for (const choice of choices) {
      const option = document.createElement("option");
      option.value = typeof choice === "string" ? choice : choice.value;
      option.textContent = typeof choice === "string" ? choice : choice.label;
      option.selected = option.value === current;
      select.appendChild(option);
    }
    bindConfigInput(select, path);
    return configField(label, select, description);
  }

  function booleanConfigField(labelText, path, description = "") {
    const label = document.createElement("label");
    label.className = "config-toggle";
    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = Boolean(configValue(path));
    bindConfigInput(input, path, { type: "boolean" });
    const switchTrack = document.createElement("span");
    switchTrack.className = "toggle-track";
    const copy = document.createElement("span");
    copy.className = "config-toggle-copy";
    const title = document.createElement("strong");
    title.textContent = labelText;
    copy.appendChild(title);
    if (description) {
      const hint = document.createElement("small");
      hint.textContent = description;
      copy.appendChild(hint);
    }
    label.append(input, switchTrack, copy);
    return label;
  }

  function configGroup(titleText, fields = [], description = "") {
    const group = document.createElement("section");
    group.className = "config-group";
    const header = document.createElement("header");
    const title = document.createElement("h3");
    title.textContent = titleText;
    header.appendChild(title);
    if (description) {
      const copy = document.createElement("p");
      copy.textContent = description;
      header.appendChild(copy);
    }
    const body = document.createElement("div");
    body.className = "config-group-body";
    body.append(...fields);
    group.append(header, body);
    return group;
  }

  function actionButton(label, className = "secondary-button") {
    const button = document.createElement("button");
    button.type = "button";
    button.className = className;
    button.textContent = label;
    return button;
  }

  function markConfigDirty() {
    state.configDirty = true;
    updateSettingsControls();
  }

  function clearProviderSecretChanges() {
    for (const key of Object.keys(state.secretChanges)) {
      if (key.startsWith("providers.")) delete state.secretChanges[key];
    }
  }

  function refreshProviderSecretStates() {
    for (const key of Object.keys(state.secretStates)) {
      if (key.startsWith("providers.")) delete state.secretStates[key];
    }
    state.providerSecretStates.forEach((configured, index) => {
      state.secretStates[`providers.${index}.api_key`] = Boolean(configured);
    });
  }

  function updateSettingsControls() {
    const busy = state.configLoading || state.configSaving;
    elements.reloadConfigButton.disabled = busy;
    elements.saveConfigButton.disabled = busy || !state.configLoaded || !state.configDirty || state.invalidConfigFields.size > 0 || conversationRunning();
    elements.addProviderButton.disabled = busy || !state.configLoaded;
    if (state.configLoading) elements.settingsStatus.textContent = "正在载入配置";
    else if (state.configSaving) elements.settingsStatus.textContent = "正在验证并保存";
    else if (!state.configLoaded) elements.settingsStatus.textContent = "尚未载入配置";
    else if (state.invalidConfigFields.size) elements.settingsStatus.textContent = "请修正表单中的错误";
    else if (conversationRunning() && state.configDirty) elements.settingsStatus.textContent = "回复完成后才能保存";
    else elements.settingsStatus.textContent = state.configDirty ? "有未保存的修改" : "配置已同步";
  }

  function updateAdvancedConfigEditor() {
    if (!state.configDraft || document.activeElement === elements.advancedConfigEditor) return;
    elements.advancedConfigEditor.value = JSON.stringify(state.configDraft, null, 2);
  }

  function renderGeneralConfig() {
    elements.generalConfigForm.replaceChildren(
      configGroup("工具", [
        booleanConfigField("启用工具", "tools.enabled"),
        textConfigField("最大工具轮数", "tools.max_rounds", { type: "number", integer: true, inputType: "number", min: 0 }),
        selectConfigField("工具加载模式", "tools.loading_mode", ["full", "hybrid"]),
        booleanConfigField("记住已加载工具", "tools.persist_loaded_tools")
      ]),
      configGroup("Skills", [
        booleanConfigField("启用 Skills", "skills.enabled"),
        booleanConfigField("允许执行命令", "skills.allow_command_execution")
      ]),
      configGroup("思考", [
        selectConfigField(
          "思考详细程度",
          "display.reasoning",
          [{ value: "summary", label: "摘要" }, { value: "full", label: "完整" }, { value: "hidden", label: "隐藏" }],
          "决定向模型请求摘要还是完整思考并写入会话；设为隐藏则不产生思考内容。WebUI 的展开/收起在「界面」里设置。"
        )
      ]),
      configGroup("上下文", [
        selectConfigField("到达上限后", "context.on_overflow", [{ value: "compact", label: "压缩上下文" }, { value: "pop", label: "弹出旧消息" }]),
        textConfigField("开始裁剪比例", "context.trim_at_ratio", { type: "number", inputType: "number", min: 0.1, max: 1, step: 0.01 }),
        textConfigField("每批裁剪比例", "context.trim_batch_ratio", { type: "number", inputType: "number", min: 0.01, max: 0.9, step: 0.01 })
      ]),
      configGroup("记忆", [
        booleanConfigField("启用记忆", "memory.enabled"),
        booleanConfigField("保留弹出上下文", "memory.evicted_context_enabled"),
        booleanConfigField("启用联想", "memory.association_enabled"),
        booleanConfigField("自动日记", "memory.auto_diary_enabled"),
        booleanConfigField("自动事实记忆", "memory.auto_fact_enabled"),
        textConfigField("联想知识条数", "memory.association_facts", { type: "number", inputType: "number", integer: true, min: 0 }),
        textConfigField("联想事件条数", "memory.association_episodes", { type: "number", inputType: "number", integer: true, min: 0 }),
        textConfigField("联想字符上限", "memory.association_max_chars", { type: "number", inputType: "number", integer: true, min: 0 }),
        textConfigField("片段字符数", "memory.snippet_chars", { type: "number", inputType: "number", integer: true, min: 0 }),
        textConfigField("遗忘期限（天）", "memory.forget_after_days", { type: "number", inputType: "number", integer: true, min: 1 }),
        booleanConfigField("启用遗忘", "memory.forgetting_enabled"),
        textConfigField("遗忘半衰期（天）", "memory.forgetting_half_life_days", { type: "number", inputType: "number", min: 0.1, step: 0.1 }),
        textConfigField("最低遗忘强度", "memory.forgetting_min_strength", { type: "number", inputType: "number", min: 0, max: 1, step: 0.01 }),
        textConfigField("回忆增强强度", "memory.forgetting_review_boost", { type: "number", inputType: "number", min: 0, step: 0.01 }),
        textConfigField("最小任务字数", "memory.learning_min_task_chars", { type: "number", inputType: "number", integer: true, min: 0 }),
        textConfigField("最小方法字数", "memory.learning_min_method_chars", { type: "number", inputType: "number", integer: true, min: 0 })
      ]),
      configGroup("MCP", [
        booleanConfigField("启用 MCP", "mcp.enabled"),
        textConfigField("服务器配置", "mcp.servers", { type: "json", multiline: true, rows: 10, description: "JSON 数组，支持 id、command、args、env、timeout_seconds 和 enabled。" })
      ])
    );
  }

  function secretEditor(labelText, key, { multiline = false } = {}) {
    const wrapper = document.createElement("div");
    wrapper.className = "secret-editor config-field";
    const label = document.createElement("span");
    label.className = "config-field-label";
    label.textContent = labelText;
    const status = document.createElement("small");
    status.className = "secret-status";
    status.textContent = state.secretChanges[key]?.action === "clear"
      ? "将清空"
      : state.secretChanges[key]?.action === "set"
        ? "已输入新值"
        : state.secretStates[key]
          ? "已配置"
          : "未配置";
    const input = multiline ? document.createElement("textarea") : document.createElement("input");
    input.className = "config-input";
    if (!multiline) input.type = "password";
    if (multiline) input.rows = 3;
    input.placeholder = state.secretStates[key] ? "留空保留现有值" : "输入新值";
    input.value = state.secretChanges[key]?.action === "set" ? state.secretChanges[key].value : "";
    input.autocomplete = "new-password";
    const actions = document.createElement("div");
    actions.className = "secret-actions";
    const clear = actionButton("清空", "text-button danger-text");
    const preserve = actionButton("保留", "text-button");
    actions.append(preserve, clear);
    input.addEventListener("input", () => {
      if (input.value) state.secretChanges[key] = { action: "set", value: input.value };
      else delete state.secretChanges[key];
      markConfigDirty();
      status.textContent = input.value ? "已输入新值" : state.secretStates[key] ? "已配置" : "未配置";
    });
    clear.addEventListener("click", () => {
      input.value = "";
      state.secretChanges[key] = { action: "clear" };
      status.textContent = "将清空";
      markConfigDirty();
    });
    preserve.addEventListener("click", () => {
      input.value = "";
      delete state.secretChanges[key];
      status.textContent = state.secretStates[key] ? "已配置" : "未配置";
      markConfigDirty();
    });
    wrapper.append(label, status, input, actions);
    return wrapper;
  }

  function ensureProviderDefaults(provider = {}) {
    return {
      id: "",
      display_name: "",
      base_url: "",
      protocol: "auto",
      api_key: null,
      models: [],
      model_context_window: {},
      model_modalities: {},
      default_model: "",
      timeout_seconds: 60,
      temperature: 0.7,
      anthropic_max_tokens: 4096,
      extra_body: null,
      ...provider
    };
  }

  const PLATFORM_MODEL_POOL_NAMES = ["text_models", "multimodal_models"];

  function forEachPlatformModelPool(callback) {
    const routes = state.configDraft?.platforms?.qq?.conversations;
    if (!Array.isArray(routes)) return;
    for (const route of routes) {
      if (!route || typeof route !== "object") continue;
      for (const poolName of PLATFORM_MODEL_POOL_NAMES) {
        if (Array.isArray(route[poolName])) callback(route, poolName, route[poolName]);
      }
    }
  }

  function normalizePlatformModelRoutes() {
    const qq = state.configDraft?.platforms?.qq;
    if (!Array.isArray(qq?.conversations)) return;
    for (const route of qq.conversations) {
      if (!route || typeof route !== "object") continue;
      for (const poolName of PLATFORM_MODEL_POOL_NAMES) {
        if (Array.isArray(route[poolName]) && route[poolName].length === 0) delete route[poolName];
      }
    }
  }

  function replacePlatformProviderReferences(previousId, nextId) {
    forEachPlatformModelPool((_route, _poolName, pool) => {
      for (const item of pool) {
        if (item?.provider_id === previousId) item.provider_id = nextId;
      }
    });
  }

  function removePlatformProviderReferences(providerId) {
    forEachPlatformModelPool((route, poolName, pool) => {
      route[poolName] = pool.filter((item) => item?.provider_id !== providerId);
    });
    normalizePlatformModelRoutes();
  }

  function providerHasConfiguredModel(provider, model) {
    const normalizedModel = String(model || "").trim();
    return Boolean(normalizedModel) && (
      String(provider?.default_model || "") === normalizedModel
      || (Array.isArray(provider?.models) && provider.models.includes(normalizedModel))
    );
  }

  function forEachSubagentTierPool(callback) {
    const tiers = state.configDraft?.subagent_tiers;
    if (!tiers || typeof tiers !== "object") return;
    for (const [tierName, pool] of Object.entries(tiers)) {
      if (Array.isArray(pool)) callback(tiers, tierName, pool);
    }
  }

  function pruneOptionalPool(owner, key, predicate) {
    if (!owner || !Array.isArray(owner[key])) return;
    const pool = owner[key].filter(predicate);
    if (pool.length) owner[key] = pool;
    else delete owner[key];
  }

  function providerModelSupportsMedia(provider, model) {
    const normalizedModel = String(model || "").trim();
    const declared = provider?.model_modalities;
    if (declared && typeof declared === "object" && Object.prototype.hasOwnProperty.call(declared, normalizedModel)) {
      return Array.isArray(declared[normalizedModel])
        && declared[normalizedModel].includes("image");
    }
    return state.configInferredImageModels.some((item) => (
      item?.provider_id === provider?.id && item?.model === normalizedModel
    ));
  }

  function modelReferenceTarget(providersById, item) {
    const provider = providersById.get(String(item?.provider_id || "").trim());
    const model = String(item?.model || "").trim();
    return provider && providerHasConfiguredModel(provider, model) ? { provider, model } : null;
  }

  function prunePlatformModelRoutes(providersById) {
    forEachPlatformModelPool((route, poolName, pool) => {
      route[poolName] = pool.filter((item) => {
        const target = modelReferenceTarget(providersById, item);
        return Boolean(target) && (
          poolName !== "multimodal_models"
          || providerModelSupportsMedia(target.provider, target.model)
        );
      });
    });
    normalizePlatformModelRoutes();
  }

  function clearInvalidPluginModelReferences(providersById) {
    const vision = state.configDraft?.plugins?.vision;
    if (vision?.vision_provider_id) {
      const provider = providersById.get(String(vision.vision_provider_id).trim());
      const configuredModel = String(vision.vision_model || "").trim();
      const model = configuredModel || String(provider?.default_model || "").trim();
      if (!provider || !providerHasConfiguredModel(provider, model) || !providerModelSupportsMedia(provider, model)) {
        vision.vision_provider_id = "";
        vision.vision_model = "";
      }
    }
    const knowledgeBase = state.configDraft?.plugins?.knowledge_base;
    if (knowledgeBase?.embedding_provider_id) {
      const provider = providersById.get(String(knowledgeBase.embedding_provider_id).trim());
      const configuredModel = String(knowledgeBase.embedding_model || "").trim();
      const model = configuredModel || String(provider?.default_model || "").trim();
      if (!provider || !providerHasConfiguredModel(provider, model)) {
        knowledgeBase.embedding_provider_id = "";
        knowledgeBase.embedding_model = "";
      }
    }
  }

  function pruneModelReferences() {
    if (!state.configDraft) return;
    const providers = Array.isArray(state.configDraft.providers) ? state.configDraft.providers : [];
    const providersById = new Map(providers.map((provider) => [String(provider?.id || ""), provider]));
    pruneOptionalPool(state.configDraft, "active_provider_models", (item) => (
      Boolean(modelReferenceTarget(providersById, item))
    ));
    pruneOptionalPool(state.configDraft, "active_multimodal_provider_models", (item) => {
      const target = modelReferenceTarget(providersById, item);
      return Boolean(target) && providerModelSupportsMedia(target.provider, target.model);
    });
    forEachSubagentTierPool((tiers, tierName, pool) => {
      tiers[tierName] = pool.filter((item) => Boolean(modelReferenceTarget(providersById, item)));
    });
    prunePlatformModelRoutes(providersById);
    clearInvalidPluginModelReferences(providersById);
  }

  function replaceProviderReferences(previousId, nextId) {
    if (!previousId || previousId === nextId || !state.configDraft) return;
    if (state.configDraft.active_provider === previousId) state.configDraft.active_provider = nextId;
    for (const poolName of ["active_provider_models", "active_multimodal_provider_models"]) {
      for (const item of state.configDraft[poolName] || []) {
        if (item.provider_id === previousId) item.provider_id = nextId;
      }
    }
    if (state.configDraft.plugins?.vision?.vision_provider_id === previousId) {
      state.configDraft.plugins.vision.vision_provider_id = nextId;
    }
    if (state.configDraft.plugins?.knowledge_base?.embedding_provider_id === previousId) {
      state.configDraft.plugins.knowledge_base.embedding_provider_id = nextId;
    }
    forEachSubagentTierPool((_tiers, _tierName, pool) => {
      for (const item of pool) {
        if (item?.provider_id === previousId) item.provider_id = nextId;
      }
    });
    replacePlatformProviderReferences(previousId, nextId);
    for (const models of [state.configMultimodalModels, state.configInferredImageModels]) {
      for (const model of models) {
        if (model?.provider_id === previousId) model.provider_id = nextId;
      }
    }
  }

  function removeProviderReferences(providerId) {
    if (!state.configDraft) return;
    pruneOptionalPool(state.configDraft, "active_provider_models", (item) => item?.provider_id !== providerId);
    pruneOptionalPool(state.configDraft, "active_multimodal_provider_models", (item) => item?.provider_id !== providerId);
    forEachSubagentTierPool((tiers, tierName, pool) => {
      tiers[tierName] = pool.filter((item) => item?.provider_id !== providerId);
    });
    if (state.configDraft.plugins?.vision?.vision_provider_id === providerId) {
      state.configDraft.plugins.vision.vision_provider_id = "";
      state.configDraft.plugins.vision.vision_model = "";
    }
    if (state.configDraft.plugins?.knowledge_base?.embedding_provider_id === providerId) {
      state.configDraft.plugins.knowledge_base.embedding_provider_id = "";
      state.configDraft.plugins.knowledge_base.embedding_model = "";
    }
    removePlatformProviderReferences(providerId);
    state.configMultimodalModels = state.configMultimodalModels.filter((item) => item?.provider_id !== providerId);
    state.configInferredImageModels = state.configInferredImageModels.filter((item) => item?.provider_id !== providerId);
  }

  function renderProviders() {
    elements.providerEditor.replaceChildren();
    const providers = Array.isArray(state.configDraft?.providers) ? state.configDraft.providers : [];
    providers.forEach((provider, index) => {
      let referencedProviderId = String(provider.id || "");
      const card = document.createElement("details");
      card.className = "provider-card";
      card.open = index === 0;
      const summary = document.createElement("summary");
      const copy = document.createElement("span");
      const name = document.createElement("strong");
      name.textContent = provider.display_name || provider.id || `供应商 ${index + 1}`;
      const id = document.createElement("small");
      id.textContent = provider.id || "尚未命名";
      copy.append(name, id);
      const remove = actionButton("", "icon-button danger-text");
      remove.title = "删除";
      remove.setAttribute("aria-label", "删除");
      remove.appendChild(makeIconSlot("trash-2"));
      remove.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!window.confirm(`删除供应商“${provider.display_name || provider.id || index + 1}”？`)) return;
        state.configDraft.providers.splice(index, 1);
        state.providerSecretStates.splice(index, 1);
        refreshProviderSecretStates();
        clearProviderSecretChanges();
        const removedProviderId = referencedProviderId || provider.id;
        removeProviderReferences(removedProviderId);
        if (state.configDraft.active_provider === removedProviderId || state.configDraft.active_provider === provider.id) {
          state.configDraft.active_provider = state.configDraft.providers[0]?.id || "";
        }
        markConfigDirty();
        renderConfigEditors();
      });
      summary.append(copy, remove);
      const body = document.createElement("div");
      body.className = "provider-card-body";
      const fields = [
        ["配置 ID", "id"], ["显示名称", "display_name"], ["Base URL", "base_url"],
        ["默认模型", "default_model"]
      ];
      for (const [label, key] of fields) {
        const input = document.createElement("input");
        input.className = "config-input";
        input.value = String(provider[key] || "");
        input.addEventListener("input", () => {
          const previousId = key === "id" ? String(provider.id || "") : "";
          provider[key] = input.value;
          if (key === "id" && previousId !== provider.id) {
            const nextId = String(provider.id || "");
            if (referencedProviderId && nextId && referencedProviderId !== nextId) {
              replaceProviderReferences(referencedProviderId, nextId);
            }
            if (nextId) referencedProviderId = nextId;
            state.providerSecretStates[index] = false;
            delete state.secretChanges[`providers.${index}.api_key`];
            refreshProviderSecretStates();
            renderModelPools();
          }
          if (key === "default_model") renderModelPools();
          if (key === "display_name" || key === "id") {
            name.textContent = provider.display_name || provider.id || `供应商 ${index + 1}`;
            id.textContent = provider.id || "尚未命名";
          }
          markConfigDirty();
          updateAdvancedConfigEditor();
        });
        if (key === "default_model") {
          input.addEventListener("change", () => {
            provider.models = Array.isArray(provider.models) ? provider.models : [];
            if (provider.default_model && !provider.models.includes(provider.default_model)) {
              provider.models.push(provider.default_model);
            }
            pruneModelReferences();
            renderModelPools();
            updateAdvancedConfigEditor();
          });
        }
        body.appendChild(configField(label, input));
      }
      const protocol = document.createElement("select");
      protocol.className = "config-input";
      for (const value of ["auto", "openai-chat", "openai-responses", "anthropic"]) {
        const option = document.createElement("option");
        option.value = value;
        option.textContent = value;
        option.selected = provider.protocol === value;
        protocol.appendChild(option);
      }
      protocol.addEventListener("change", () => { provider.protocol = protocol.value; markConfigDirty(); updateAdvancedConfigEditor(); });
      body.appendChild(configField("协议", protocol));
      const secretKey = `providers.${index}.api_key`;
      body.appendChild(secretEditor("API Key", secretKey));

      const numeric = [
        ["超时秒数", "timeout_seconds", 1, 1], ["Temperature", "temperature", 0, 0.1], ["Anthropic 最大 Token", "anthropic_max_tokens", 1, 1]
      ];
      for (const [label, key, min, step] of numeric) {
        const input = document.createElement("input");
        input.className = "config-input";
        input.type = "number";
        input.min = String(min);
        input.step = String(step);
        input.value = String(provider[key] ?? "");
        input.addEventListener("input", () => {
          const value = Number(input.value);
          if (Number.isFinite(value)) {
            provider[key] = key === "temperature" ? value : Math.trunc(value);
            markConfigDirty();
            updateAdvancedConfigEditor();
          }
        });
        body.appendChild(configField(label, input));
      }
      const structured = [
        ["可用模型", "models", "lines", "每行一个模型"],
        ["模型上下文窗口", "model_context_window", "json", "JSON 对象：模型名到 Token 数"],
        ["模型输入模态", "model_modalities", "json", "JSON 对象：模型名到 text/image/audio/video/pdf 数组"],
        ["额外请求体", "extra_body", "json", "JSON 对象，留空表示不设置"]
      ];
      for (const [label, key, type, description] of structured) {
        const input = document.createElement("textarea");
        input.className = "config-input";
        input.rows = key === "models" ? 4 : 5;
        input.value = type === "lines" ? (provider[key] || []).join("\n") : provider[key] == null ? "" : JSON.stringify(provider[key], null, 2);
        input.addEventListener("input", () => {
          clearConfigFieldError(input);
          try {
            provider[key] = type === "lines"
              ? input.value.split(/\r?\n|,/).map((item) => item.trim()).filter(Boolean)
              : input.value.trim() ? JSON.parse(input.value) : key === "extra_body" ? null : {};
            if (key === "models" && provider.default_model && !provider.models.includes(provider.default_model)) {
              provider.models.push(provider.default_model);
            }
            markConfigDirty();
            updateAdvancedConfigEditor();
            if (key === "models" || key === "model_modalities") renderModelPools();
          } catch (_) {
            setConfigFieldError(input, "请输入有效 JSON");
            updateSettingsControls();
          }
        });
        if (key === "models" || key === "model_modalities") {
          input.addEventListener("change", () => {
            if (state.invalidConfigFields.has(input)) return;
            pruneModelReferences();
            renderModelPools();
            updateAdvancedConfigEditor();
          });
        }
        body.appendChild(configField(label, input, description));
      }
      card.append(summary, body);
      elements.providerEditor.appendChild(card);
    });
    if (!providers.length) {
      const empty = document.createElement("p");
      empty.className = "settings-empty";
      empty.textContent = "至少需要添加一个供应商。";
      elements.providerEditor.appendChild(empty);
    }
  }

  function configuredModelChoices() {
    const result = [];
    for (const provider of state.configDraft?.providers || []) {
      const models = Array.isArray(provider.models) && provider.models.length ? provider.models : provider.default_model ? [provider.default_model] : [];
      for (const model of models) {
        if (String(model).trim()) result.push({ provider_id: String(provider.id || ""), provider_name: String(provider.display_name || provider.id || ""), model: String(model) });
      }
    }
    return result;
  }

  function renderModelPoolList(titleText, path, choices) {
    const providers = Array.isArray(state.configDraft?.providers) ? state.configDraft.providers : [];
    const selected = Array.isArray(state.configDraft[path])
      ? state.configDraft[path]
      : path === "active_provider_models"
        ? choices.filter((choice) => choice.provider_id === state.configDraft.active_provider && choice.model === providers.find((provider) => provider.id === state.configDraft.active_provider)?.default_model)
        : [];
    const group = configGroup(titleText);
    const body = group.querySelector(".config-group-body");
    if (!choices.length) {
      const empty = document.createElement("p");
      empty.className = "settings-empty";
      empty.textContent = "请先在供应商中配置模型。";
      body.appendChild(empty);
    }
    for (const model of choices) {
      const label = document.createElement("label");
      label.className = "model-pool-option";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = selected.some((item) => item.provider_id === model.provider_id && item.model === model.model);
      input.addEventListener("change", () => {
        let pool = Array.isArray(state.configDraft[path]) ? state.configDraft[path] : [...selected];
        if (input.checked && !pool.some((item) => item.provider_id === model.provider_id && item.model === model.model)) {
          pool = [...pool, { provider_id: model.provider_id, model: model.model }];
        } else if (!input.checked) {
          pool = pool.filter((item) => item.provider_id !== model.provider_id || item.model !== model.model);
        }
        state.configDraft[path] = pool;
        markConfigDirty();
        updateAdvancedConfigEditor();
      });
      const copy = document.createElement("span");
      const name = document.createElement("strong");
      name.textContent = model.model;
      const provider = document.createElement("small");
      provider.textContent = model.provider_name;
      copy.append(name, provider);
      label.append(input, copy);
      body.appendChild(label);
    }
    return group;
  }

  function renderSubagentTierList(titleText, tierKey, choices) {
    if (!state.configDraft.subagent_tiers || typeof state.configDraft.subagent_tiers !== "object") {
      state.configDraft.subagent_tiers = {};
    }
    const tiers = state.configDraft.subagent_tiers;
    const selected = Array.isArray(tiers[tierKey]) ? tiers[tierKey] : [];
    const group = configGroup(titleText);
    const body = group.querySelector(".config-group-body");
    if (!choices.length) {
      const empty = document.createElement("p");
      empty.className = "settings-empty";
      empty.textContent = "请先在供应商中配置模型。";
      body.appendChild(empty);
    }
    for (const model of choices) {
      const label = document.createElement("label");
      label.className = "model-pool-option";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = selected.some((item) => item.provider_id === model.provider_id && item.model === model.model);
      input.addEventListener("change", () => {
        let pool = Array.isArray(tiers[tierKey]) ? tiers[tierKey] : [];
        if (input.checked && !pool.some((item) => item.provider_id === model.provider_id && item.model === model.model)) {
          pool = [...pool, { provider_id: model.provider_id, model: model.model }];
        } else if (!input.checked) {
          pool = pool.filter((item) => item.provider_id !== model.provider_id || item.model !== model.model);
        }
        tiers[tierKey] = pool;
        markConfigDirty();
        updateAdvancedConfigEditor();
      });
      const copy = document.createElement("span");
      const name = document.createElement("strong");
      name.textContent = model.model;
      const provider = document.createElement("small");
      provider.textContent = model.provider_name;
      copy.append(name, provider);
      label.append(input, copy);
      body.appendChild(label);
    }
    return group;
  }

  function renderModelPools() {
    const providers = Array.isArray(state.configDraft?.providers) ? state.configDraft.providers : [];
    const choices = configuredModelChoices();
    const multimodal = choices.filter((choice) => {
      const provider = providers.find((item) => item.id === choice.provider_id);
      return providerModelSupportsMedia(provider, choice.model);
    });
    elements.modelPoolEditor.replaceChildren(
      renderModelPoolList("文本模型池", "active_provider_models", choices),
      renderModelPoolList("多模态模型池", "active_multimodal_provider_models", multimodal),
      renderSubagentTierList("子代理档位池 · cheap（简单任务）", "cheap", choices),
      renderSubagentTierList("子代理档位池 · balanced（普通任务）", "balanced", choices),
      renderSubagentTierList("子代理档位池 · strong（复杂任务）", "strong", choices)
    );
  }

  const PLUGIN_LABELS = {
    weather: "天气", web: "网络搜索", web_images: "图片搜索", deep_research: "深度研究", deep_diagnose: "深度诊断",
    vision: "识图", exchange_rate: "汇率", xuanxue: "玄学", image_generation: "生图", print_image: "打印图片",
    memes: "表情包", knowledge_base: "知识库", archlinux: "Arch Linux", man: "在线手册", moegirl: "萌娘百科",
    hash_codec: "哈希与编解码", calculator: "计算器", package_advisor: "AUR 审查",
    deep_research_linux_game_compatibility: "Linux 游戏兼容", diagnostics: "系统诊断", memory: "记忆"
  };

  const SECRET_PLUGIN_PATHS = new Map([
    ["web.tavily_api_keys", "plugins.web.tavily_api_keys"],
    ["web.firecrawl_api_keys", "plugins.web.firecrawl_api_keys"],
    ["web.anysearch_api_keys", "plugins.web.anysearch_api_keys"],
    ["exchange_rate.api_key", "plugins.exchange_rate.api_key"],
    ["image_generation.api_keys", "plugins.image_generation.api_keys"]
  ]);

  const WEB_HIDDEN_PLUGIN_FIELDS = new Set([
    "vision.preview_with_chafa",
    "image_generation.auto_print",
    "print_image.width_percent",
    "print_image.height_percent",
    "memes.width_percent",
    "memes.height_percent",
    "web_images.auto_preview",
    "web_images.preview_count"
  ]);

  function humanizeConfigKey(key) {
    return String(key).replace(/_/g, " ").replace(/\b\w/g, (character) => character.toUpperCase());
  }

  function pluginValueEditor(pluginKey, fieldKey, value) {
    const path = `plugins.${pluginKey}.${fieldKey}`;
    const secretKey = SECRET_PLUGIN_PATHS.get(`${pluginKey}.${fieldKey}`);
    if (secretKey) return secretEditor(humanizeConfigKey(fieldKey), secretKey, { multiline: Array.isArray(value) });
    if (typeof value === "boolean") return booleanConfigField(humanizeConfigKey(fieldKey), path);
    if (typeof value === "number") return textConfigField(humanizeConfigKey(fieldKey), path, { type: "number", integer: Number.isInteger(value), inputType: "number", step: Number.isInteger(value) ? 1 : 0.01 });
    if (typeof value === "string") return textConfigField(humanizeConfigKey(fieldKey), path, { multiline: value.length > 100, rows: 3 });
    return textConfigField(humanizeConfigKey(fieldKey), path, { type: "json", multiline: true, rows: 5 });
  }

  function renderPlugins() {
    elements.pluginEditor.replaceChildren();
    for (const [pluginKey, plugin] of Object.entries(state.configDraft?.plugins || {})) {
      if (pluginKey === "memory" || pluginKey === "print_image") continue;
      const details = document.createElement("details");
      details.className = "plugin-card";
      const summary = document.createElement("summary");
      const copy = document.createElement("span");
      const title = document.createElement("strong");
      title.textContent = PLUGIN_LABELS[pluginKey] || humanizeConfigKey(pluginKey);
      const technical = document.createElement("small");
      technical.textContent = pluginKey;
      copy.append(title, technical);
      const badge = document.createElement("span");
      badge.className = `plugin-state${plugin?.enabled ? " is-enabled" : ""}`;
      badge.textContent = plugin?.enabled ? "启用" : "禁用";
      summary.append(copy, badge);
      const body = document.createElement("div");
      body.className = "plugin-card-body";
      for (const [fieldKey, value] of Object.entries(plugin || {})) {
        if (WEB_HIDDEN_PLUGIN_FIELDS.has(`${pluginKey}.${fieldKey}`)) continue;
        body.appendChild(pluginValueEditor(pluginKey, fieldKey, value));
      }
      details.append(summary, body);
      elements.pluginEditor.appendChild(details);
    }
  }

  function normalizedDocumentName(name) {
    const trimmed = String(name || "").trim().replace(/[\\/]/g, "-").replace(/\.md$/i, "");
    return trimmed ? `${trimmed}.md` : "";
  }

  function personaTextField(promptDocument, key, label, placeholder) {
    const input = document.createElement("input");
    input.className = "config-input";
    input.type = "text";
    input.maxLength = 200;
    input.placeholder = placeholder;
    input.value = String(promptDocument[key] || "");
    input.addEventListener("input", () => {
      promptDocument[key] = input.value.trim() || null;
      markConfigDirty();
    });
    return configField(label, input);
  }

  function personaImageField(promptDocument, key, label, fallbackUrl) {
    const pathInput = document.createElement("input");
    pathInput.className = "config-input";
    pathInput.type = "text";
    pathInput.placeholder = "";
    pathInput.value = String(promptDocument[key] || "");
    const picker = document.createElement("input");
    picker.type = "file";
    picker.accept = "image/png,image/jpeg,image/webp,image/gif,image/bmp";
    picker.hidden = true;
    const pickButton = actionButton("", "icon-button");
    pickButton.title = `选择${label.replace(/^自定义/, "")}`;
    pickButton.setAttribute("aria-label", pickButton.title);
    pickButton.appendChild(makeIconSlot("folder"));
    pickButton.addEventListener("click", () => picker.click());
    const preview = document.createElement("img");
    preview.className = `persona-avatar-preview${key === "board_image_path" ? " persona-board-preview" : ""}`;
    preview.alt = "";
    preview.setAttribute("aria-hidden", "true");
    const showStoredPreview = () => {
      preview.classList.remove("is-missing");
      preview.src = promptDocument[key]
        ? `/api/persona/avatar?path=${encodeURIComponent(promptDocument[key])}`
        : fallbackUrl || "";
      if (!promptDocument[key] && !fallbackUrl) {
        preview.removeAttribute("src");
        preview.classList.add("is-missing");
      }
    };
    preview.addEventListener("error", () => {
      preview.removeAttribute("src");
      preview.classList.add("is-missing");
    });
    showStoredPreview();
    pathInput.addEventListener("input", () => {
      promptDocument[key] = pathInput.value.trim() || null;
      showStoredPreview();
      markConfigDirty();
    });
    picker.addEventListener("change", async () => {
      const file = picker.files?.[0];
      if (!file) return;
      if (file.size > 8 * 1024 * 1024) return showToast("图片不能超过 8 MiB", "error");
      preview.src = URL.createObjectURL(file);
      preview.classList.remove("is-missing");
      pickButton.disabled = true;
      try {
        const response = await apiRequest("/api/persona/assets", {
          method: "POST",
          headers: { "Content-Type": file.type || "application/octet-stream" },
          body: file
        });
        const result = await response.json();
        promptDocument[key] = result.path;
        pathInput.value = result.path;
        preview.src = result.preview_url;
        markConfigDirty();
      } catch (error) {
        showToast(error.message || "图片上传失败", "error");
      } finally {
        pickButton.disabled = false;
        picker.value = "";
      }
    });
    const row = document.createElement("div");
    row.className = "avatar-path-row";
    row.append(pathInput, pickButton, preview, picker);
    return configField(label, row);
  }

  function renderPromptCollection(kind, titleText, activePath) {
    const documents = state.promptDraft[kind];
    const group = configGroup(titleText);
    const body = group.querySelector(".config-group-body");
    const active = document.createElement("select");
    active.className = "config-input";
    const defaultOption = document.createElement("option");
    defaultOption.value = "";
    defaultOption.textContent = kind === "personas" ? "Miyu 默认人格" : "不使用用户身份";
    active.appendChild(defaultOption);
    for (const promptDocument of documents) {
      const option = document.createElement("option");
      option.value = promptDocument.name;
      option.textContent = promptDocument.name.replace(/\.md$/i, "");
      active.appendChild(option);
    }
    active.value = String(configValue(activePath, ""));
    active.addEventListener("change", () => { setConfigValue(activePath, active.value); renderPromptEditor(); updateAdvancedConfigEditor(); });
    body.appendChild(configField("当前使用", active));
    const selected = documents.find((document) => document.name === active.value);
    for (const [index, promptDocument] of documents.entries()) {
      if (promptDocument !== selected) continue;
      const card = document.createElement("section");
      card.className = "prompt-document";
      const header = document.createElement("header");
      const name = document.createElement("input");
      name.className = "config-input";
      name.value = promptDocument.name.replace(/\.md$/i, "");
      name.setAttribute("aria-label", `${titleText}名称`);
      const remove = actionButton("删除", "text-button danger-text");
      remove.addEventListener("click", () => {
        const wasActive = configValue(activePath, "") === promptDocument.name;
        documents.splice(index, 1);
        if (wasActive) setConfigValue(activePath, "");
        markConfigDirty();
        renderPromptEditor();
        updateAdvancedConfigEditor();
      });
      header.append(configField("名称", name), remove);
      const content = document.createElement("textarea");
      content.className = "config-input prompt-content";
      content.rows = 10;
      content.value = promptDocument.content;
      content.setAttribute("aria-label", `${titleText}内容`);
      name.addEventListener("input", () => {
        const previous = promptDocument.name;
        promptDocument.name = normalizedDocumentName(name.value);
        if (configValue(activePath, "") === previous) setConfigValue(activePath, promptDocument.name);
        markConfigDirty();
        updateAdvancedConfigEditor();
      });
      content.addEventListener("input", () => { promptDocument.content = content.value; markConfigDirty(); });
      card.append(header, configField("内容", content));
      if (kind === "personas") {
        card.append(
          personaImageField(promptDocument, "avatar_path", "自定义头像图片", null),
          personaImageField(promptDocument, "board_image_path", "自定义看板图片", null),
          personaTextField(promptDocument, "board_title", "自定义看板大字", DEFAULT_BOARD_TITLE),
          personaTextField(promptDocument, "board_subtitle", "自定义看板小字", DEFAULT_BOARD_SUBTITLE)
        );
        const starterFields = document.createElement("div");
        starterFields.className = "persona-starter-fields";
        const values = Array.isArray(promptDocument.starter_prompts)
          ? DEFAULT_STARTER_PROMPTS.map((_, index) => String(promptDocument.starter_prompts[index] || ""))
          : DEFAULT_STARTER_PROMPTS.map(() => "");
        values.forEach((value, promptIndex) => {
          const input = document.createElement("input");
          input.className = "config-input";
          input.type = "text";
          input.maxLength = 200;
          input.value = value;
          input.placeholder = DEFAULT_STARTER_PROMPTS[promptIndex];
          input.setAttribute("aria-label", `预设问题 ${promptIndex + 1}`);
          input.addEventListener("input", () => {
            values[promptIndex] = input.value;
            promptDocument.starter_prompts = values.some((item) => item.trim()) ? [...values] : null;
            markConfigDirty();
          });
          starterFields.appendChild(input);
        });
        card.appendChild(configField("自定义预设问题", starterFields));
      }
      body.appendChild(card);
    }
    const add = actionButton("添加", "secondary-button compact-button");
    add.addEventListener("click", () => {
      const base = kind === "personas" ? "新建人格" : "新建身份";
      let name = `${base}.md`;
      let suffix = 2;
      while (documents.some((document) => document.name === name)) name = `${base} ${suffix++}.md`;
      documents.push({ name, content: "", avatar_path: null, original_name: null });
      setConfigValue(activePath, name);
      markConfigDirty();
      renderPromptEditor();
    });
    body.appendChild(add);
    return group;
  }

  function renderPromptEditor() {
    elements.promptEditor.replaceChildren(
      renderPromptCollection("personas", "AI 人格", "prompt.active_persona"),
      renderPromptCollection("identities", "用户身份", "prompt.active_identity")
    );
  }

  function renderConfigEditors() {
    if (!state.configLoaded || !state.configDraft) return;
    state.invalidConfigFields.clear();
    renderGeneralConfig();
    renderProviders();
    renderModelPools();
    renderPlugins();
    renderPromptEditor();
    updateAdvancedConfigEditor();
    updateSettingsControls();
  }

  function mapServerSecretStates(payload) {
    const providers = state.configDraft?.providers || [];
    state.providerSecretStates = providers.map((_, index) => Boolean(payload[`providers.${index}.api_key`]));
    const states = { ...payload };
    state.secretStates = states;
    refreshProviderSecretStates();
    return states;
  }

  // 配置文件会省略未修改的平台默认值；草稿仍需补齐真实语义，
  // 以免 WebUI 保存其他设置时覆盖通讯平台的默认策略。
  function ensurePlatformDefaults(draft) {
    if (!draft || typeof draft !== "object") return;
    draft.platforms = Object.assign({
      command_prefix: "/",
      commands: {}
    }, draft.platforms);
    const qq = Object.assign({
      enabled: false,
      reverse_ws_port: 8300,
      access_token: "",
      admin_users: [],
      allow_non_admin_host_tools: false,
      conversations: [],
      plugins: {},
      asset_base_url: "",
      max_reply_chars: 3000,
    }, draft.platforms.qq);
    qq.private_chats = Object.assign({
      whitelist: [],
      allow_non_whitelist: true,
      non_whitelist_rate_per_minute: 3
    }, qq.private_chats);
    qq.group_chats = Object.assign({
      whitelist: [],
      trigger_keywords: [],
      whitelist_rate_per_minute: 30,
      allow_non_whitelist: true,
      non_whitelist_rate_per_minute: 10
    }, qq.group_chats);
    draft.platforms.qq = qq;
  }

  function applyConfigPayload(payload) {
    state.configDraft = deepClone(payload?.config || {});
    ensurePlatformDefaults(state.configDraft);
    state.configOriginal = deepClone(payload?.config || {});
    state.promptDraft = deepClone(payload?.prompts || { personas: [], identities: [] });
    state.promptOriginal = deepClone(payload?.prompts || { personas: [], identities: [] });
    state.secretChanges = {};
    mapServerSecretStates(payload?.secret_states || {});
    state.configDirty = false;
    state.configLoaded = true;
    state.invalidConfigFields.clear();
    if (Array.isArray(payload?.models)) state.models = payload.models;
    state.configMultimodalModels = Array.isArray(payload?.multimodal_models) ? payload.multimodal_models : [];
    const providersById = new Map(
      (Array.isArray(state.configDraft?.providers) ? state.configDraft.providers : [])
        .map((provider) => [String(provider?.id || ""), provider])
    );
    state.configInferredImageModels = state.configMultimodalModels.filter((model) => {
      const provider = providersById.get(String(model?.provider_id || ""));
      const declared = provider?.model_modalities;
      return !(declared && typeof declared === "object"
        && Object.prototype.hasOwnProperty.call(declared, String(model?.model || "")));
    });
    if (payload?.display && typeof payload.display === "object") state.display = payload.display;
    if (payload?.context && typeof payload.context === "object") state.context = payload.context;
    if (payload?.persona) applyPersona(payload.persona);
    renderConfigEditors();
    renderModelMenu();
    updateContext();
  }

  async function loadConfigDraft() {
    if (state.configLoading || state.configSaving) return;
    if (state.configDirty && !window.confirm("放弃尚未保存的配置修改并重新载入？")) return;
    state.configLoading = true;
    updateSettingsControls();
    try {
      const response = await apiRequest("/api/config");
      applyConfigPayload(await response.json());
    } catch (error) {
      showToast(error.message || "配置载入失败", "error");
      elements.settingsStatus.textContent = error.message || "配置载入失败";
    } finally {
      state.configLoading = false;
      updateSettingsControls();
    }
  }

  function promptStateChanged() {
    if (!state.configOriginal || !state.promptOriginal) return false;
    const promptKeys = ["prompt", "system_prompt_file", "system_prompt"];
    const current = Object.fromEntries(promptKeys.map((key) => [key, state.configDraft?.[key]]));
    const original = Object.fromEntries(promptKeys.map((key) => [key, state.configOriginal?.[key]]));
    const withoutPersonaMetadata = (documents) => Object.fromEntries(
      Object.entries(documents || {}).map(([kind, items]) => [
        kind,
        (Array.isArray(items) ? items : []).map(({
          avatar_path: _avatarPath,
          board_image_path: _BoardImagePath,
          board_title: _BoardTitle,
          board_subtitle: _BoardSubtitle,
          starter_prompts: _StarterPrompts,
          ...document
        }) => document)
      ])
    );
    return JSON.stringify(current) !== JSON.stringify(original)
      || JSON.stringify(withoutPersonaMetadata(state.promptDraft)) !== JSON.stringify(withoutPersonaMetadata(state.promptOriginal));
  }

  function buildSecretMutations() {
    return { ...state.secretChanges };
  }

  async function saveConfigDraft() {
    if (!state.configLoaded || state.configSaving || state.configLoading || conversationRunning() || state.invalidConfigFields.size) return;
    const personaChanged = String(state.configDraft?.prompt?.active_persona || "")
      !== String(state.configOriginal?.prompt?.active_persona || "");
    state.configSaving = true;
    state.adminBusy = true;
    updateSettingsControls();
    updateControlState();
    try {
      const response = await apiRequest("/api/config", {
        method: "PUT",
        body: JSON.stringify({
          config: state.configDraft,
          secrets: buildSecretMutations(),
          prompts: state.promptDraft,
          reset_conversation: false
        })
      });
      applyConfigPayload(await response.json());
      if (personaChanged) await loadBootstrap();
      showToast("配置已保存");
    } catch (error) {
      showToast(error.message || "配置保存失败", "error");
      elements.settingsStatus.textContent = error.message || "配置保存失败";
    } finally {
      state.configSaving = false;
      state.adminBusy = false;
      updateSettingsControls();
      updateControlState();
    }
  }

  function applyAdvancedConfig() {
    try {
      const parsed = JSON.parse(elements.advancedConfigEditor.value);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("配置必须是 JSON 对象");
      const oldSecretStates = new Map((state.configDraft?.providers || []).map((provider, index) => [String(provider?.id || ""), Boolean(state.providerSecretStates[index])]));
      state.configDraft = parsed;
      ensurePlatformDefaults(state.configDraft);
      state.providerSecretStates = (Array.isArray(parsed.providers) ? parsed.providers : []).map((provider) => oldSecretStates.get(String(provider?.id || "")) || false);
      refreshProviderSecretStates();
      clearProviderSecretChanges();
      markConfigDirty();
      renderConfigEditors();
      showToast("完整配置已应用到草稿");
    } catch (error) {
      showToast(error.message || "JSON 无效", "error");
    }
  }

  async function readErrorMessage(response) {
    try {
      const payload = await response.json();
      const message = payload?.error?.message;
      if (typeof message === "string" && message.trim()) return message.trim();
    } catch (_) {
      // Fall through to an HTTP status message.
    }
    return `请求失败 (${response.status})`;
  }

  async function apiRequest(path, options = {}) {
    const headers = new Headers(options.headers || {});
    headers.set("Accept", "application/json");
    if (options.body != null && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
    let response;
    try {
      response = await fetch(path, { ...options, headers, credentials: "same-origin" });
    } catch (_) {
      throw new ApiError("无法连接 Miyu WebUI", 0);
    }
    if (!response.ok) throw new ApiError(await readErrorMessage(response), response.status);
    return response;
  }

  function asFiniteNumber(value, fallback = 0) {
    const number = Number(value);
    return Number.isFinite(number) ? number : fallback;
  }

  function formatInteger(value) {
    const number = Math.max(0, asFiniteNumber(value));
    try {
      return new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 0 }).format(number);
    } catch (_) {
      return String(Math.round(number));
    }
  }

  function formatTokens(value) {
    const number = Math.max(0, asFiniteNumber(value));
    if (number < 1000) return formatInteger(number);
    const useMillions = number >= 1_000_000;
    const amount = number / (useMillions ? 1_000_000 : 1000);
    const digits = amount >= 100 ? 0 : amount >= 10 ? 1 : 1;
    const suffix = useMillions ? "M" : "k";
    try {
      return `${new Intl.NumberFormat("zh-CN", { maximumFractionDigits: digits }).format(amount)}${suffix}`;
    } catch (_) {
      return `${amount.toFixed(digits)}${suffix}`;
    }
  }

  function parseDate(value) {
    if (value == null || value === "") return null;
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? null : date;
  }

  function formatTime(value) {
    const date = parseDate(value);
    if (!date) return "";
    try {
      return new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false }).format(date);
    } catch (_) {
      return date.toLocaleTimeString?.() || "";
    }
  }

  function formatDateTime(value) {
    const date = parseDate(value);
    if (!date) return "";
    try {
      return new Intl.DateTimeFormat("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false
      }).format(date);
    } catch (_) {
      return date.toLocaleString?.() || "";
    }
  }

  function formatRelativeTime(value) {
    const date = parseDate(value);
    if (!date) return "";
    const difference = Date.now() - date.getTime();
    if (difference >= 0 && difference < 60_000) return "刚刚";
    if (difference >= 0 && difference < 3_600_000) return `${Math.max(1, Math.floor(difference / 60_000))} 分钟前`;
    const now = new Date();
    if (date.toDateString() === now.toDateString()) return formatTime(date);
    try {
      return new Intl.DateTimeFormat("zh-CN", { month: "numeric", day: "numeric" }).format(date);
    } catch (_) {
      return date.toLocaleDateString?.() || "";
    }
  }

  function dayKey(value) {
    const date = parseDate(value);
    if (!date) return "unknown";
    return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
  }

  function formatDayLabel(value) {
    const date = parseDate(value);
    if (!date) return "较早";
    const today = new Date();
    const yesterday = new Date(today);
    yesterday.setDate(today.getDate() - 1);
    if (date.toDateString() === today.toDateString()) return "今天";
    if (date.toDateString() === yesterday.toDateString()) return "昨天";
    try {
      return new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "long", day: "numeric" }).format(date);
    } catch (_) {
      return date.toLocaleDateString?.() || "较早";
    }
  }

  function firstLine(value) {
    return String(value || "").split(/\r?\n/, 1)[0].trim();
  }

  function modelMark(model) {
    const source = String(model?.provider_name || model?.provider_id || model?.model || "").trim();
    if (!source) return "--";
    const words = source.split(/[\s._/-]+/).filter(Boolean);
    const mark = words.length > 1 ? `${words[0][0] || ""}${words[1][0] || ""}` : source.slice(0, 2);
    return mark.toLocaleUpperCase("en-US");
  }

  function modelKey(model) {
    return JSON.stringify([String(model?.provider_id || ""), String(model?.model || "")]);
  }

  function effectiveUsageTotal(usage) {
    if (!usage || typeof usage !== "object") return 0;
    const explicit = asFiniteNumber(usage.total_tokens, 0);
    return explicit > 0 ? explicit : asFiniteNumber(usage.prompt_tokens, 0) + asFiniteNumber(usage.completion_tokens, 0);
  }

  function setConnectionStatus(status) {
    state.connection = status;
    const definitions = {
      online: { sidebar: "在线", className: "" },
      connecting: { sidebar: "重连中", className: "is-connecting" },
      offline: { sidebar: "离线", className: "is-offline" },
      blocked: { sidebar: "未授权", className: "is-blocked" }
    };
    const selected = definitions[status] || definitions.connecting;
    elements.sidebarConnectionStatus.textContent = selected.sidebar;
    elements.sidebarStatusDot.classList.remove("is-connecting", "is-offline", "is-blocked");
    if (selected.className) elements.sidebarStatusDot.classList.add(selected.className);
  }

  function updateContext() {
    const tokens = Math.max(0, asFiniteNumber(state.context?.tokens));
    const windowSize = state.context?.window == null ? null : Math.max(0, asFiniteNumber(state.context.window));
    elements.contextNumbers.textContent = windowSize ? `${formatTokens(tokens)} / ${formatTokens(windowSize)}` : `${formatTokens(tokens)} / --`;
    const percent = windowSize > 0 ? Math.min(100, Math.max(0, (tokens / windowSize) * 100)) : 0;
    elements.contextBar.style.width = `${percent}%`;
    elements.contextTrack.setAttribute("aria-valuenow", String(Math.round(percent)));
    elements.contextTrack.setAttribute("aria-label", windowSize ? `上下文使用 ${Math.round(percent)}%` : `上下文 ${formatInteger(tokens)} tokens`);
    elements.contextTrack.classList.toggle("is-high", percent >= 75 && percent < 90);
    elements.contextTrack.classList.toggle("is-critical", percent >= 90);
  }

  function updateRuntimeUsage() {}

  function updateCapabilities() {
    const values = [
      ["会话", state.capabilities?.multi_conversation ? "多会话" : "当前单一对话"],
      ["附件", state.capabilities?.attachments ? "可用" : "不可用"],
      ["消息队列", state.capabilities?.queue ? "可用" : "不可用"]
    ];
    elements.capabilityList.replaceChildren();
    for (const [name, value] of values) {
      const row = document.createElement("div");
      const term = document.createElement("dt");
      const description = document.createElement("dd");
      term.textContent = name;
      description.textContent = value;
      row.append(term, description);
      elements.capabilityList.appendChild(row);
    }
  }

  function activeModels() {
    return state.models.filter((model) => model?.active);
  }

  function updateCurrentModelDisplay() {
    const active = activeModels();
    if (active.length === 0) {
      elements.modelMark.textContent = "--";
      elements.modelLabel.textContent = state.models.length ? "未选择模型" : "未配置模型";
      elements.modelLabel.title = elements.modelLabel.textContent;
      elements.settingsModelMark.textContent = "--";
      elements.settingsModelName.textContent = elements.modelLabel.textContent;
      elements.settingsModelProvider.textContent = "--";
      return;
    }
    if (active.length > 1) {
      const title = active.map((model) => `${model.provider_name || model.provider_id || ""} · ${model.model || ""}`).join("\n");
      elements.modelMark.textContent = "MX";
      elements.modelLabel.textContent = `混合模型 · ${active.length}`;
      elements.modelLabel.title = title;
      elements.settingsModelMark.textContent = "MX";
      elements.settingsModelName.textContent = "混合模型";
      elements.settingsModelProvider.textContent = `${active.length} 个活动端点`;
      return;
    }
    const selected = active[0];
    const mark = modelMark(selected);
    elements.modelMark.textContent = mark;
    elements.modelLabel.textContent = String(selected.model || "");
    elements.modelLabel.title = `${selected.provider_name || selected.provider_id || ""} · ${selected.model || ""}`;
    elements.settingsModelMark.textContent = mark;
    elements.settingsModelName.textContent = String(selected.model || "");
    elements.settingsModelProvider.textContent = String(selected.provider_name || selected.provider_id || "");
  }

  function refreshLiveEndpointVisibility() {
    for (const live of state.liveRuns.values()) {
      if (!live.endpoint) continue;
      const values = [live.providerId, live.model].map((value) => String(value || "").trim()).filter(Boolean);
      live.endpoint.hidden = !state.display?.show_mixed_model_endpoint || values.length === 0;
    }
  }

  function renderModelMenu() {
    elements.modelMenu.replaceChildren();
    const staged = state.stagedModelKeys instanceof Set
      ? state.stagedModelKeys
      : new Set(activeModels().map(modelKey));
    const list = document.createElement("div");
    list.className = "model-menu-list";
    list.setAttribute("role", "group");
    list.setAttribute("aria-label", "可用模型");
    for (const model of state.models) {
      if (!model || typeof model !== "object") continue;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "model-menu-item";
      button.setAttribute("role", "menuitemcheckbox");
      button.dataset.modelKey = modelKey(model);
      const selected = staged.has(button.dataset.modelKey);
      button.setAttribute("aria-checked", String(selected));
      button.classList.toggle("selected", selected);

      const mark = document.createElement("span");
      mark.className = "model-mark";
      mark.textContent = modelMark(model);
      const copy = document.createElement("span");
      copy.className = "model-menu-copy";
      const name = document.createElement("strong");
      name.textContent = String(model.model || "");
      const provider = document.createElement("small");
      provider.textContent = String(model.provider_name || model.provider_id || "");
      copy.append(name, provider);
      const check = document.createElement("span");
      check.className = "icon-slot check-slot";
      check.setAttribute("aria-hidden", "true");
      if (selected) check.appendChild(createIcon("check"));
      button.append(mark, copy, check);
      button.addEventListener("click", () => toggleStagedModel(button.dataset.modelKey));
      list.appendChild(button);
    }

    const footer = document.createElement("footer");
    footer.className = "model-menu-footer";
    footer.setAttribute("role", "none");
    const feedback = document.createElement("span");
    feedback.className = "model-menu-feedback";
    feedback.setAttribute("role", "status");
    feedback.setAttribute("aria-live", "polite");
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "model-cancel";
    cancel.setAttribute("role", "menuitem");
    cancel.textContent = "取消";
    cancel.addEventListener("click", () => closeModelMenu({ restoreFocus: true }));
    const confirm = document.createElement("button");
    confirm.type = "button";
    confirm.className = "model-confirm";
    confirm.setAttribute("role", "menuitem");
    confirm.textContent = "确认";
    confirm.addEventListener("click", confirmModelSelection);
    footer.append(feedback, cancel, confirm);
    elements.modelMenu.append(list, footer);
    updateModelMenuState();
    updateCurrentModelDisplay();
    refreshLiveEndpointVisibility();
    updateControlState();
  }

  function updateModelMenuState() {
    const staged = state.stagedModelKeys instanceof Set
      ? state.stagedModelKeys
      : new Set(activeModels().map(modelKey));
    elements.modelMenu.querySelectorAll(".model-menu-item").forEach((button) => {
      const selected = staged.has(button.dataset.modelKey || "");
      button.classList.toggle("selected", selected);
      button.setAttribute("aria-checked", String(selected));
      const check = button.querySelector(".check-slot");
      if (check) check.replaceChildren(...(selected ? [createIcon("check")] : []));
    });
    const feedback = elements.modelMenu.querySelector(".model-menu-feedback");
    if (feedback) {
      const empty = staged.size === 0;
      feedback.textContent = state.modelMenuError || (empty ? "至少选择一个模型" : `已选择 ${formatInteger(staged.size)} 个模型`);
      feedback.classList.toggle("is-error", Boolean(state.modelMenuError) || empty);
    }
    const confirm = elements.modelMenu.querySelector(".model-confirm");
    if (confirm) {
      confirm.textContent = state.modelSelectionSubmitting ? "正在应用" : "确认";
      confirm.disabled = state.modelSelectionSubmitting || state.adminBusy || state.blocked || conversationRunning() || state.submitting || staged.size === 0;
    }
    const cancel = elements.modelMenu.querySelector(".model-cancel");
    if (cancel) cancel.disabled = state.modelSelectionSubmitting || (state.adminBusy && !state.modelSelectionSubmitting);
  }

  function toggleStagedModel(key) {
    if (!(state.stagedModelKeys instanceof Set) || state.modelSelectionSubmitting) return;
    if (state.stagedModelKeys.has(key)) state.stagedModelKeys.delete(key);
    else state.stagedModelKeys.add(key);
    state.modelMenuError = state.stagedModelKeys.size === 0 ? "至少选择一个模型" : "";
    updateModelMenuState();
  }

  function newestLiveRun() {
    let latest = null;
    for (const live of state.liveRuns.values()) latest = live;
    return latest;
  }

  function deriveConversationDetails() {
    const live = newestLiveRun();
    if (state.turns.length === 0) {
      const liveUser = live?.userText || state.pendingSubmission?.content || "";
      if (!liveUser) return { title: "新对话", snippet: "尚未开始", timestamp: null };
      return { title: firstLine(liveUser) || "新对话", snippet: firstLine(liveUser), timestamp: new Date() };
    }
    const firstTurn = state.turns[0];
    const lastTurn = state.turns[state.turns.length - 1];
    const followups = Array.isArray(lastTurn?.followups) ? lastTurn.followups : [];
    const lastFollowup = followups[followups.length - 1];
    const assistant = String(lastTurn?.assistant_content || "").trim();
    const liveContent = live ? String(live.userText || "").trim() : "";
    const snippet = firstLine(liveContent || assistant || lastFollowup?.content || lastTurn?.user_content || "");
    const timestamp = liveContent ? live?.startedAt : lastTurn?.assistant_timestamp || lastFollowup?.submitted_at || lastTurn?.user_timestamp;
    return {
      title: firstLine(firstTurn?.user_content) || "当前对话",
      snippet: snippet || (lastTurn?.status === "running" ? "正在回复" : "对话已开始"),
      timestamp
    };
  }

  function multiSessionEnabled() {
    return Boolean(state.capabilities?.multi_conversation);
  }

  function sessionDisplayName(session) {
    const name = firstLine(session?.name || "");
    return name || "新会话";
  }

  function findSession(sessionId) {
    const id = String(sessionId || "");
    return state.sessions.find((session) => String(session?.session_id) === id) || null;
  }

  function findArchivedSession(sessionId) {
    const id = String(sessionId || "");
    return state.archivedSessions.find((session) => String(session?.session_id) === id) || null;
  }

  function viewSessionEntry() {
    return state.viewSessionId ? findSession(state.viewSessionId) : null;
  }

  function trackRun(sessionId, runId) {
    const session = String(sessionId || "");
    const run = String(runId || "");
    if (!session || !run) return;
    let runs = state.runsBySession.get(session);
    if (!runs) {
      runs = new Set();
      state.runsBySession.set(session, runs);
    }
    runs.add(run);
  }

  function untrackRun(runId) {
    const run = String(runId || "");
    for (const [sessionId, runs] of state.runsBySession) {
      if (runs.delete(run) && runs.size === 0) state.runsBySession.delete(sessionId);
    }
  }

  function runSessionId(runId) {
    const run = String(runId || "");
    if (!run) return "";
    for (const [sessionId, runs] of state.runsBySession) {
      if (runs.has(run)) return sessionId;
    }
    return "";
  }

  function sessionHasRuns(sessionId) {
    return (state.runsBySession.get(String(sessionId || ""))?.size || 0) > 0;
  }

  function closeSessionMenu() {
    if (!state.sessionMenuFor) return;
    state.sessionMenuFor = null;
    renderSessionList();
  }

  function toggleSessionMenu(sessionId) {
    state.sessionMenuFor = state.sessionMenuFor === sessionId ? null : sessionId;
    renderSessionList();
    if (!state.sessionMenuFor) return;
    const item = elements.sessionItems.querySelector(`.session-item[data-session-id="${CSS.escape(sessionId)}"]`);
    const menu = item?.querySelector(".session-menu");
    if (menu) {
      const menuRect = menu.getBoundingClientRect();
      const listRect = elements.sessionList.getBoundingClientRect();
      if (menuRect.bottom > listRect.bottom - 4) menu.classList.add("open-up");
      window.requestAnimationFrame(() => menu.querySelector("button")?.focus());
    }
  }

  function beginSessionRename(sessionId) {
    state.sessionRenaming = sessionId;
    renderSessionList();
  }

  function cancelSessionRename() {
    state.sessionRenaming = null;
    renderSessionList();
  }

  async function commitSessionRename(sessionId, value) {
    if (state.sessionRenaming !== sessionId) return;
    state.sessionRenaming = null;
    const session = findSession(sessionId) || findArchivedSession(sessionId);
    const name = String(value || "").trim();
    if (!session || !name || name === String(session.name || "").trim()) {
      renderSessionList();
      return;
    }
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}`, {
        method: "PATCH",
        body: JSON.stringify({ name })
      });
      session.name = name;
      showToast("会话已重命名");
    } catch (error) {
      showToast(error.message || "重命名失败", "error");
    }
    renderSessionList();
    renderArchivedList();
    if (sessionId === state.viewSessionId) updateConversationChrome();
  }

  function buildSessionMenu(session, isDefault) {
    const id = String(session?.session_id || "");
    const menu = document.createElement("div");
    menu.className = "session-menu";
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", `会话操作：${sessionDisplayName(session)}`);
    const actions = [{ label: "重命名", handler: () => beginSessionRename(id) }];
    if (!isDefault) actions.push({ label: "设为默认", handler: () => makeDefaultSession(id) });
    if (isDefault) actions.push({ label: "清空对话", handler: requestClearConversation });
    actions.push({ label: "归档", handler: () => archiveSession(id) });
    actions.push({ label: "删除", danger: true, handler: () => deleteSession(id) });
    for (const action of actions) {
      const button = document.createElement("button");
      button.type = "button";
      button.setAttribute("role", "menuitem");
      if (action.danger) button.classList.add("is-danger");
      button.textContent = action.label;
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        closeSessionMenu();
        action.handler();
      });
      menu.appendChild(button);
    }
    return menu;
  }

  function buildSessionItem(session) {
    const id = String(session?.session_id || "");
    const isView = Boolean(id) && id === state.viewSessionId;
    const isDefault = Boolean(id) && id === state.currentSessionId;
    const item = document.createElement("div");
    item.className = `session-item${isView ? " active" : ""}`;
    item.dataset.sessionId = id;

    const renaming = state.sessionRenaming === id;
    const main = document.createElement(renaming ? "div" : "button");
    main.className = `session-item-main${renaming ? " is-renaming" : ""}`;
    if (!renaming) {
      main.type = "button";
      main.title = isView ? sessionDisplayName(session) : `查看「${sessionDisplayName(session)}」`;
      main.addEventListener("click", () => openSessionView(id));
    }
    main.appendChild(makeIconSlot("message-circle"));

    const copy = document.createElement("span");
    copy.className = "session-copy";
    if (renaming) {
      const input = document.createElement("input");
      input.className = "session-rename-input";
      input.type = "text";
      input.value = String(session?.name || "");
      input.maxLength = 200;
      input.setAttribute("aria-label", "会话名称");
      input.addEventListener("click", (event) => event.stopPropagation());
      input.addEventListener("keydown", (event) => {
        event.stopPropagation();
        if (event.key === "Enter") {
          event.preventDefault();
          commitSessionRename(id, input.value);
        } else if (event.key === "Escape") {
          event.preventDefault();
          cancelSessionRename();
        }
      });
      input.addEventListener("blur", () => {
        if (state.sessionRenaming === id) commitSessionRename(id, input.value);
      });
      copy.appendChild(input);
      window.requestAnimationFrame(() => {
        input.focus();
        input.select();
      });
    } else {
      const titleRow = document.createElement("span");
      titleRow.className = "session-title-row";
      const title = document.createElement("strong");
      title.textContent = sessionDisplayName(session);
      titleRow.appendChild(title);
      if (isDefault) {
        const badge = document.createElement("span");
        badge.className = "session-default-badge";
        badge.textContent = "默认";
        badge.title = "CLI 与快捷入口的默认会话";
        titleRow.appendChild(badge);
      }
      copy.appendChild(titleRow);
    }

    // Gemini-style list rows: name only; details live in the hover tooltip.
    if (!renaming) {
      const snippet = firstLine(session?.last_user_content || "");
      const workspace = String(session?.workspace || "").trim();
      const details = [snippet, workspace].filter(Boolean).join("\n");
      if (details) {
        main.title = `${sessionDisplayName(session)}\n${details}`;
      }
    }

    main.appendChild(copy);
    item.appendChild(main);

    const trailing = document.createElement("span");
    trailing.className = "session-trailing";

    if (sessionHasRuns(id)) {
      const dot = document.createElement("span");
      dot.className = "session-run-dot";
      dot.title = "有回复正在运行";
      trailing.appendChild(dot);
    }

    const menuButton = document.createElement("button");
    menuButton.type = "button";
    menuButton.className = "session-menu-button";
    menuButton.title = "会话操作";
    menuButton.setAttribute("aria-label", `会话操作：${sessionDisplayName(session)}`);
    menuButton.setAttribute("aria-haspopup", "menu");
    menuButton.setAttribute("aria-expanded", String(state.sessionMenuFor === id));
    menuButton.appendChild(makeIconSlot("ellipsis"));
    menuButton.addEventListener("click", (event) => {
      event.stopPropagation();
      toggleSessionMenu(id);
    });
    trailing.appendChild(menuButton);
    item.appendChild(trailing);

    if (state.sessionMenuFor === id) item.appendChild(buildSessionMenu(session, isDefault));
    return item;
  }

  function buildFallbackSessionItem() {
    const details = deriveConversationDetails();
    const item = document.createElement("div");
    item.className = "session-item active";
    const main = document.createElement("button");
    main.type = "button";
    main.className = "session-item-main";
    main.title = details.title;
    main.appendChild(makeIconSlot("message-circle"));
    const copy = document.createElement("span");
    copy.className = "session-copy";
    const title = document.createElement("strong");
    title.textContent = details.title;
    const snippet = document.createElement("small");
    snippet.className = "session-snippet";
    snippet.textContent = details.snippet;
    snippet.title = details.snippet;
    copy.append(title, snippet);
    main.appendChild(copy);
    main.addEventListener("click", () => {
      closeSidebar();
      scrollToBottom({ force: true, smooth: true });
    });
    item.appendChild(main);
    const trailing = document.createElement("span");
    trailing.className = "session-trailing";
    const time = document.createElement("span");
    time.className = "session-time";
    time.textContent = details.timestamp ? formatRelativeTime(details.timestamp) : "";
    trailing.appendChild(time);
    item.appendChild(trailing);
    return item;
  }

  function renderSessionList() {
    if (!elements.sessionItems) return;
    if (state.sessionRenaming && elements.sessionItems.querySelector(".session-rename-input")) return;
    elements.sessionItems.replaceChildren();
    if (!multiSessionEnabled() || state.sessions.length === 0) {
      elements.sessionItems.appendChild(buildFallbackSessionItem());
      elements.archivedSection.hidden = !multiSessionEnabled();
      return;
    }
    for (const session of state.sessions) {
      if (session?.archived) continue;
      elements.sessionItems.appendChild(buildSessionItem(session));
    }
    elements.archivedSection.hidden = false;
  }

  function renderArchivedList() {
    elements.archivedToggle.setAttribute("aria-expanded", String(state.archivedOpen));
    elements.archivedToggle.classList.toggle("is-open", state.archivedOpen);
    elements.archivedList.hidden = !state.archivedOpen;
    if (!state.archivedOpen) return;
    elements.archivedList.replaceChildren();
    if (state.archivedLoading) {
      const note = document.createElement("p");
      note.className = "archived-note";
      note.textContent = "正在载入";
      elements.archivedList.appendChild(note);
      return;
    }
    if (state.archivedSessions.length === 0) {
      const note = document.createElement("p");
      note.className = "archived-note";
      note.textContent = "暂无已归档会话";
      elements.archivedList.appendChild(note);
      return;
    }
    for (const session of state.archivedSessions) {
      const id = String(session?.session_id || "");
      const row = document.createElement("div");
      row.className = "archived-item";
      const copy = document.createElement("span");
      copy.className = "archived-copy";
      const title = document.createElement("strong");
      title.textContent = sessionDisplayName(session);
      title.title = sessionDisplayName(session);
      const meta = document.createElement("small");
      const workspace = String(session?.workspace || "").trim();
      const turnCount = Math.max(0, asFiniteNumber(session?.turn_count));
      meta.textContent = workspace ? `${formatInteger(turnCount)} 轮 · ${workspace}` : `${formatInteger(turnCount)} 轮`;
      if (workspace) meta.title = workspace;
      copy.append(title, meta);
      row.appendChild(copy);
      const actions = document.createElement("span");
      actions.className = "archived-actions";
      const restore = document.createElement("button");
      restore.type = "button";
      restore.className = "text-button";
      restore.textContent = "恢复";
      restore.addEventListener("click", () => restoreSession(id));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "text-button danger-text";
      remove.textContent = "删除";
      remove.addEventListener("click", () => deleteSession(id));
      actions.append(restore, remove);
      row.appendChild(actions);
      elements.archivedList.appendChild(row);
    }
  }

  async function loadArchivedSessions() {
    if (state.archivedLoading) return;
    state.archivedLoading = true;
    renderArchivedList();
    try {
      const response = await apiRequest("/api/sessions?include_archived=true");
      const payload = await response.json();
      const sessions = Array.isArray(payload?.sessions) ? payload.sessions : [];
      state.archivedSessions = sessions.filter((session) => session?.archived);
    } catch (error) {
      showToast(error.message || "载入归档会话失败", "error");
    } finally {
      state.archivedLoading = false;
      renderArchivedList();
    }
  }

  function toggleArchivedSection() {
    state.archivedOpen = !state.archivedOpen;
    renderArchivedList();
    if (state.archivedOpen) loadArchivedSessions();
  }

  async function refreshSessions() {
    try {
      const response = await apiRequest("/api/sessions?include_archived=true");
      const payload = await response.json();
      const sessions = Array.isArray(payload?.sessions) ? payload.sessions : [];
      state.sessions = sessions.filter((session) => !session?.archived);
      state.archivedSessions = sessions.filter((session) => session?.archived);
      renderSessionList();
      renderArchivedList();
      updateConversationChrome();
    } catch (_) {
      // 后续 SSE 或 bootstrap 会补齐会话列表。
    }
  }

  function setSessionBusy(value) {
    state.sessionBusy = Boolean(value);
    updateControlState();
  }

  async function createSession() {
    if (state.blocked || state.sessionBusy || state.adminBusy || state.submitting) return;
    setSessionBusy(true);
    try {
      const response = await apiRequest("/api/sessions", {
        method: "POST",
        body: JSON.stringify({})
      });
      const payload = await response.json();
      const record = payload?.session && typeof payload.session === "object" ? payload.session : null;
      const sessionId = String(record?.session_id || "");
      if (sessionId && !findSession(sessionId)) {
        state.sessions.unshift(record);
        renderSessionList();
      }
      if (sessionId) await loadSessionView(sessionId);
      focusComposerIfDesktop();
    } catch (error) {
      showToast(error.message || "新建会话失败", "error");
    } finally {
      setSessionBusy(false);
    }
  }

  async function openSessionView(sessionId) {
    if (!sessionId) return;
    if (sessionId === state.viewSessionId) {
      closeSidebar();
      scrollToBottom({ force: true, smooth: true });
      return;
    }
    await loadSessionView(sessionId);
  }

  async function loadSessionView(sessionId, { quiet = false } = {}) {
    if (!sessionId || state.viewLoading) return;
    state.viewLoading = true;
    try {
      const response = await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}/turns`);
      applySessionView(await response.json());
      if (!quiet) closeSidebar();
    } catch (error) {
      if (error.status === 401) showBlockedState(true);
      else if (error.status === 404) {
        showToast("会话不存在", "error");
        refreshSessions();
        if (sessionId === state.viewSessionId) window.setTimeout(() => openFallbackSessionView(sessionId), 0);
      } else showToast(error.message || "载入会话失败", "error");
    } finally {
      state.viewLoading = false;
      updateControlState();
    }
  }

  function disposeAllLiveRuns() {
    for (const live of state.liveRuns.values()) disposeLiveState(live);
    state.liveRuns.clear();
    elements.liveStopRail.replaceChildren();
    elements.liveStopRail.hidden = true;
  }

  function applySessionView(payload) {
    const sessionId = String(payload?.session_id || "");
    if (!sessionId) return;
    disposeAllLiveRuns();
    clearViewSyncTimer();
    state.viewSessionId = sessionId;
    state.turns = Array.isArray(payload?.turns)
      ? payload.turns.sort((a, b) => asFiniteNumber(a?.seq) - asFiniteNumber(b?.seq))
      : [];
    state.queuedPrompts = Array.isArray(payload?.queued_prompts) ? payload.queued_prompts : [];
    state.pendingSubmission = null;
    const runs = (Array.isArray(payload?.runs) ? payload.runs : []).filter((run) => run?.run_id);
    if (runs.length) state.runsBySession.set(sessionId, new Set(runs.map((run) => String(run.run_id))));
    else state.runsBySession.delete(sessionId);
    state.viewRunningTurnId = !runs.length && typeof payload?.running_turn_id === "string" && payload.running_turn_id
      ? payload.running_turn_id
      : null;
    renderConversation();
    renderQueueTray();
    restoreLiveRuns(runs);
    updateConversationChrome();
    updateControlState();
    scheduleViewSync();
  }

  function findUnclaimedRunningTurn() {
    const claimed = new Set();
    for (const live of state.liveRuns.values()) {
      if (live.turnId) claimed.add(String(live.turnId));
    }
    return state.turns.find((turn) => turn?.status === "running" && !claimed.has(String(turn?.id))) || null;
  }

  function createLiveForRun(runId, userText = "", { claimTurn = true } = {}) {
    const existing = state.liveRuns.get(runId);
    if (existing) return existing;
    const runningTurn = userText || !claimTurn ? null : findUnclaimedRunningTurn();
    const live = createLiveState(runId, {
      turnId: runningTurn?.id || null,
      userText: userText || runningTurn?.user_content || "",
      startedAt: runningTurn?.user_timestamp || new Date(),
      userRendered: Boolean(runningTurn)
    });
    state.liveRuns.set(runId, live);
    return live;
  }

  function beginRunReplay() {
    state.replayRunIds = new Set(state.liveRuns.keys());
    state.replayCutoff = Math.max(state.lastEventId, state.replayCutoff, state.latestEventId);
    state.lastEventId = 0;
    connectEventSource(0);
  }

  function restoreLiveRuns(runs) {
    let restored = false;
    for (const run of runs) {
      const runId = String(run?.run_id || "");
      if (!runId || state.terminalRunIds.has(runId)) continue;
      createLiveForRun(runId);
      restored = true;
    }
    if (restored) beginRunReplay();
  }

  async function openFallbackSessionView(excludedSessionId) {
    const excluded = String(excludedSessionId || "");
    const fallback = state.currentSessionId && state.currentSessionId !== excluded
      ? state.currentSessionId
      : String(state.sessions.find((session) => String(session?.session_id) !== excluded)?.session_id || "");
    if (fallback) await loadSessionView(fallback, { quiet: true });
    else await loadBootstrap();
  }

  async function makeDefaultSession(sessionId) {
    if (!sessionId || state.sessionBusy) return;
    setSessionBusy(true);
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}/activate`, { method: "POST" });
      state.currentSessionId = sessionId;
      renderSessionList();
      showToast("已设为默认会话");
    } catch (error) {
      showToast(error.message || "设为默认失败", "error");
    } finally {
      setSessionBusy(false);
    }
  }

  async function archiveSession(sessionId) {
    if (state.sessionBusy) return;
    setSessionBusy(true);
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}`, {
        method: "PATCH",
        body: JSON.stringify({ archived: true })
      });
      showToast("会话已归档");
      state.sessions = state.sessions.filter((session) => String(session?.session_id) !== String(sessionId));
      renderSessionList();
      if (sessionId === state.viewSessionId) await openFallbackSessionView(sessionId);
      if (state.archivedOpen) await loadArchivedSessions();
    } catch (error) {
      showToast(error.message || "归档失败", "error");
    } finally {
      setSessionBusy(false);
    }
  }

  async function restoreSession(sessionId) {
    if (state.sessionBusy) return;
    setSessionBusy(true);
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}`, {
        method: "PATCH",
        body: JSON.stringify({ archived: false })
      });
      showToast("会话已恢复");
      await refreshSessions();
    } catch (error) {
      showToast(error.message || "恢复失败", "error");
    } finally {
      setSessionBusy(false);
    }
  }

  async function deleteSession(sessionId) {
    const session = findSession(sessionId) || findArchivedSession(sessionId);
    if (!window.confirm(`删除会话「${sessionDisplayName(session)}」？此操作无法撤销。`)) return;
    if (state.sessionBusy) return;
    setSessionBusy(true);
    try {
      await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}`, { method: "DELETE" });
      showToast("会话已删除");
      state.sessions = state.sessions.filter((item) => String(item?.session_id) !== String(sessionId));
      state.archivedSessions = state.archivedSessions.filter((item) => String(item?.session_id) !== String(sessionId));
      renderSessionList();
      renderArchivedList();
      if (sessionId === state.viewSessionId) await openFallbackSessionView(sessionId);
    } catch (error) {
      showToast(error.message || "删除失败", "error");
    } finally {
      setSessionBusy(false);
    }
  }

  function handleSessionEvent(name, data) {
    const sessionId = String(data?.session_id || "");
    if (!sessionId) return;
    if (name === "session.created") {
      if (data?.platform) return;
      if (!findSession(sessionId) && !findArchivedSession(sessionId)) {
        state.sessions.unshift({
          session_id: sessionId,
          name: String(data?.name || ""),
          kind: "",
          workspace: "",
          archived: false,
          created_at: null,
          updated_at: new Date().toISOString(),
          turn_count: 0,
          last_user_content: ""
        });
        renderSessionList();
      }
    } else if (name === "session.renamed") {
      const target = findSession(sessionId) || findArchivedSession(sessionId);
      if (target) target.name = String(data?.name || "");
      renderSessionList();
      renderArchivedList();
      if (sessionId === state.viewSessionId) updateConversationChrome();
    } else if (name === "session.archived") {
      refreshSessions();
    } else if (name === "session.deleted") {
      state.sessions = state.sessions.filter((item) => String(item?.session_id) !== sessionId);
      state.archivedSessions = state.archivedSessions.filter((item) => String(item?.session_id) !== sessionId);
      renderSessionList();
      renderArchivedList();
      if (sessionId === state.viewSessionId && !state.bootstrapPromise && !state.viewLoading) {
        openFallbackSessionView(sessionId);
      }
    } else if (name === "session.updated") {
      const target = findSession(sessionId) || findArchivedSession(sessionId);
      if (target) target.workspace = String(data?.workspace || "");
      renderSessionList();
      if (sessionId === state.viewSessionId) updateConversationChrome();
    } else if (name === "session.current_changed") {
      // 每视图独立浏览：默认会话只影响侧栏「默认」徽标，不再跟随切换。
      state.currentSessionId = sessionId;
      renderSessionList();
    }
  }

  function updateConversationChrome() {
    const details = deriveConversationDetails();
    const current = multiSessionEnabled() ? viewSessionEntry() : null;
    const title = current ? sessionDisplayName(current) : details.title;
    elements.conversationTitle.textContent = title;
    elements.conversationTitle.title = title;
    const workspace = String(current?.workspace || "").trim();
    let meta;
    if (conversationRunning()) {
      meta = state.liveRuns.size > 1 ? `${formatInteger(state.liveRuns.size)} 路回复进行中` : "正在回复";
    } else meta = details.timestamp ? formatRelativeTime(details.timestamp) : "尚未开始";
    elements.conversationMeta.textContent = workspace ? `${meta} · ${workspace}` : meta;
    elements.conversationMeta.title = workspace;
    renderSessionList();
  }

  function conversationRunning() {
    return state.liveRuns.size > 0 || Boolean(state.viewRunningTurnId);
  }

  function hasPendingQuestion() {
    for (const live of state.liveRuns.values()) {
      for (const question of live.questions.values()) {
        if (question.pending) return true;
      }
    }
    return false;
  }

  function countCharacters(value) {
    return Array.from(String(value || "")).length;
  }

  // 触屏设备上程序化聚焦会弹出软键盘挡住内容，只在桌面端自动聚焦
  function focusComposerIfDesktop() {
    if (window.matchMedia("(hover: none), (pointer: coarse)").matches) return;
    elements.composerInput.focus();
  }

  function resizeComposer() {
    const input = elements.composerInput;
    input.style.height = "auto";
    input.style.height = `${Math.min(input.scrollHeight, window.innerWidth <= 760 ? 120 : 146)}px`;
    const count = countCharacters(input.value);
    elements.characterCount.textContent = `${formatInteger(count)} / 20,000`;
    elements.characterCount.hidden = count < 18_000;
    elements.characterCount.classList.toggle("is-error", count > MAX_CONTENT_CHARS);
    updateControlState();
    window.requestAnimationFrame(updateJumpButtonOffset);
  }

  function updateJumpButtonOffset() {
    elements.jumpBottomButton.style.bottom = `${elements.composerDock.offsetHeight + 10}px`;
  }

  function updateControlState() {
    const running = conversationRunning();
    const busy = state.adminBusy || state.submitting;
    const locked = state.blocked || state.adminBusy;
    const inputCount = countCharacters(elements.composerInput.value.trim());

    elements.composerInput.disabled = locked;
    elements.composerForm.classList.toggle("is-disabled", locked);
    elements.newChatButton.disabled = state.blocked || busy || state.sessionBusy || state.viewLoading;
    elements.modelButton.disabled = state.blocked || running || busy || state.models.length === 0;
    elements.modeSwitch.querySelectorAll("button").forEach((button) => {
      button.disabled = state.blocked || running || busy;
    });
    elements.promptGrid.querySelectorAll("button").forEach((button) => {
      button.disabled = state.blocked || running || busy;
    });
    elements.modelMenu.querySelectorAll(".model-menu-item").forEach((button) => {
      button.disabled = state.blocked || running || busy;
    });
    updateModelMenuState();

    elements.sendButton.classList.remove("is-cancel");
    elements.sendButton.querySelector(".icon-slot").replaceChildren(createIcon("arrow-up"));
    elements.sendButton.title = running ? "加入队列" : "发送消息";
    elements.sendButton.setAttribute("aria-label", elements.sendButton.title);
    elements.sendButton.disabled = state.blocked || state.adminBusy || state.submitting || hasPendingQuestion() || inputCount === 0 || inputCount > MAX_CONTENT_CHARS;

    if (state.blocked) elements.composerState.textContent = "未授权";
    else if (hasPendingQuestion()) elements.composerState.textContent = "等待回答";
    else if (busy) elements.composerState.textContent = state.submitting ? (running ? "正在加入队列" : "正在发送") : "正在处理";
    else if (inputCount > MAX_CONTENT_CHARS) elements.composerState.textContent = "消息不能超过 20,000 个字符";
    else elements.composerState.textContent = "";
    elements.composerState.classList.toggle("is-error", inputCount > MAX_CONTENT_CHARS);
    updateSettingsControls();
  }

  function isNearBottom() {
    const distance = elements.chatScroll.scrollHeight - elements.chatScroll.scrollTop - elements.chatScroll.clientHeight;
    return distance <= NEAR_BOTTOM_PX;
  }

  function isAtBottom() {
    const distance = elements.chatScroll.scrollHeight - elements.chatScroll.scrollTop - elements.chatScroll.clientHeight;
    return distance <= 2;
  }

  function suspendOutputFollowing() {
    state.followOutput = false;
    state.scrollRequestId += 1;
    elements.jumpBottomButton.hidden = false;
  }

  function scrollToBottom({ force = false, smooth = false } = {}) {
    if (!force && !state.followOutput) {
      elements.jumpBottomButton.hidden = false;
      return;
    }
    if (force) state.followOutput = true;
    const requestId = ++state.scrollRequestId;
    window.requestAnimationFrame(() => {
      if (!force && (!state.followOutput || requestId !== state.scrollRequestId)) return;
      state.programmaticScroll = true;
      elements.chatScroll.scrollTo({ top: elements.chatScroll.scrollHeight, behavior: smooth ? "smooth" : "auto" });
      state.nearBottom = true;
      elements.jumpBottomButton.hidden = true;
      window.setTimeout(() => {
        state.programmaticScroll = false;
      }, smooth ? 300 : 0);
    });
  }

  function contentAdded() {
    if (state.followOutput) scrollToBottom();
    else elements.jumpBottomButton.hidden = false;
  }

  async function copyText(text) {
    const value = String(text || "");
    if (!value) return false;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
        showToast("已复制");
        return true;
      }
    } catch (_) {
      // Use the selection fallback below.
    }
    const textarea = document.createElement("textarea");
    textarea.value = value;
    textarea.setAttribute("readonly", "");
    textarea.style.position = "fixed";
    textarea.style.left = "-9999px";
    textarea.style.top = "0";
    document.body.appendChild(textarea);
    textarea.select();
    textarea.setSelectionRange(0, textarea.value.length);
    let copied = false;
    try {
      copied = document.execCommand("copy");
    } catch (_) {
      copied = false;
    }
    textarea.remove();
    showToast(copied ? "已复制" : "复制失败", copied ? "info" : "error");
    return copied;
  }

  function makeCopyButton(textProvider, label = "复制") {
    const button = document.createElement("button");
    button.type = "button";
    button.title = label;
    button.setAttribute("aria-label", label);
    button.appendChild(makeIconSlot("copy"));
    button.addEventListener("click", () => copyText(typeof textProvider === "function" ? textProvider() : textProvider));
    return button;
  }

  function validHttpUrl(value) {
    const raw = String(value || "").trim();
    if (!/^https?:\/\//i.test(raw)) return null;
    try {
      const url = new URL(raw);
      return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
    } catch (_) {
      return null;
    }
  }

  function appendInline(parent, source, depth = 0) {
    const text = String(source || "");
    if (depth > 8) {
      parent.appendChild(document.createTextNode(text));
      return;
    }
    let index = 0;
    let plainStart = 0;
    const flushPlain = (end) => {
      if (end > plainStart) parent.appendChild(document.createTextNode(text.slice(plainStart, end)));
    };
    while (index < text.length) {
      if (text[index] === "\\" && index + 1 < text.length && "\\`*_[]|~".includes(text[index + 1])) {
        flushPlain(index);
        parent.appendChild(document.createTextNode(text[index + 1]));
        index += 2;
        plainStart = index;
        continue;
      }
      if (text[index] === "\n") {
        flushPlain(index);
        parent.appendChild(document.createElement("br"));
        index += 1;
        plainStart = index;
        continue;
      }
      if (text[index] === "`") {
        const end = text.indexOf("`", index + 1);
        if (end > index + 1) {
          flushPlain(index);
          const code = document.createElement("code");
          code.textContent = text.slice(index + 1, end);
          parent.appendChild(code);
          index = end + 1;
          plainStart = index;
          continue;
        }
      }
      if (text[index] === "[") {
        const labelEnd = text.indexOf("](", index + 1);
        const urlEnd = labelEnd >= 0 ? text.indexOf(")", labelEnd + 2) : -1;
        if (labelEnd > index + 1 && urlEnd > labelEnd + 2) {
          const href = validHttpUrl(text.slice(labelEnd + 2, urlEnd));
          if (href) {
            flushPlain(index);
            const link = document.createElement("a");
            link.href = href;
            link.target = "_blank";
            link.rel = "noopener noreferrer";
            appendInline(link, text.slice(index + 1, labelEnd), depth + 1);
            parent.appendChild(link);
            index = urlEnd + 1;
            plainStart = index;
            continue;
          }
        }
      }
      if (text.startsWith("~~", index)) {
        const end = text.indexOf("~~", index + 2);
        if (end > index + 2 && text.slice(index + 2, end).trim()) {
          flushPlain(index);
          const deletion = document.createElement("del");
          appendInline(deletion, text.slice(index + 2, end), depth + 1);
          parent.appendChild(deletion);
          index = end + 2;
          plainStart = index;
          continue;
        }
      }
      const strongMarker = text.startsWith("**", index) ? "**" : text.startsWith("__", index) ? "__" : null;
      if (strongMarker) {
        const end = text.indexOf(strongMarker, index + 2);
        if (end > index + 2 && text.slice(index + 2, end).trim()) {
          flushPlain(index);
          const strong = document.createElement("strong");
          appendInline(strong, text.slice(index + 2, end), depth + 1);
          parent.appendChild(strong);
          index = end + 2;
          plainStart = index;
          continue;
        }
      }
      if (text[index] === "*" || text[index] === "_") {
        const marker = text[index];
        const end = text.indexOf(marker, index + 1);
        if (end > index + 1 && text.slice(index + 1, end).trim()) {
          flushPlain(index);
          const emphasis = document.createElement("em");
          appendInline(emphasis, text.slice(index + 1, end), depth + 1);
          parent.appendChild(emphasis);
          index = end + 1;
          plainStart = index;
          continue;
        }
      }
      index += 1;
    }
    flushPlain(text.length);
  }

  function codeBlock(language, codeText) {
    const wrapper = document.createElement("div");
    wrapper.className = "code-block";
    const toolbar = document.createElement("div");
    toolbar.className = "code-toolbar";
    const label = document.createElement("span");
    label.textContent = language || "代码";
    const copy = makeCopyButton(codeText, "复制代码");
    copy.className = "code-copy-button";
    toolbar.append(label, copy);
    const pre = document.createElement("pre");
    const code = document.createElement("code");
    if (language) code.className = `language-${language}`;
    code.textContent = codeText;
    pre.appendChild(code);
    wrapper.append(toolbar, pre);
    return wrapper;
  }

  function parseTableRow(line) {
    const text = String(line || "").trim();
    const cells = [];
    let cell = "";
    let codeFenceLength = 0;
    let hasSeparator = false;
    let endedWithSeparator = false;
    for (let index = 0; index < text.length;) {
      if (text[index] === "\\" && index + 1 < text.length) {
        cell += text.slice(index, index + 2);
        index += 2;
        endedWithSeparator = false;
        continue;
      }
      if (text[index] === "`") {
        let end = index + 1;
        while (end < text.length && text[end] === "`") end += 1;
        const runLength = end - index;
        if (!codeFenceLength) codeFenceLength = runLength;
        else if (codeFenceLength === runLength) codeFenceLength = 0;
        cell += text.slice(index, end);
        index = end;
        endedWithSeparator = false;
        continue;
      }
      if (text[index] === "|" && !codeFenceLength) {
        cells.push(cell.trim());
        cell = "";
        hasSeparator = true;
        endedWithSeparator = true;
        index += 1;
        continue;
      }
      cell += text[index];
      endedWithSeparator = false;
      index += 1;
    }
    cells.push(cell.trim());
    if (text.startsWith("|")) cells.shift();
    if (endedWithSeparator) cells.pop();
    return { cells, hasSeparator };
  }

  function tableAlignments(line) {
    const row = parseTableRow(line);
    if (!row.hasSeparator || !row.cells.length) return null;
    const alignments = [];
    for (const cell of row.cells) {
      const marker = cell.match(/^(:)?-{3,}(:)?$/);
      if (!marker) return null;
      alignments.push(marker[1] && marker[2] ? "center" : marker[2] ? "right" : marker[1] ? "left" : "");
    }
    return alignments;
  }

  function isTableStart(lines, index) {
    if (index + 1 >= lines.length) return false;
    const header = parseTableRow(lines[index]);
    const alignments = tableAlignments(lines[index + 1]);
    return Boolean(alignments && header.hasSeparator && header.cells.length === alignments.length);
  }

  function isHorizontalRule(line) {
    const text = String(line || "").trim();
    return /^(?:\*\s*){3,}$/.test(text) || /^(?:-\s*){3,}$/.test(text) || /^(?:_\s*){3,}$/.test(text);
  }

  function markdownTable(lines, startIndex) {
    const headers = parseTableRow(lines[startIndex]).cells;
    const alignments = tableAlignments(lines[startIndex + 1]);
    const wrapper = document.createElement("div");
    wrapper.className = "markdown-table-scroll";
    const table = document.createElement("table");
    const head = document.createElement("thead");
    const headRow = document.createElement("tr");
    headers.forEach((content, column) => {
      const cell = document.createElement("th");
      cell.scope = "col";
      if (alignments[column]) cell.className = `align-${alignments[column]}`;
      appendInline(cell, content);
      headRow.appendChild(cell);
    });
    head.appendChild(headRow);
    table.appendChild(head);

    const body = document.createElement("tbody");
    let index = startIndex + 2;
    while (index < lines.length && lines[index].trim()) {
      const row = parseTableRow(lines[index]);
      if (!row.hasSeparator) break;
      const tableRow = document.createElement("tr");
      for (let column = 0; column < headers.length; column += 1) {
        const cell = document.createElement("td");
        if (alignments[column]) cell.className = `align-${alignments[column]}`;
        appendInline(cell, row.cells[column] || "");
        tableRow.appendChild(cell);
      }
      body.appendChild(tableRow);
      index += 1;
    }
    if (body.children.length) table.appendChild(body);
    wrapper.appendChild(table);
    return { node: wrapper, nextIndex: index };
  }

  function isMarkdownBlockStart(lines, index) {
    const line = lines[index];
    return /^\s*```/.test(line) || /^#{1,6}\s+/.test(line) || /^\s*[-*+]\s+/.test(line) || /^\s*\d+[.)]\s+/.test(line) || /^\s*>/.test(line) || isHorizontalRule(line) || isTableStart(lines, index);
  }

  function renderMarkdown(container, source) {
    const lines = String(source || "").replace(/\r\n?/g, "\n").split("\n");
    const fragment = document.createDocumentFragment();
    let index = 0;
    while (index < lines.length) {
      const line = lines[index];
      if (!line.trim()) {
        index += 1;
        continue;
      }
      const fence = line.match(/^\s*```\s*([\w.+-]*)\s*$/);
      if (fence) {
        const codeLines = [];
        index += 1;
        while (index < lines.length && !/^\s*```\s*$/.test(lines[index])) {
          codeLines.push(lines[index]);
          index += 1;
        }
        if (index < lines.length) index += 1;
        const language = /^[\w.+-]{1,40}$/.test(fence[1] || "") ? fence[1] : "";
        fragment.appendChild(codeBlock(language, codeLines.join("\n")));
        continue;
      }
      if (isTableStart(lines, index)) {
        const rendered = markdownTable(lines, index);
        fragment.appendChild(rendered.node);
        index = rendered.nextIndex;
        continue;
      }
      if (isHorizontalRule(line)) {
        fragment.appendChild(document.createElement("hr"));
        index += 1;
        continue;
      }
      const heading = line.match(/^(#{1,6})\s+(.+)$/);
      if (heading) {
        const level = Math.min(6, heading[1].length + 1);
        const node = document.createElement(`h${level}`);
        appendInline(node, heading[2]);
        fragment.appendChild(node);
        index += 1;
        continue;
      }
      const unordered = line.match(/^\s*[-*+]\s+(.+)$/);
      if (unordered) {
        const list = document.createElement("ul");
        let hasTask = false;
        while (index < lines.length) {
          const itemMatch = lines[index].match(/^\s*[-*+]\s+(.+)$/);
          if (!itemMatch) break;
          const item = document.createElement("li");
          const task = itemMatch[1].match(/^\[([ xX])\]\s+(.*)$/);
          if (task) {
            hasTask = true;
            item.className = "task-list-item";
            const checkbox = document.createElement("input");
            checkbox.type = "checkbox";
            checkbox.checked = task[1].toLowerCase() === "x";
            checkbox.disabled = true;
            const content = document.createElement("span");
            appendInline(content, task[2]);
            item.append(checkbox, content);
          } else {
            appendInline(item, itemMatch[1]);
          }
          list.appendChild(item);
          index += 1;
        }
        if (hasTask) list.classList.add("task-list");
        fragment.appendChild(list);
        continue;
      }
      const ordered = line.match(/^\s*\d+[.)]\s+(.+)$/);
      if (ordered) {
        const list = document.createElement("ol");
        while (index < lines.length) {
          const itemMatch = lines[index].match(/^\s*\d+[.)]\s+(.+)$/);
          if (!itemMatch) break;
          const item = document.createElement("li");
          appendInline(item, itemMatch[1]);
          list.appendChild(item);
          index += 1;
        }
        fragment.appendChild(list);
        continue;
      }
      if (/^\s*>/.test(line)) {
        const quoteLines = [];
        while (index < lines.length) {
          const quote = lines[index].match(/^\s*>\s?(.*)$/);
          if (!quote) break;
          quoteLines.push(quote[1]);
          index += 1;
        }
        const blockquote = document.createElement("blockquote");
        appendInline(blockquote, quoteLines.join("\n"));
        fragment.appendChild(blockquote);
        continue;
      }
      const paragraphLines = [line];
      index += 1;
      while (index < lines.length && lines[index].trim() && !isMarkdownBlockStart(lines, index)) {
        paragraphLines.push(lines[index]);
        index += 1;
      }
      const paragraph = document.createElement("p");
      appendInline(paragraph, paragraphLines.join("\n"));
      fragment.appendChild(paragraph);
    }
    container.replaceChildren(fragment);
  }

  function createDayDivider(timestamp) {
    const divider = document.createElement("div");
    divider.className = "day-divider";
    divider.dataset.dayKey = dayKey(timestamp);
    const label = document.createElement("span");
    label.textContent = formatDayLabel(timestamp);
    divider.appendChild(label);
    return divider;
  }

  function appendDayDividerIfNeeded(timestamp) {
    const dividers = elements.timeline.querySelectorAll(".day-divider");
    const lastDivider = dividers[dividers.length - 1];
    if (!lastDivider || lastDivider.dataset.dayKey !== dayKey(timestamp)) elements.timeline.appendChild(createDayDivider(timestamp));
  }

  function createUserMessage(content, timestamp, attributes = {}) {
    const article = document.createElement("article");
    article.className = "message user-message";
    article.dataset.role = "user";
    if (attributes.turnId) article.dataset.turnId = attributes.turnId;
    if (attributes.runId) article.dataset.runId = attributes.runId;
    if (attributes.followupId) article.dataset.followupId = attributes.followupId;
    const bubble = document.createElement("div");
    bubble.className = "user-bubble";
    const paragraph = document.createElement("p");
    paragraph.textContent = String(content || "");
    bubble.appendChild(paragraph);
    const actions = document.createElement("div");
    actions.className = "message-actions";
    const time = document.createElement("span");
    time.textContent = formatTime(timestamp) || "刚刚";
    time.title = formatDateTime(timestamp);
    actions.append(time, makeCopyButton(String(content || ""), "复制消息"));
    article.append(bubble, actions);
    return article;
  }

  function safeAssetUrl(value) {
    const raw = String(value || "").trim();
    if (!raw) return null;
    try {
      const url = new URL(raw, window.location.origin);
      if (url.origin !== window.location.origin || !url.pathname.startsWith("/api/assets/") || url.pathname === "/api/assets/") return null;
      return url.href;
    } catch (_) {
      return null;
    }
  }

  function validAssetDimension(value) {
    const number = Number(value);
    return Number.isInteger(number) && number > 0 && number <= 100_000 ? number : null;
  }

  function createAssetAction(iconName, label, href, download = false) {
    const link = document.createElement("a");
    link.href = href;
    link.title = label;
    link.setAttribute("aria-label", label);
    link.rel = "noopener noreferrer";
    if (download) link.setAttribute("download", "");
    else link.target = "_blank";
    link.appendChild(makeIconSlot(iconName));
    return link;
  }

  function createConversationMedia(asset, { eager = false } = {}) {
    const source = asset && typeof asset === "object" ? asset : {};
    const url = safeAssetUrl(source.url);
    const mime = String(source.mime || "").trim().toLowerCase();
    const imageMime = !mime || mime.startsWith("image/");
    const width = validAssetDimension(source.width);
    const height = validAssetDimension(source.height);
    const alt = String(source.alt || "").trim() || "Miyu 生成的图片";
    const hideCaption = Boolean(source.hide_caption);

    const figure = document.createElement("figure");
    figure.className = "conversation-media";
    if (source.id != null) figure.dataset.assetId = String(source.id);
    const visual = document.createElement("div");
    visual.className = "conversation-media-visual";
    if (width && height) {
      const ratio = width / height;
      if (ratio >= 0.05 && ratio <= 20) {
        visual.classList.add("has-aspect");
        visual.style.aspectRatio = `${width} / ${height}`;
      }
    }
    const fallback = document.createElement("div");
    fallback.className = "conversation-media-fallback";
    fallback.appendChild(makeIconSlot("circle-alert"));
    const fallbackText = document.createElement("span");
    fallbackText.textContent = url && imageMime ? "图片载入失败" : "图片地址不可用";
    fallback.appendChild(fallbackText);

    if (url && imageMime) {
      const image = document.createElement("img");
      image.alt = alt;
      image.loading = eager ? "eager" : "lazy";
      image.decoding = "async";
      if (width) image.width = width;
      if (height) image.height = height;
      fallback.hidden = true;
      image.addEventListener("error", () => {
        image.remove();
        fallback.hidden = false;
        figure.classList.add("is-error");
        contentAdded();
      }, { once: true });
      image.addEventListener("load", contentAdded, { once: true });
      image.src = url;
      visual.append(image, fallback);
    } else {
      visual.appendChild(fallback);
    }

    const caption = document.createElement("figcaption");
    caption.className = "conversation-media-caption";
    if (!hideCaption) {
      const captionText = document.createElement("span");
      captionText.textContent = alt;
      captionText.title = alt;
      caption.appendChild(captionText);
    } else {
      caption.classList.add("is-actions-only");
    }
    if (url) {
      const actions = document.createElement("span");
      actions.className = "conversation-media-actions";
      actions.append(
        createAssetAction("external-link", "在新窗口打开图片", url),
        createAssetAction("download", "下载图片", url, true)
      );
      caption.appendChild(actions);
    }
    figure.appendChild(visual);
    if (caption.childElementCount) figure.appendChild(caption);
    return figure;
  }

  /*
   * display.reasoning 只决定后端产生什么(摘要/完整/不产生);
   * WebUI 是否渲染仅以「有没有思考内容」为准,hidden 时若仍收到文本则不渲染(保底)。
   * 默认展开/收起由本地偏好 miyu.web.reasoningExpanded 决定,与 summary/full 无关。
   */
  function reasoningHidden() {
    return state.display?.reasoning === "hidden";
  }

  function normalizeReasoningTitle(value) {
    const title = String(value || "").trim().replace(/^[*#\s]+|[*#\s]+$/g, "");
    if (!title || /^正在(?:思考)?(?:\.{3}|…+)?$/u.test(title)) return "";
    return title;
  }

  function splitReasoningText(value) {
    const raw = String(value || "").trim();
    const bold = raw.match(/^\*\*([^\n*]{1,160})\*\*(?:\r?\n){0,2}([\s\S]*)$/);
    if (bold) return { title: normalizeReasoningTitle(bold[1]), body: bold[2].trim() };
    const heading = raw.match(/^#{1,6}\s+([^\n]{1,160})(?:\r?\n)+([\s\S]*)$/);
    if (heading) return { title: normalizeReasoningTitle(heading[1]), body: heading[2].trim() };
    return { title: "", body: raw };
  }

  function createReasoningBlock(text, title = "已思考", live = false, summaryOnly = false) {
    const details = document.createElement("details");
    details.className = "reasoning-block";
    details.classList.toggle("is-summary", summaryOnly);
    details.classList.toggle("is-live", live);
    details.open = state.reasoningExpanded === true;
    const summary = document.createElement("summary");
    const atom = makeIconSlot("atom", "reasoning-icon");
    if (live) for (let index = 0; index < 3; index += 1) atom.appendChild(document.createElement("i"));
    const titleNode = document.createElement("span");
    titleNode.className = "reasoning-title";
    titleNode.textContent = title || (live ? "正在思考" : "已思考");
    const chevron = makeIconSlot("chevron-right", "reasoning-chevron");
    summary.append(atom, titleNode);
    let liveStatus = null;
    let progress = null;
    if (live) {
      liveStatus = document.createElement("span");
      liveStatus.className = "reasoning-live-status";
      liveStatus.textContent = "0s";
      summary.appendChild(liveStatus);
      progress = document.createElement("div");
      progress.className = "reasoning-progress";
      progress.setAttribute("role", "progressbar");
      progress.setAttribute("aria-label", "思考进度");
      progress.setAttribute("aria-valuetext", "正在思考");
      const progressFill = document.createElement("i");
      progressFill.setAttribute("aria-hidden", "true");
      progress.appendChild(progressFill);
    }
    summary.appendChild(chevron);
    const body = document.createElement("div");
    body.className = "reasoning-text";
    body.textContent = String(text || "");
    details.append(summary);
    if (progress) details.appendChild(progress);
    details.appendChild(body);
    const block = {
      element: details,
      title: titleNode,
      liveStatus,
      progress,
      body,
      raw: String(text || ""),
      pendingTitle: "",
      summaryOnly,
      partOpen: false,
      startedAt: live ? performance.now() : null,
      finished: !live,
      userToggled: false,
      ignoreNextToggle: false
    };
    details.addEventListener("toggle", () => {
      if (block.ignoreNextToggle) {
        block.ignoreNextToggle = false;
        return;
      }
      block.userToggled = true;
    });
    return block;
  }

  function createAssistantMessage({
    content = "",
    reasoning = "",
    reasoningTitle = "已思考",
    assets = [],
    timestamp = null,
    tokenTotal = 0,
    tokenEstimated = false,
    providerId = "",
    model = "",
    activeContext = true,
    turnId = null,
    muted = false
  } = {}) {
    const article = document.createElement("article");
    article.className = `message assistant-message${muted ? " is-muted" : ""}`;
    article.dataset.role = "assistant";
    if (turnId) article.dataset.turnId = turnId;
    const header = document.createElement("header");
    header.className = "assistant-label";
    const avatar = document.createElement("img");
    avatar.alt = "";
    avatar.setAttribute("aria-hidden", "true");
    setPersonaAvatar(avatar);
    const identity = document.createElement("div");
    const name = document.createElement("strong");
    name.textContent = state.persona.name;
    const time = document.createElement("span");
    time.textContent = formatTime(timestamp) || "";
    time.title = formatDateTime(timestamp);
    identity.append(name, time);
    header.append(avatar, identity, stop);
    const assistantContent = document.createElement("div");
    assistantContent.className = "assistant-content";
    const blocks = document.createElement("div");
    blocks.className = "assistant-blocks";
    if (String(reasoning || "").trim() && !reasoningHidden()) {
      const parsed = splitReasoningText(reasoning);
      blocks.appendChild(createReasoningBlock(parsed.body, "已思考", false).element);
    }
    if (String(content || "").trim()) {
      const markdown = document.createElement("div");
      markdown.className = "markdown-body";
      renderMarkdown(markdown, content);
      blocks.appendChild(markdown);
    }
    for (const asset of Array.isArray(assets) ? assets : []) blocks.appendChild(createConversationMedia(asset));
    assistantContent.appendChild(blocks);
    assistantContent.classList.toggle("is-slim", !blocks.querySelector(WIDE_BLOCK_SELECTOR));
    article.append(header, assistantContent);

    const meta = document.createElement("div");
    meta.className = "assistant-meta";
    if (state.display?.show_mixed_model_endpoint && (String(providerId || "").trim() || String(model || "").trim())) {
      const endpoint = document.createElement("span");
      endpoint.className = "assistant-endpoint";
      endpoint.textContent = [providerId, model].map((value) => String(value || "").trim()).filter(Boolean).join(" / ");
      meta.appendChild(endpoint);
    }
    if (asFiniteNumber(tokenTotal) > 0) {
      const token = document.createElement("span");
      token.textContent = `${tokenEstimated ? "约 " : ""}${formatTokens(tokenTotal)} tokens`;
      meta.appendChild(token);
    }
    if (!activeContext) {
      const contextBadge = document.createElement("span");
      contextBadge.className = "context-state-badge";
      contextBadge.textContent = "已移出当前上下文";
      meta.appendChild(contextBadge);
    }
    const copyValue = String(content || "").trim() || String(reasoning || "");
    if (copyValue) {
      const spacer = document.createElement("span");
      spacer.className = "meta-spacer";
      meta.append(spacer, makeCopyButton(copyValue, "复制回复"));
    }
    if (meta.childNodes.length) article.appendChild(meta);
    return article;
  }

  function createAnsweredQuestionCard(exchange, compact = true) {
    const card = document.createElement("section");
    card.className = "answered-question-card";
    if (compact) card.classList.add("is-compact");
    const header = document.createElement("header");
    const icon = document.createElement("span");
    icon.className = "question-icon";
    icon.appendChild(makeIconSlot("check"));
    const copy = document.createElement("div");
    const status = document.createElement("small");
    status.textContent = "已回答";
    const title = document.createElement("strong");
    const questions = Array.isArray(exchange?.questions) ? exchange.questions : [];
    title.textContent = questions.length === 1 ? String(questions[0]?.header || "补充确认") : `${questions.length} 项补充确认`;
    copy.append(status, title);
    header.append(icon, copy);
    const list = document.createElement("dl");
    list.className = "answered-question-list";
    const answers = Array.isArray(exchange?.answers) ? exchange.answers : [];
    questions.forEach((question, index) => {
      const row = document.createElement("div");
      const term = document.createElement("dt");
      term.textContent = String(question?.question || question?.header || `问题 ${index + 1}`);
      const description = document.createElement("dd");
      const selected = Array.isArray(answers[index]) ? answers[index] : [];
      description.textContent = selected.map(String).join("、") || "未记录";
      row.append(term, description);
      list.appendChild(row);
    });
    card.append(header, list);
    return card;
  }

  function createPersistedQuestion(exchange, turnId) {
    const wrapper = document.createElement("article");
    wrapper.className = "persisted-question-wrap";
    if (turnId) wrapper.dataset.turnId = turnId;
    wrapper.appendChild(createAnsweredQuestionCard(exchange));
    return wrapper;
  }

  function createTurnStatus(turn) {
    const status = document.createElement("div");
    status.className = "turn-status-line";
    status.dataset.turnStatus = String(turn?.id || "");
    const isInterrupted = turn?.status === "interrupted";
    status.classList.toggle("is-interrupted", isInterrupted);
    status.appendChild(makeIconSlot(isInterrupted ? "circle-alert" : "loader-circle"));
    const text = document.createElement("span");
    text.textContent = isInterrupted ? "本轮已中断" : "本轮正在运行";
    status.appendChild(text);
    if (asFiniteNumber(turn?.token_total) > 0) {
      const usage = document.createElement("span");
      usage.textContent = `${turn.token_usage_estimated ? "约 " : ""}${formatTokens(turn.token_total)} tokens`;
      status.appendChild(usage);
    }
    if (turn?.active_context === false) {
      const context = document.createElement("span");
      context.className = "context-state-badge";
      context.textContent = "已移出当前上下文";
      status.appendChild(context);
    }
    return status;
  }

  function renderPersistedTurn(turn) {
    const turnId = String(turn?.id || "");
    elements.timeline.appendChild(createUserMessage(turn?.user_content || "", turn?.user_timestamp, { turnId }));

    /*
     * 本页会话内完成的 turn:优先复用 live 流式渲染出的 article(含按时序排列的
     * 思考签 / 工具签 / 正文块),避免用扁平的「单 reasoning + 正文」重建而丢失时序。
     * 历史重载(后端快照没有 parts 顺序)才退回扁平重建。
     */
    const stash = turnId && turn?.status !== "running" ? state.finishedTurnArticles.get(turnId) : null;
    let stashIndex = 0;
    const takeStash = (kind) => {
      if (!stash || stashIndex >= stash.length || stash[stashIndex].kind !== kind) return null;
      return stash[stashIndex++].article;
    };

    // 已回答的问题卡在 live article 内部原位保留;仅在无存档时用快照重建。
    if (!stash) {
      const exchanges = Array.isArray(turn?.question_exchanges) ? turn.question_exchanges : [];
      for (const exchange of exchanges) elements.timeline.appendChild(createPersistedQuestion(exchange, turnId));
    }

    const followups = Array.isArray(turn?.followups) ? turn.followups : [];
    for (const followup of followups) {
      const precedingContent = String(followup?.preceding_assistant_content || "");
      const precedingReasoning = String(followup?.preceding_assistant_reasoning || "");
      const stashedSegment = takeStash("segment");
      if (stashedSegment) {
        elements.timeline.appendChild(stashedSegment);
      } else if (precedingContent.trim() || precedingReasoning.trim()) {
        elements.timeline.appendChild(createAssistantMessage({
          content: precedingContent,
          reasoning: precedingReasoning,
          providerId: followup?.provider_id,
          model: followup?.model,
          timestamp: followup?.submitted_at,
          turnId,
          activeContext: turn?.active_context !== false
        }));
      }
      elements.timeline.appendChild(createUserMessage(followup?.content || "", followup?.submitted_at, {
        turnId,
        followupId: String(followup?.id || "")
      }));
    }
    let leftoverSegment;
    while ((leftoverSegment = takeStash("segment"))) elements.timeline.appendChild(leftoverSegment);

    const assistantContent = String(turn?.assistant_content || "");
    const assistantReasoning = String(turn?.assistant_reasoning || "");
    const assets = turn?.status === "running" ? [] : (Array.isArray(turn?.assets) ? turn.assets : []);
    const stashedFinal = takeStash("final");
    if (stashedFinal) {
      stashedFinal.classList.toggle("is-muted", turn?.active_context === false);
      elements.timeline.appendChild(stashedFinal);
    } else if (assistantContent.trim() || assistantReasoning.trim() || assets.length) {
      elements.timeline.appendChild(createAssistantMessage({
        content: assistantContent,
        reasoning: assistantReasoning,
        providerId: turn?.provider_id,
        model: turn?.model,
        assets,
        timestamp: turn?.assistant_timestamp,
        tokenTotal: turn?.token_total,
        tokenEstimated: Boolean(turn?.token_usage_estimated),
        activeContext: turn?.active_context !== false,
        turnId,
        muted: turn?.active_context === false
      }));
    }
    if (turn?.status === "running" || turn?.status === "interrupted") elements.timeline.appendChild(createTurnStatus(turn));
    else if (!stashedFinal && !assistantContent.trim() && !assistantReasoning.trim() && (asFiniteNumber(turn?.token_total) > 0 || turn?.active_context === false)) {
      const metadata = createTurnStatus({ ...turn, status: "completed" });
      metadata.querySelector("span:nth-child(2)").textContent = "本轮已完成";
      metadata.querySelector(".icon-slot").replaceChildren(createIcon("check"));
      elements.timeline.appendChild(metadata);
    }
  }

  function renderConversation() {
    elements.loadingState.hidden = true;
    elements.blockedState.hidden = true;
    clearQuestionDock();
    elements.timeline.replaceChildren();
    const turns = [...state.turns].sort((left, right) => asFiniteNumber(left?.seq) - asFiniteNumber(right?.seq));
    state.turns = turns;
    if (state.finishedTurnArticles.size) {
      const knownTurnIds = new Set(turns.map((turn) => String(turn?.id)));
      for (const key of [...state.finishedTurnArticles.keys()]) {
        if (!knownTurnIds.has(key)) state.finishedTurnArticles.delete(key);
      }
    }
    if (turns.length === 0) {
      elements.timeline.hidden = true;
      elements.emptyState.hidden = false;
    } else {
      elements.emptyState.hidden = true;
      elements.timeline.hidden = false;
      let previousDay = null;
      for (const turn of turns) {
        const currentDay = dayKey(turn?.user_timestamp);
        if (currentDay !== previousDay) {
          elements.timeline.appendChild(createDayDivider(turn?.user_timestamp));
          previousDay = currentDay;
        }
        renderPersistedTurn(turn);
      }
    }
    state.nearBottom = true;
    state.followOutput = true;
    elements.jumpBottomButton.hidden = true;
    updateConversationChrome();
    window.requestAnimationFrame(() => {
      elements.chatScroll.scrollTop = elements.chatScroll.scrollHeight;
    });
  }

  function createLiveState(runId, options = {}) {
    return {
      runId,
      turnId: options.turnId || null,
      userText: options.userText || "",
      startedAt: options.startedAt || new Date(),
      userRendered: Boolean(options.userRendered),
      article: null,
      blocks: null,
      headerStatus: null,
      stopButton: null,
      cancellationRequested: false,
      meta: null,
      endpoint: null,
      copyButton: null,
      currentText: null,
      assistantText: "",
      assistantReasoning: "",
      assets: [],
      reasoning: null,
      reasoningParts: [],
      reasoningStarted: false,
      reasoningTitle: "",
      reasoningTimer: null,
      providerId: "",
      model: "",
      tools: new Map(),
      questions: new Map(),
      contextOperation: null,
      typing: null,
      ended: false
    };
  }

  function renderQueueTray() {
    const prompts = Array.isArray(state.queuedPrompts) ? state.queuedPrompts : [];
    elements.queueTray.replaceChildren();
    elements.queueTray.hidden = prompts.length === 0;
    for (const prompt of prompts) {
      const row = document.createElement("div");
      row.className = "queue-item";
      const text = document.createElement("span");
      text.textContent = String(prompt?.content || "");
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "queue-remove";
      remove.title = "移除排队消息";
      remove.setAttribute("aria-label", "移除排队消息");
      remove.appendChild(makeIconSlot("x"));
      remove.addEventListener("click", () => removeQueuedPrompt(prompt.id));
      row.append(text, remove);
      elements.queueTray.appendChild(row);
    }
    updateControlState();
  }

  async function removeQueuedPrompt(promptId) {
    if (!promptId) return;
    try {
      await apiRequest(`/api/queue/${encodeURIComponent(promptId)}`, { method: "DELETE" });
      state.queuedPrompts = state.queuedPrompts.filter((prompt) => String(prompt?.id) !== String(promptId));
      renderQueueTray();
    } catch (error) {
      showToast(error.message || "排队消息移除失败", "error");
      if (error.status === 404 && state.viewSessionId) await loadSessionView(state.viewSessionId, { quiet: true });
    }
  }

  function disposeLiveState(live) {
    if (!live) return;
    removeLiveStopButton(live);
    if (live.reasoningTimer) {
      window.clearInterval(live.reasoningTimer);
      live.reasoningTimer = null;
    }
    if (live.currentText?.renderFrame) {
      window.cancelAnimationFrame(live.currentText.renderFrame);
      live.currentText.renderFrame = null;
    }
    for (const tool of live.tools?.values?.() || []) {
      if (tool.collapseTimer) window.clearTimeout(tool.collapseTimer);
      tool.collapseTimer = null;
    }
  }

  function ensureTimelineVisible() {
    elements.loadingState.hidden = true;
    elements.blockedState.hidden = true;
    elements.emptyState.hidden = true;
    elements.timeline.hidden = false;
  }

  function ensureLiveUser(live, content) {
    if (!live || live.userRendered) return;
    const text = String(content || live.userText || "");
    if (!text.trim()) return;
    live.userText = text;
    ensureTimelineVisible();
    appendDayDividerIfNeeded(new Date());
    const message = createUserMessage(text, new Date(), { runId: live.runId });
    if (live.article?.isConnected) elements.timeline.insertBefore(message, live.article);
    else elements.timeline.appendChild(message);
    live.userRendered = true;
    updateConversationChrome();
    contentAdded();
  }

  function removeRunningStatus(turnId) {
    if (!turnId) return;
    const status = Array.from(elements.timeline.querySelectorAll("[data-turn-status]"))
      .find((node) => node.dataset.turnStatus === String(turnId));
    status?.remove();
  }

  /* 发送后、第一个内容 part 到达前:气泡内三点弹跳等待动画 */
  function showTypingIndicator(live) {
    if (!live || live.ended || live.typing) return;
    ensureLiveArticle(live);
    if (live.blocks.childElementCount > 0) return;
    const indicator = document.createElement("div");
    indicator.className = "typing-indicator";
    indicator.setAttribute("aria-hidden", "true");
    for (let index = 0; index < 3; index += 1) indicator.appendChild(document.createElement("i"));
    live.blocks.appendChild(indicator);
    live.typing = indicator;
    contentAdded();
  }

  function clearTypingIndicator(live) {
    if (!live?.typing) return;
    live.typing.remove();
    live.typing = null;
  }

  /* 完成态保时序:live 渲染出的 article 按 turn 存档,重渲染时原样复用 */
  function stashLiveArticle(live, kind) {
    if (!live?.article || !live.turnId) return;
    clearTypingIndicator(live);
    if (!live.blocks || live.blocks.childElementCount === 0) return;
    live.article.classList.remove("live-assistant");
    const key = String(live.turnId);
    const list = state.finishedTurnArticles.get(key) || [];
    list.push({ kind, article: live.article });
    state.finishedTurnArticles.set(key, list);
  }

  function updateLiveStopButton(live) {
    if (!live.stopButton) return;
    live.stopButton.disabled = live.ended || live.cancellationRequested;
    live.stopButton.title = live.cancellationRequested ? "正在停止" : "停止本条回复";
    live.stopButton.setAttribute("aria-label", live.stopButton.title);
  }

  function removeLiveStopButton(live) {
    if (!live.stopButton) return;
    live.stopButton.remove();
    live.stopButton = null;
    elements.liveStopRail.hidden = elements.liveStopRail.childElementCount === 0;
  }

  async function cancelLiveRun(live) {
    if (!live || live.ended || live.cancellationRequested) return;
    live.cancellationRequested = true;
    updateLiveStopButton(live);
    if (live.headerStatus) live.headerStatus.textContent = "正在停止";
    try {
      await apiRequest(`/api/runs/${encodeURIComponent(live.runId)}/cancel`, { method: "POST" });
    } catch (error) {
      live.cancellationRequested = false;
      updateLiveStopButton(live);
      if (live.headerStatus && !live.ended) live.headerStatus.textContent = "正在回复";
      showToast(error.message || "停止失败", "error");
      if ((error.status === 404 || error.status === 409) && state.viewSessionId) {
        await loadSessionView(state.viewSessionId, { quiet: true });
      }
    }
  }

  // 气泡宽度:blocks 里出现正文/媒体/上下文操作等「宽内容」前保持贴合内容
  const WIDE_BLOCK_SELECTOR = ".markdown-body, .conversation-media, .context-operation, img, .tool-live-progress:not([hidden])";
  function syncBubbleWidth(article) {
    if (!article) return;
    const content = article.querySelector(".assistant-content");
    if (!content) return;
    content.classList.toggle("is-slim", !content.querySelector(WIDE_BLOCK_SELECTOR));
  }

  function ensureLiveArticle(live) {
    if (live.article) return live.article;
    ensureTimelineVisible();
    ensureLiveUser(live, live.userText);
    removeRunningStatus(live.turnId);
    const article = document.createElement("article");
    article.className = "message assistant-message live-assistant";
    article.dataset.role = "assistant";
    article.dataset.runId = live.runId;
    const header = document.createElement("header");
    header.className = "assistant-label";
    const avatar = document.createElement("img");
    avatar.alt = "";
    avatar.setAttribute("aria-hidden", "true");
    setPersonaAvatar(avatar);
    const identity = document.createElement("div");
    const name = document.createElement("strong");
    name.textContent = state.persona.name;
    const status = document.createElement("span");
    status.className = "live-indicator";
    // 直播状态由三点弹跳/思考签表达,header 不再写「正在回复」;完成后写「刚刚」等
    status.textContent = "";
    identity.append(name, status);
    // Each running reply owns a compact stop control in its bubble corner.
    const stop = document.createElement("button");
    stop.type = "button";
    stop.className = "live-stop-button";
    stop.dataset.runId = live.runId;
    stop.appendChild(makeIconSlot("stop-square"));
    stop.addEventListener("click", () => cancelLiveRun(live));
    header.append(avatar, identity);
    for (const existing of elements.liveStopRail.querySelectorAll(".live-stop-button")) {
      if (existing.dataset.runId === live.runId) existing.remove();
    }
    elements.liveStopRail.appendChild(stop);
    elements.liveStopRail.hidden = false;
    const assistantContent = document.createElement("div");
    assistantContent.className = "assistant-content is-slim";
    const blocks = document.createElement("div");
    blocks.className = "assistant-blocks";
    assistantContent.appendChild(blocks);
    const bubble = document.createElement("div");
    bubble.className = "assistant-bubble";
    bubble.appendChild(assistantContent);
    const meta = document.createElement("div");
    meta.className = "assistant-meta";
    const endpoint = document.createElement("span");
    endpoint.className = "assistant-endpoint";
    endpoint.hidden = true;
    const metaText = document.createElement("span");
    metaText.textContent = "";
    const spacer = document.createElement("span");
    spacer.className = "meta-spacer";
    const copy = makeCopyButton(() => live.assistantText, "复制回复");
    copy.hidden = true;
    meta.append(endpoint, metaText, spacer, copy);
    article.append(header, bubble, meta);
    elements.timeline.appendChild(article);
    live.article = article;
    live.blocks = blocks;
    live.headerStatus = status;
    live.stopButton = stop;
    live.meta = metaText;
    live.endpoint = endpoint;
    live.copyButton = copy;
    updateLiveStopButton(live);
    contentAdded();
    return article;
  }

  function breakLiveText(live) {
    live.currentText = null;
  }

  function scheduleMarkdownRender(block) {
    if (block.renderFrame) return;
    block.renderFrame = window.requestAnimationFrame(() => {
      block.renderFrame = null;
      renderMarkdown(block.element, block.raw);
      contentAdded();
    });
  }

  function appendAssistantDelta(live, delta) {
    const text = String(delta || "");
    if (!text) return;
    ensureLiveArticle(live);
    clearTypingIndicator(live);
    if (!live.currentText) {
      finalizeLiveReasoning(live);
      const element = document.createElement("div");
      element.className = "markdown-body live-text-block";
      const block = { element, raw: "", renderFrame: null };
      live.blocks.appendChild(element);
      syncBubbleWidth(live.article);
      live.currentText = block;
      live.contextOperation = null;
      if (live.assistantText && !/\s$/.test(live.assistantText)) live.assistantText += "\n\n";
    }
    live.currentText.raw += text;
    live.assistantText += text;
    live.copyButton.hidden = !live.assistantText.trim();
    scheduleMarkdownRender(live.currentText);
    contentAdded();
  }

  function ensureLiveReasoning(live) {
    ensureLiveArticle(live);
    clearTypingIndicator(live);
    if (live.reasoning) return live.reasoning;
    breakLiveText(live);
    live.contextOperation = null;
    const reasoning = createReasoningBlock("", "正在思考", true);
    // 计时从 reasoning.start 事件算起,而不是签出现的时刻(签是惰性创建的)
    if (live.reasoningClockStart != null) reasoning.startedAt = live.reasoningClockStart;
    reasoning.pendingTitle = normalizeReasoningTitle(live.reasoningTitle);
    if (!reasoningHidden()) live.blocks.appendChild(reasoning.element);
    live.reasoning = reasoning;
    live.reasoningParts.push(reasoning);
    if (live.reasoningTimer) window.clearInterval(live.reasoningTimer);
    const updateProgress = () => {
      if (!reasoning.liveStatus || reasoning.startedAt == null) return;
      const elapsed = Math.max(0, Math.floor((performance.now() - reasoning.startedAt) / 1000));
      reasoning.liveStatus.textContent = `${elapsed}s`;
    };
    updateProgress();
    live.reasoningTimer = window.setInterval(updateProgress, 1000);
    return reasoning;
  }

  function collectLiveReasoning(live) {
    return (live.reasoningParts || [])
      .map((part) => String(part.raw || "").trim())
      .filter(Boolean)
      .join("\n\n");
  }

  function finalizeLiveReasoning(live) {
    const reasoning = live.reasoning;
    if (!reasoning) return;
    if (live.reasoningTimer) {
      window.clearInterval(live.reasoningTimer);
      live.reasoningTimer = null;
    }
    const parsed = splitReasoningText(reasoning.raw);
    const title = "已思考";
    reasoning.raw = parsed.body;
    reasoning.finished = true;
    if (!reasoning.raw.trim() && title === "已思考") {
      reasoning.element.remove();
    } else {
      reasoning.element.classList.remove("is-live");
      reasoning.title.textContent = title;
      reasoning.body.textContent = reasoning.raw;
      if (reasoning.progress) reasoning.progress.remove();
      if (reasoning.liveStatus) {
        if (reasoning.startedAt != null) {
          reasoning.liveStatus.textContent = `${((performance.now() - reasoning.startedAt) / 1000).toFixed(1)}s`;
        } else {
          reasoning.liveStatus.remove();
        }
      }
    }
    live.reasoning = null;
    live.reasoningTitle = "";
    live.reasoningStarted = false;
    live.reasoningClockStart = null;
    live.assistantReasoning = collectLiveReasoning(live);
  }

  function handleReasoningEvent(name, live, data) {
    if (name === "reasoning.start" || name === "reasoning.part_start") {
      // 惰性创建:只记状态,签等第一段真实思考文本(reasoning.delta)到达才出现,
      // 避免不输出思考的模型挂着空的「正在思考」签和空面板
      finalizeLiveReasoning(live);
      live.reasoningStarted = true;
      live.reasoningClockStart = performance.now();
      breakLiveText(live);
      return;
    }
    if (name === "reasoning.reset") {
      if (live.reasoning) {
        live.reasoning.raw = "";
        live.reasoning.body.textContent = "";
        live.reasoning.pendingTitle = "";
      }
      return;
    }
    if (name === "reasoning.title") {
      live.reasoningTitle = String(data?.title || "").trim();
      // 只更新已存在的签;没有思考文本就不为标题单独建签
      if (live.reasoning) live.reasoning.pendingTitle = normalizeReasoningTitle(live.reasoningTitle);
      return;
    }
    if (name === "reasoning.delta") {
      const delta = String(data?.delta || "");
      if (!delta) return;
      if (!live.reasoning && !delta.trim()) return;
      const reasoning = ensureLiveReasoning(live);
      reasoning.raw += delta;
      reasoning.body.textContent = reasoning.raw;
      live.assistantReasoning = collectLiveReasoning(live);
      contentAdded();
      return;
    }
    if (name === "reasoning.part_end") {
      finalizeLiveReasoning(live);
    }
  }

  function prettyArguments(value) {
    if (value == null) return "";
    if (typeof value === "string") {
      const trimmed = value.trim();
      if (!trimmed) return "";
      try {
        return JSON.stringify(JSON.parse(trimmed), null, 2);
      } catch (_) {
        return value;
      }
    }
    try {
      return JSON.stringify(value, null, 2);
    } catch (_) {
      return String(value);
    }
  }

  function parsedToolArguments(value) {
    if (value && typeof value === "object" && !Array.isArray(value)) return value;
    if (typeof value !== "string" || !value.trim()) return {};
    try {
      const parsed = JSON.parse(value);
      return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {};
    } catch (_) {
      return {};
    }
  }

  function compactLine(value, limit = 92) {
    const line = String(value || "").replace(/\s+/g, " ").trim();
    if (line.length <= limit) return line;
    return `${line.slice(0, Math.max(1, limit - 1))}…`;
  }

  function compactPath(value) {
    const path = String(value || "").trim();
    if (!path) return "";
    return path.split(/[\\/]/).filter(Boolean).pop() || path;
  }

  function toolSubject(name, value) {
    const args = parsedToolArguments(value);
    const toolName = String(name || "");
    if (toolName === "run_command") return compactLine(args.command || args.cmd);
    if (["read", "write", "edit", "apply_patch", "print_image", "vision_analyze"].includes(toolName)) {
      return compactPath(args.filePath || args.file_path || args.path || args.image);
    }
    if (toolName === "grep") {
      const target = compactPath(args.path);
      return compactLine(`${args.pattern || ""}${target ? ` · ${target}` : ""}`);
    }
    if (toolName === "glob") return compactLine(`${args.pattern || ""}${args.path ? ` · ${compactPath(args.path)}` : ""}`);
    if (["webfetch", "web_fetch"].includes(toolName)) return compactLine(args.url);
    if (["web_search", "search_web", "search_web_images"].includes(toolName)) return compactLine(args.query || args.q);
    if (toolName === "generate_image") return compactLine(args.prompt);
    if (toolName === "task") return compactLine(args.description || args.prompt);
    if (toolName === "load_skill") return compactLine(args.name);
    const preferred = ["query", "command", "path", "filePath", "url", "name", "id", "target"];
    for (const key of preferred) {
      if (typeof args[key] === "string" && args[key].trim()) return compactLine(args[key]);
    }
    return "";
  }

  function formatToolDuration(milliseconds) {
    if (!Number.isFinite(milliseconds) || milliseconds < 0) return "";
    if (milliseconds < 1_000) return `${Math.max(1, Math.round(milliseconds))} ms`;
    if (milliseconds < 10_000) return `${(milliseconds / 1_000).toFixed(1)} s`;
    return `${Math.round(milliseconds / 1_000)} s`;
  }

  // 主题与工具显示名共享 ≥6 字符前缀时去重(如「Linux 游戏兼容性调查」+「Linux 游戏兼容性: xxx」)
  function dedupeToolSubject(title, subject) {
    const t = String(title || "").trim();
    const s = String(subject || "").trim();
    if (!t || !s) return s;
    let i = 0;
    while (i < t.length && i < s.length && t[i] === s[i]) i += 1;
    if (i < 6) return s;
    const rest = s.slice(i).replace(/^[\s:：·,，、-]+/, "");
    return rest || s;
  }

  function updateToolSummary(tool) {
    const details = [];
    const subject = dedupeToolSubject(tool.titleText, tool.subject);
    if (subject) details.push(subject);
    if (tool.imageCount) details.push(`${tool.imageCount} 张图片`);
    if (tool.finishedAt != null) details.push(formatToolDuration(tool.finishedAt - tool.startedAt));
    tool.summary.textContent = details.filter(Boolean).join(" · ") || (tool.finished ? "无输出" : "等待输出");
  }

  function scrollToolOutputToEnd(tool) {
    for (const detail of [tool.stdoutDetail, tool.stderrDetail, tool.resultDetail]) {
      if (!detail.wrapper.hidden) detail.content.scrollTop = detail.content.scrollHeight;
    }
  }

  function boundedAppend(current, addition) {
    const combined = `${current || ""}${addition || ""}`;
    if (combined.length <= MAX_TOOL_OUTPUT_CHARS) return combined;
    return `[较早输出已省略]\n${combined.slice(combined.length - MAX_TOOL_OUTPUT_CHARS)}`;
  }

  function createToolDetail(labelText, preformatted = false) {
    const wrapper = document.createElement("div");
    wrapper.className = "tool-detail";
    wrapper.hidden = true;
    const label = document.createElement("span");
    label.className = "tool-detail-label";
    label.textContent = labelText;
    const content = document.createElement(preformatted ? "pre" : "p");
    wrapper.append(label, content);
    return { wrapper, content, raw: "" };
  }

  function updateToolStatus(tool, status, iconName, statusClass = "") {
    tool.statusText.textContent = status;
    tool.statusIcon.replaceChildren(createIcon(iconName));
    tool.statusIcon.classList.toggle("is-spinning", iconName === "loader-circle");
    tool.card.classList.remove("is-success", "is-failure");
    if (statusClass) tool.card.classList.add(statusClass);
  }

  function createTool(live, data) {
    ensureLiveArticle(live);
    clearTypingIndicator(live);
    breakLiveText(live);
    finalizeLiveReasoning(live);
    live.contextOperation = null;
    const toolId = String(data?.tool_id || `${live.runId}_tool_unknown_${live.tools.size + 1}`);
    if (live.tools.has(toolId)) return live.tools.get(toolId);
    const card = document.createElement("section");
    card.className = state.toolExpanded ? "tool-card" : "tool-card collapsed";
    card.dataset.toolId = toolId;
    const isCommand = String(data?.name || "") === "run_command";
    if (isCommand) card.classList.add("is-command");
    const isTask = String(data?.name || "") === "task" || /^task[:：]/i.test(String(data?.display_name || ""));
    if (isTask) card.classList.add("is-task");
    const subjectText = toolSubject(data?.name, data?.arguments);
    const head = document.createElement("button");
    head.className = "tool-head";
    head.type = "button";
    head.setAttribute("aria-expanded", String(Boolean(state.toolExpanded)));
    const icon = document.createElement("span");
    icon.className = "tool-icon";
    icon.appendChild(makeIconSlot(isCommand ? "fileTerminal" : "wrench"));
    const title = document.createElement("span");
    title.className = "tool-title";
    const displayName = document.createElement("strong");
    displayName.textContent = String(data?.display_name || data?.name || "工具");
    const realName = document.createElement("small");
    realName.className = "tool-technical-name";
    realName.textContent = String(data?.name || "");
    const summary = document.createElement("small");
    summary.className = "tool-summary";
    title.append(displayName, realName, summary);
    const status = document.createElement("span");
    status.className = "tool-status";
    const statusIcon = makeIconSlot("loader-circle", "is-spinning");
    const statusText = document.createElement("span");
    statusText.textContent = "运行中";
    status.append(statusIcon, statusText);
    const chevron = makeIconSlot("chevron-down", "tool-chevron");
    head.append(icon, title, status, chevron);
    const body = document.createElement("div");
    body.className = "tool-body";
    const argumentsDetail = createToolDetail("参数", true);
    const progressDetail = createToolDetail("进度");
    const stdoutDetail = createToolDetail("命令输出", true);
    const stderrDetail = createToolDetail("错误输出", true);
    stderrDetail.wrapper.classList.add("is-stderr");
    const resultDetail = createToolDetail("结果", true);
    const argumentText = prettyArguments(data?.arguments);
    if (argumentText) {
      argumentsDetail.raw = argumentText;
      argumentsDetail.content.textContent = argumentText;
      argumentsDetail.wrapper.hidden = false;
    }
    body.append(argumentsDetail.wrapper, progressDetail.wrapper, stdoutDetail.wrapper, stderrDetail.wrapper, resultDetail.wrapper);
    // 子代理签:标题行下方的实时进度面板,收起态也可见,tool.progress 原地刷新
    let liveProgress = null;
    if (isTask) {
      liveProgress = document.createElement("div");
      liveProgress.className = "tool-live-progress";
      liveProgress.textContent = subjectText || "正在启动子代理…";
      card.append(head, liveProgress, body);
    } else {
      card.append(head, body);
    }
    const tool = {
      id: toolId,
      name: String(data?.name || ""),
      card,
      head,
      body,
      status,
      statusIcon,
      statusText,
      summary,
      argumentsDetail,
      progressDetail,
      stdoutDetail,
      stderrDetail,
      resultDetail,
      isTask,
      liveProgress,
      titleText: String(data?.display_name || data?.name || "工具"),
      subject: subjectText,
      startedAt: performance.now(),
      finishedAt: null,
      imageCount: 0,
      finished: false,
      collapseTimer: null
    };
    head.addEventListener("click", () => {
      const collapsed = card.classList.toggle("collapsed");
      head.setAttribute("aria-expanded", String(!collapsed));
      if (!collapsed) {
        window.requestAnimationFrame(() => {
          scrollToolOutputToEnd(tool);
          contentAdded();
        });
      }
    });
    updateToolSummary(tool);
    live.tools.set(toolId, tool);
    live.blocks.appendChild(card);
    contentAdded();
    return tool;
  }

  function ensureTool(live, data) {
    const toolId = String(data?.tool_id || "");
    return (toolId && live.tools.get(toolId)) || createTool(live, data);
  }

  function handleToolEvent(name, live, data) {
    if (name === "tool.started") {
      createTool(live, data);
      return;
    }
    const tool = ensureTool(live, data);
    if (name === "tool.image") {
      const asset = data?.asset && typeof data.asset === "object" ? data.asset : null;
      if (asset && safeAssetUrl(asset.url)) {
        const assetId = String(asset.id || asset.url);
        if (!live.assets.some((item) => String(item?.id || item?.url) === assetId)) {
          ensureLiveArticle(live);
          clearTypingIndicator(live);
          breakLiveText(live);
          finalizeLiveReasoning(live);
          live.contextOperation = null;
          live.assets.push(asset);
          live.blocks.appendChild(createConversationMedia(asset, { eager: true }));
          syncBubbleWidth(live.article);
          tool.imageCount += 1;
        }
      } else if (data?.error) {
        const message = String(data.error);
        tool.progressDetail.raw = message;
        tool.progressDetail.content.textContent = message;
        tool.progressDetail.wrapper.hidden = Boolean(tool.liveProgress);
        if (tool.liveProgress) {
          tool.liveProgress.textContent = message;
          tool.liveProgress.hidden = false;
        }
      }
      updateToolSummary(tool);
    } else if (name === "tool.progress") {
      const message = String(data?.message || "");
      // 任何持续汇报进度的工具(插件子代理如深度研究/兼容性调查)都惰性获得实时进度面板,
      // 不再仅限内置 task 工具
      if (!tool.liveProgress && !tool.finished && message) {
        tool.liveProgress = document.createElement("div");
        tool.liveProgress.className = "tool-live-progress";
        tool.card.insertBefore(tool.liveProgress, tool.body);
      }
      tool.progressDetail.raw = message;
      tool.progressDetail.content.textContent = message;
      tool.progressDetail.wrapper.hidden = !message || Boolean(tool.liveProgress);
      if (tool.liveProgress && message) {
        tool.liveProgress.textContent = message;
        tool.liveProgress.hidden = false;
        syncBubbleWidth(live.article);
      }
      if (!tool.subject && message) tool.subject = compactLine(message);
      updateToolStatus(tool, "运行中", "loader-circle");
      updateToolSummary(tool);
    } else if (name === "tool.output") {
      const detail = data?.stream === "stderr" ? tool.stderrDetail : tool.stdoutDetail;
      detail.raw = boundedAppend(detail.raw, String(data?.output || ""));
      detail.content.textContent = detail.raw;
      detail.wrapper.hidden = !detail.raw;
      if (!tool.card.classList.contains("collapsed")) detail.content.scrollTop = detail.content.scrollHeight;
      updateToolSummary(tool);
    } else if (name === "tool.finished") {
      tool.finished = true;
      tool.finishedAt = performance.now();
      const output = String(data?.output || "");
      tool.resultDetail.raw = output.length > MAX_TOOL_OUTPUT_CHARS ? `[较早输出已省略]\n${output.slice(-MAX_TOOL_OUTPUT_CHARS)}` : output;
      tool.resultDetail.content.textContent = tool.resultDetail.raw;
      tool.resultDetail.wrapper.hidden = !tool.resultDetail.raw;
      const ok = Boolean(data?.ok);
      updateToolStatus(tool, ok ? "完成" : "失败", ok ? "check" : "circle-alert", ok ? "is-success" : "is-failure");
      updateToolSummary(tool);
      if (tool.liveProgress) {
        if (ok) tool.liveProgress.hidden = true;
        else tool.liveProgress.classList.add("is-error");
        tool.progressDetail.wrapper.hidden = !tool.progressDetail.raw;
        syncBubbleWidth(live.article);
      }
      if (!state.toolExpanded) {
        tool.card.classList.add("collapsed");
        tool.head.setAttribute("aria-expanded", "false");
      }
    }
    contentAdded();
  }

  function updateQuestionOptionClasses(questionState) {
    for (const control of questionState.controls) {
      for (const option of control.options) option.label.classList.toggle("selected", option.input.checked);
      if (control.custom) control.custom.wrapper.classList.toggle("selected", control.custom.toggle.checked);
    }
    questionState.pageTabs?.forEach((tab, index) => {
      const control = questionState.controls[index];
      const answered = control.options.some((option) => option.input.checked)
        || Boolean(control.custom?.toggle.checked && control.custom.textarea.value.trim());
      tab.classList.toggle("is-complete", answered);
    });
  }

  function updateQuestionDock() {
    elements.questionDock.hidden = elements.questionDock.childElementCount === 0;
    window.requestAnimationFrame(updateJumpButtonOffset);
  }

  function clearQuestionDock() {
    elements.questionDock.replaceChildren();
    updateQuestionDock();
  }

  function moveQuestionToTimeline(questionState) {
    if (questionState.card.parentElement !== elements.questionDock) return;
    if (questionState.timelineParent?.isConnected) questionState.timelineParent.appendChild(questionState.card);
    else questionState.card.remove();
    updateQuestionDock();
  }

  function removeQuestionFromDock(questionState) {
    if (questionState.card.parentElement === elements.questionDock) questionState.card.remove();
    updateQuestionDock();
  }

  function setQuestionPage(questionState, index, { focus = false } = {}) {
    if (!questionState?.pages?.length) return;
    const lastIndex = questionState.pages.length - 1;
    const nextIndex = Math.max(0, Math.min(lastIndex, Number(index) || 0));
    questionState.pageIndex = nextIndex;
    questionState.pages.forEach((page, pageIndex) => {
      page.hidden = pageIndex !== nextIndex;
    });
    questionState.pageTabs.forEach((tab, pageIndex) => {
      const active = pageIndex === nextIndex;
      tab.classList.toggle("active", active);
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    });
    const multiple = Boolean(questionState.questions[nextIndex]?.multiple);
    questionState.pageLabel.textContent = `第 ${nextIndex + 1} / ${questionState.pages.length} 项`;
    questionState.hint.textContent = multiple ? "可多选" : "请选择一项";
    questionState.previous.hidden = nextIndex === 0;
    questionState.next.hidden = nextIndex === lastIndex;
    questionState.submit.hidden = nextIndex !== lastIndex;
    elements.questionDock.scrollTop = 0;
    window.requestAnimationFrame(() => {
      updateJumpButtonOffset();
      if (focus) questionState.pages[nextIndex].querySelector("input:not(:disabled), textarea:not(:disabled)")?.focus();
    });
  }

  function selectedQuestionAnswers(questionState) {
    const answers = [];
    for (let index = 0; index < questionState.controls.length; index += 1) {
      const control = questionState.controls[index];
      const selected = control.options.filter((option) => option.input.checked).map((option) => option.value);
      if (control.custom?.toggle.checked) {
        const custom = control.custom.textarea.value.trim();
        if (!custom) throw new Error(`请填写第 ${index + 1} 项的自定义回答`);
        if (countCharacters(custom) > MAX_CUSTOM_ANSWER_CHARS) throw new Error(`第 ${index + 1} 项的自定义回答不能超过 4,000 个字符`);
        if (/[\u0000-\u001f\u007f-\u009f]/.test(custom)) throw new Error(`第 ${index + 1} 项的自定义回答不能包含控制字符或换行`);
        if (selected.includes(custom)) throw new Error(`第 ${index + 1} 项包含重复回答`);
        selected.push(custom);
      }
      if (selected.length === 0) throw new Error(`请回答第 ${index + 1} 项`);
      if (!control.multiple && selected.length !== 1) throw new Error(`第 ${index + 1} 项只能选择一个回答`);
      answers.push(selected);
    }
    return answers;
  }

  function setQuestionControlsDisabled(questionState, disabled) {
    questionState.form.querySelectorAll("input, textarea, button").forEach((control) => {
      control.disabled = disabled;
    });
  }

  function renderQuestionAnswerSummary(questionState, answers) {
    questionState.summary.replaceChildren();
    const normalized = Array.isArray(answers) ? answers : [];
    questionState.questions.forEach((question, index) => {
      const row = document.createElement("div");
      const term = document.createElement("dt");
      term.textContent = String(question?.question || question?.header || `问题 ${index + 1}`);
      const value = document.createElement("dd");
      value.textContent = (Array.isArray(normalized[index]) ? normalized[index] : []).map(String).join("、") || "未记录";
      row.append(term, value);
      questionState.summary.appendChild(row);
    });
    questionState.summary.hidden = false;
  }

  function markQuestionAnswered(questionState, answers) {
    if (!questionState || !questionState.pending) return;
    questionState.pending = false;
    questionState.submitting = false;
    questionState.answers = answers;
    questionState.card.classList.remove("is-error");
    questionState.card.classList.add("is-answered");
    questionState.status.textContent = "已回答";
    questionState.icon.replaceChildren(makeIconSlot("check"));
    questionState.error.hidden = true;
    setQuestionControlsDisabled(questionState, true);
    renderQuestionAnswerSummary(questionState, answers);
    moveQuestionToTimeline(questionState);
    updateControlState();
    contentAdded();
  }

  async function submitQuestion(questionState) {
    if (!questionState.pending || questionState.submitting) return;
    let answers;
    try {
      answers = selectedQuestionAnswers(questionState);
    } catch (error) {
      const page = String(error.message || "").match(/第 (\d+) 项/);
      if (page) setQuestionPage(questionState, Number(page[1]) - 1);
      questionState.error.textContent = error.message;
      questionState.error.hidden = false;
      questionState.card.classList.add("is-error");
      return;
    }
    questionState.submitting = true;
    questionState.error.hidden = true;
    questionState.card.classList.remove("is-error");
    questionState.submit.textContent = "提交中";
    setQuestionControlsDisabled(questionState, true);
    try {
      await apiRequest(`/api/questions/${encodeURIComponent(questionState.id)}/answer`, {
        method: "POST",
        body: JSON.stringify({ answers })
      });
      if (questionState.pending) markQuestionAnswered(questionState, answers);
    } catch (error) {
      if (!questionState.pending) return;
      questionState.submitting = false;
      questionState.error.textContent = error.message || "回答提交失败";
      questionState.error.hidden = false;
      questionState.card.classList.add("is-error");
      questionState.submit.textContent = "提交回答";
      setQuestionControlsDisabled(questionState, false);
      showToast(error.message || "回答提交失败", "error");
      if ((error.status === 404 || error.status === 409) && state.viewSessionId) {
        window.setTimeout(() => loadSessionView(state.viewSessionId, { quiet: true }), 300);
      }
    }
  }

  function createQuestion(live, data) {
    clearTypingIndicator(live);
    const questionId = String(data?.question_id || "");
    if (!questionId) return null;
    if (live.questions.has(questionId)) return live.questions.get(questionId);
    ensureLiveArticle(live);
    breakLiveText(live);
    finalizeLiveReasoning(live);
    live.contextOperation = null;
    const questions = Array.isArray(data?.questions) ? data.questions : [];
    const card = document.createElement("section");
    card.className = "question-card";
    card.dataset.questionId = questionId;
    const titleId = `live-question-title-${live.questions.size + 1}`;
    card.setAttribute("aria-labelledby", titleId);
    const header = document.createElement("header");
    const icon = document.createElement("span");
    icon.className = "question-icon";
    icon.appendChild(makeIconSlot("circle-help"));
    const headerCopy = document.createElement("div");
    const status = document.createElement("small");
    status.textContent = "等待回答";
    const title = document.createElement("strong");
    title.id = titleId;
    title.textContent = questions.length === 1 ? String(questions[0]?.header || "补充确认") : `${questions.length} 项补充确认`;
    headerCopy.append(status, title);
    header.append(icon, headerCopy);
    const form = document.createElement("form");
    form.className = "question-form";
    const pagination = document.createElement("div");
    pagination.className = "question-pagination";
    const pageLabel = document.createElement("span");
    pageLabel.className = "question-page-label";
    const pageTabsWrap = document.createElement("div");
    pageTabsWrap.className = "question-page-tabs";
    pageTabsWrap.setAttribute("role", "tablist");
    pageTabsWrap.setAttribute("aria-label", "问题页");
    pagination.append(pageLabel, pageTabsWrap);
    form.appendChild(pagination);
    const controls = [];
    const pages = [];
    const pageTabs = [];
    questions.forEach((question, questionIndex) => {
      const fieldset = document.createElement("fieldset");
      fieldset.className = "question-fieldset";
      fieldset.id = `question-${questionId}-page-${questionIndex + 1}`;
      fieldset.setAttribute("role", "tabpanel");
      fieldset.hidden = questionIndex !== 0;
      const pageTab = document.createElement("button");
      pageTab.type = "button";
      pageTab.className = "question-page-tab";
      pageTab.id = `question-${questionId}-tab-${questionIndex + 1}`;
      pageTab.textContent = String(questionIndex + 1);
      pageTab.title = String(question?.header || `问题 ${questionIndex + 1}`);
      pageTab.setAttribute("role", "tab");
      pageTab.setAttribute("aria-controls", fieldset.id);
      pageTab.setAttribute("aria-selected", String(questionIndex === 0));
      fieldset.setAttribute("aria-labelledby", pageTab.id);
      pageTabsWrap.appendChild(pageTab);
      pageTabs.push(pageTab);
      const legend = document.createElement("legend");
      const headerLabel = document.createElement("span");
      headerLabel.className = "question-header-label";
      headerLabel.textContent = String(question?.header || `问题 ${questionIndex + 1}`);
      legend.append(headerLabel, document.createTextNode(String(question?.question || "")));
      fieldset.appendChild(legend);
      const optionList = document.createElement("div");
      optionList.className = "question-options";
      const multiple = Boolean(question?.multiple);
      const inputType = multiple ? "checkbox" : "radio";
      const inputName = `question-${questionId}-${questionIndex}`;
      const options = [];
      for (const option of Array.isArray(question?.options) ? question.options : []) {
        const label = document.createElement("label");
        label.className = "question-option";
        const input = document.createElement("input");
        input.type = inputType;
        input.name = inputName;
        input.value = String(option?.label || "");
        const optionCopy = document.createElement("span");
        optionCopy.className = "question-option-copy";
        const optionLabel = document.createElement("strong");
        optionLabel.textContent = String(option?.label || "");
        optionCopy.appendChild(optionLabel);
        if (String(option?.description || "")) {
          const description = document.createElement("small");
          description.textContent = String(option.description);
          optionCopy.appendChild(description);
        }
        label.append(input, optionCopy);
        optionList.appendChild(label);
        options.push({ input, label, value: String(option?.label || "") });
      }
      fieldset.appendChild(optionList);
      let custom = null;
      if (question?.custom !== false) {
        const wrapper = document.createElement("label");
        wrapper.className = "custom-answer";
        const toggle = document.createElement("input");
        toggle.type = inputType;
        toggle.name = inputName;
        toggle.value = "__custom__";
        const textarea = document.createElement("textarea");
        textarea.rows = 1;
        textarea.placeholder = "自定义回答";
        textarea.setAttribute("aria-label", `${question?.header || `问题 ${questionIndex + 1}`}的自定义回答`);
        textarea.addEventListener("focus", () => {
          toggle.checked = true;
          updateQuestionOptionClasses(questionState);
        });
        textarea.addEventListener("input", () => {
          if (textarea.value) toggle.checked = true;
          updateQuestionOptionClasses(questionState);
        });
        wrapper.append(toggle, textarea);
        fieldset.appendChild(wrapper);
        custom = { wrapper, toggle, textarea };
      }
      form.appendChild(fieldset);
      pages.push(fieldset);
      controls.push({ multiple, options, custom });
    });
    pagination.hidden = questions.length <= 1;
    const error = document.createElement("p");
    error.className = "question-error";
    error.hidden = true;
    const actions = document.createElement("footer");
    actions.className = "question-actions";
    const hint = document.createElement("span");
    const pageActions = document.createElement("div");
    pageActions.className = "question-page-actions";
    const previous = document.createElement("button");
    previous.type = "button";
    previous.className = "question-page-button is-previous";
    previous.title = "上一题";
    previous.setAttribute("aria-label", "上一题");
    previous.appendChild(makeIconSlot("chevron-right"));
    const next = document.createElement("button");
    next.type = "button";
    next.className = "question-page-button";
    next.title = "下一题";
    next.setAttribute("aria-label", "下一题");
    next.appendChild(makeIconSlot("chevron-right"));
    const submit = document.createElement("button");
    submit.className = "question-submit";
    submit.type = "submit";
    submit.textContent = "提交回答";
    pageActions.append(previous, next, submit);
    actions.append(hint, pageActions);
    form.append(error, actions);
    const summary = document.createElement("dl");
    summary.className = "question-answer-summary";
    summary.hidden = true;
    card.append(header, form, summary);
    const questionState = {
      id: questionId,
      runId: live.runId,
      questions,
      card,
      form,
      controls,
      pages,
      pageTabs,
      pageIndex: 0,
      pageLabel,
      hint,
      previous,
      next,
      icon,
      status,
      submit,
      error,
      summary,
      timelineParent: live.blocks,
      pending: true,
      submitting: false,
      answers: null
    };
    form.querySelectorAll("input").forEach((input) => input.addEventListener("change", () => updateQuestionOptionClasses(questionState)));
    pageTabs.forEach((tab, index) => tab.addEventListener("click", () => setQuestionPage(questionState, index, { focus: true })));
    pageTabsWrap.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      let index = questionState.pageIndex;
      if (event.key === "ArrowLeft") index -= 1;
      else if (event.key === "ArrowRight") index += 1;
      else index = event.key === "Home" ? 0 : pageTabs.length - 1;
      setQuestionPage(questionState, index);
      pageTabs[questionState.pageIndex]?.focus();
    });
    previous.addEventListener("click", () => setQuestionPage(questionState, questionState.pageIndex - 1, { focus: true }));
    next.addEventListener("click", () => setQuestionPage(questionState, questionState.pageIndex + 1, { focus: true }));
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      submitQuestion(questionState);
    });
    live.questions.set(questionId, questionState);
    elements.questionDock.appendChild(card);
    updateQuestionDock();
    setQuestionPage(questionState, 0);
    updateQuestionOptionClasses(questionState);
    updateControlState();
    contentAdded();
    return questionState;
  }

  function endPendingQuestions(live, message) {
    for (const question of live.questions.values()) {
      if (!question.pending) continue;
      question.pending = false;
      question.submitting = false;
      question.card.classList.add("is-error");
      question.status.textContent = "本轮已结束";
      question.error.textContent = message;
      question.error.hidden = false;
      setQuestionControlsDisabled(question, true);
      removeQuestionFromDock(question);
    }
  }

  function createContextOperation(live, kind) {
    ensureLiveArticle(live);
    clearTypingIndicator(live);
    breakLiveText(live);
    finalizeLiveReasoning(live);
    const block = document.createElement("section");
    block.className = "context-operation";
    const title = document.createElement("strong");
    title.append(makeIconSlot("refresh-cw"), document.createElement("span"));
    title.lastChild.textContent = kind === "compact" ? "正在整理上下文" : "正在释放旧上下文";
    const output = document.createElement("pre");
    output.hidden = true;
    block.append(title, output);
    const operation = { kind, block, title: title.lastChild, output, raw: "" };
    live.blocks.appendChild(block);
    syncBubbleWidth(live.article);
    live.contextOperation = operation;
    contentAdded();
    return operation;
  }

  function handleContextEvent(name, live, data) {
    if (name === "context.compact_start") createContextOperation(live, "compact");
    else if (name === "context.compact_delta") {
      const operation = live.contextOperation?.kind === "compact" ? live.contextOperation : createContextOperation(live, "compact");
      operation.raw = boundedAppend(operation.raw, String(data?.delta || ""));
      operation.output.textContent = operation.raw;
      operation.output.hidden = !operation.raw;
    } else if (name === "context.compact_end") {
      if (live.contextOperation?.kind === "compact") live.contextOperation.title.textContent = "上下文已整理";
      live.contextOperation = null;
    } else if (name === "context.pop_start") createContextOperation(live, "pop");
    else if (name === "context.pop_end") {
      if (live.contextOperation?.kind === "pop") live.contextOperation.title.textContent = "旧上下文已释放";
      live.contextOperation = null;
    } else if (name === "context.error") {
      const operation = live.contextOperation || createContextOperation(live, "compact");
      operation.block.classList.add("is-error");
      operation.title.textContent = "上下文整理未完成";
      operation.raw = String(data?.message || "上下文维护失败");
      operation.output.textContent = operation.raw;
      operation.output.hidden = false;
      live.contextOperation = null;
    }
    contentAdded();
  }

  function appendRunNotice(live, message, error = false) {
    ensureLiveArticle(live);
    clearTypingIndicator(live);
    breakLiveText(live);
    const notice = document.createElement("div");
    notice.className = `run-notice${error ? " is-error" : ""}`;
    notice.append(makeIconSlot(error ? "circle-alert" : "circle-stop"));
    const text = document.createElement("span");
    text.textContent = String(message || "");
    notice.appendChild(text);
    live.blocks.appendChild(notice);
  }

  function markUnfinishedTools(live) {
    for (const tool of live.tools.values()) {
      if (tool.finished) continue;
      tool.finished = true;
      tool.finishedAt = performance.now();
      updateToolStatus(tool, "已中断", "circle-alert", "is-failure");
      updateToolSummary(tool);
      if (tool.liveProgress) {
        if (tool.liveProgress.textContent.trim()) tool.liveProgress.classList.add("is-error");
        else tool.liveProgress.hidden = true;
        tool.progressDetail.wrapper.hidden = !tool.progressDetail.raw;
        syncBubbleWidth(live.article);
      }
      if (!state.toolExpanded) {
        tool.card.classList.add("collapsed");
        tool.head.setAttribute("aria-expanded", "false");
      }
    }
  }

  function setLiveEndpoint(live, providerId, model) {
    const values = [providerId, model].map((value) => String(value || "").trim()).filter(Boolean);
    live.providerId = String(providerId || "");
    live.model = String(model || "");
    if (!live.endpoint) return;
    live.endpoint.textContent = values.join(" / ");
    live.endpoint.hidden = !state.display?.show_mixed_model_endpoint || values.length === 0;
  }

  function consumeLiveQueue(live, data) {
    finalizeLiveReasoning(live);
    setLiveEndpoint(live, data?.provider_id, data?.model);
    if (live.headerStatus) live.headerStatus.textContent = "刚刚";
    if (live.meta) live.meta.textContent = "已完成";

    const ids = new Set((Array.isArray(data?.prompt_ids) ? data.prompt_ids : []).map(String));
    const consumed = state.queuedPrompts.filter((prompt) => ids.has(String(prompt?.id)));
    state.queuedPrompts = state.queuedPrompts.filter((prompt) => !ids.has(String(prompt?.id)));
    for (const prompt of consumed) {
      elements.timeline.appendChild(createUserMessage(prompt?.content || "", prompt?.submitted_at || new Date(), {
        turnId: live.turnId,
        runId: live.runId,
        followupId: prompt?.id
      }));
    }
    renderQueueTray();

    stashLiveArticle(live, "segment");
    removeLiveStopButton(live);
    live.article = null;
    live.blocks = null;
    live.headerStatus = null;
    live.meta = null;
    live.endpoint = null;
    live.copyButton = null;
    live.currentText = null;
    live.assistantText = "";
    live.assistantReasoning = "";
    live.reasoning = null;
    live.reasoningParts = [];
    live.reasoningStarted = false;
    live.reasoningTitle = "";
    live.tools = new Map();
    live.questions = new Map();
    live.contextOperation = null;
    if (["normal", "plan", "chat"].includes(data?.mode)) setMode(data.mode, false);
    showTypingIndicator(live);
    contentAdded();
  }

  function updateLocalTurnFromLive(live, terminalStatus, data) {
    const status = terminalStatus === "completed" ? "completed" : "interrupted";
    let turn = live.turnId ? state.turns.find((item) => String(item?.id) === String(live.turnId)) : null;
    if (!turn && live.userText) {
      turn = {
        id: live.turnId || `local-${live.runId}`,
        seq: state.turns.length ? Math.max(...state.turns.map((item) => asFiniteNumber(item?.seq))) + 1 : 1,
        status,
        active_context: true,
        user_content: live.userText,
        assistant_content: live.assistantText,
        assistant_reasoning: live.assistantReasoning || null,
        provider_id: data?.provider_id || live.providerId || null,
        model: data?.model || live.model || null,
        user_timestamp: new Date().toISOString(),
        assistant_timestamp: new Date().toISOString(),
        token_total: effectiveUsageTotal(data?.usage),
        token_usage_estimated: Boolean(data?.usage_estimated),
        question_exchanges: [],
        followups: [],
        assets: [...live.assets]
      };
      state.turns.push(turn);
    } else if (turn) {
      turn.status = status;
      if (live.assistantText.trim()) turn.assistant_content = live.assistantText;
      if (live.assistantReasoning.trim()) turn.assistant_reasoning = live.assistantReasoning;
      if (data?.provider_id || live.providerId) turn.provider_id = data?.provider_id || live.providerId;
      if (data?.model || live.model) turn.model = data?.model || live.model;
      if (live.assets.length) turn.assets = [...live.assets];
      turn.assistant_timestamp = new Date().toISOString();
      if (terminalStatus === "completed") {
        turn.token_total = effectiveUsageTotal(data?.usage);
        turn.token_usage_estimated = Boolean(data?.usage_estimated);
      }
    }
  }

  function finishLiveRun(kind, data, live) {
    if (!live || live.ended) return;
    const runId = live.runId;
    live.ended = true;
    clearTypingIndicator(live);
    finalizeLiveReasoning(live);
    setLiveEndpoint(live, data?.provider_id, data?.model);
    removeLiveStopButton(live);
    state.terminalRunIds.add(runId);
    if (state.terminalRunIds.size > 30) state.terminalRunIds.delete(state.terminalRunIds.values().next().value);

    if (kind === "completed") {
      if (live.headerStatus) live.headerStatus.textContent = "刚刚";
      if (live.meta) {
        const total = effectiveUsageTotal(data?.usage);
        live.meta.textContent = total > 0 ? `${data?.usage_estimated ? "约 " : ""}${formatTokens(total)} tokens` : "已完成";
      }
    } else if (kind === "cancelled") {
      markUnfinishedTools(live);
      endPendingQuestions(live, "本轮已停止，无法再提交回答");
      // 停止状态只由时间线的「本轮已中断」一处表达,气泡内通知与 header/meta 不再重复
      if (live.headerStatus) live.headerStatus.textContent = "";
      if (live.meta) live.meta.textContent = "";
    } else {
      markUnfinishedTools(live);
      endPendingQuestions(live, "本轮已结束，无法再提交回答");
      appendRunNotice(live, String(data?.message || "本轮运行失败"), true);
      if (live.headerStatus) live.headerStatus.textContent = "运行失败";
      if (live.meta) live.meta.textContent = "";
    }

    updateLocalTurnFromLive(live, kind, data);
    stashLiveArticle(live, "final");
    if (kind === "completed") {
      // 上下文条展示全局（默认会话）上下文；其他会话的 run 不覆盖它。
      const updatesGlobalContext = !data?.session_id || String(data.session_id) === String(state.currentSessionId || "");
      if (updatesGlobalContext) {
        if (data?.context_tokens != null) state.context.tokens = Math.max(0, asFiniteNumber(data.context_tokens));
        state.context.window = data?.context_window == null ? state.context.window : Math.max(0, asFiniteNumber(data.context_window));
      }
      const usage = data?.usage && typeof data.usage === "object" ? data.usage : null;
      if (usage) {
        state.usage.last_usage = usage;
        state.usage.last_conversation_usage = usage;
        state.usage.requests = asFiniteNumber(state.usage.requests) + 1;
        state.usage.prompt_tokens = asFiniteNumber(state.usage.prompt_tokens) + asFiniteNumber(usage.prompt_tokens);
        state.usage.completion_tokens = asFiniteNumber(state.usage.completion_tokens) + asFiniteNumber(usage.completion_tokens);
        state.usage.total_tokens = asFiniteNumber(state.usage.total_tokens) + effectiveUsageTotal(usage);
      }
    }
    state.liveRuns.delete(runId);
    state.replayRunIds?.delete(runId);
    state.pendingSubmission = null;
    updateContext();
    updateRuntimeUsage(data?.usage || null, Boolean(data?.usage_estimated));
    updateConversationChrome();
    updateControlState();
    contentAdded();
    if (state.liveRuns.size === 0) {
      window.requestAnimationFrame(() => {
        if (!state.blocked && !elements.settingsDrawer.classList.contains("open")) focusComposerIfDesktop();
      });
      window.setTimeout(() => {
        if (state.liveRuns.size === 0) refreshViewSnapshot();
      }, 120);
    }
  }

  function clearViewSyncTimer() {
    if (!state.viewSyncTimer) return;
    window.clearTimeout(state.viewSyncTimer);
    state.viewSyncTimer = null;
  }

  function scheduleViewSync() {
    clearViewSyncTimer();
    if (!state.viewRunningTurnId || state.blocked) return;
    state.viewSyncTimer = window.setTimeout(() => {
      state.viewSyncTimer = null;
      refreshViewSnapshot();
    }, 1_000);
  }

  async function refreshViewSnapshot() {
    const sessionId = state.viewSessionId;
    if (!sessionId || state.blocked || state.viewLoading || state.resyncing) {
      scheduleViewSync();
      return;
    }
    try {
      const response = await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}/turns`);
      const payload = await response.json();
      if (state.viewSessionId !== sessionId || state.viewLoading) return;
      const runs = (Array.isArray(payload?.runs) ? payload.runs : []).filter((run) => run?.run_id);
      if (runs.length) state.runsBySession.set(sessionId, new Set(runs.map((run) => String(run.run_id))));
      else if (state.liveRuns.size === 0) state.runsBySession.delete(sessionId);
      state.viewRunningTurnId = !runs.length && typeof payload?.running_turn_id === "string" && payload.running_turn_id
        ? payload.running_turn_id
        : null;
      if (state.liveRuns.size === 0) {
        const nextTurns = Array.isArray(payload?.turns)
          ? payload.turns.sort((a, b) => asFiniteNumber(a?.seq) - asFiniteNumber(b?.seq))
          : state.turns;
        const turnsChanged = JSON.stringify(nextTurns) !== JSON.stringify(state.turns);
        state.turns = nextTurns;
        state.queuedPrompts = Array.isArray(payload?.queued_prompts) ? payload.queued_prompts : state.queuedPrompts;
        if (turnsChanged) renderConversation();
        renderQueueTray();
        restoreLiveRuns(runs);
      }
      renderSessionList();
      updateConversationChrome();
      updateControlState();
    } catch (error) {
      if (error.status === 401) {
        showBlockedState(true);
        return;
      }
      if (error.status === 404) {
        state.viewRunningTurnId = null;
        refreshSessions();
        return;
      }
    } finally {
      scheduleViewSync();
    }
  }

  async function ensureActiveTurnUser(live, turnId) {
    if (!live || live.userRendered || !turnId) return;
    const existing = state.turns.find((turn) => String(turn?.id) === String(turnId));
    if (existing) {
      live.userText = String(existing.user_content || "");
      live.userRendered = true;
      updateConversationChrome();
      return;
    }
    const sessionId = state.viewSessionId;
    try {
      const response = await apiRequest(`/api/sessions/${encodeURIComponent(sessionId)}/turns`);
      const payload = await response.json();
      if (state.viewSessionId !== sessionId || state.liveRuns.get(live.runId) !== live || live.userRendered) return;
      const turn = Array.isArray(payload?.turns) ? payload.turns.find((item) => String(item?.id) === String(turnId)) : null;
      if (!turn) return;
      live.userText = String(turn.user_content || "");
      ensureLiveUser(live, live.userText);
    } catch (_) {
      // The stream can continue; a later view refresh will recover the user turn.
    }
  }

  function handleRunEvent(name, data) {
    const runId = String(data?.run_id || "");
    if (!runId) return;
    const sessionId = typeof data?.session_id === "string" && data.session_id ? data.session_id : runSessionId(runId);
    const terminal = name === "run.completed" || name === "run.cancelled" || name === "run.failed";
    if (name === "run.started" && sessionId) trackRun(sessionId, runId);

    let live = state.liveRuns.get(runId);
    if (!live && !terminal && !state.terminalRunIds.has(runId) && sessionId && sessionId === state.viewSessionId) {
      // 视图会话里出现的新 run（本端发起、他端发起或重放）都会挂上 live 块。
      // run.started 意味着全新的 turn，不去认领时间线里已有的 running turn。
      live = createLiveForRun(runId, "", { claimTurn: name !== "run.started" });
      if (live.turnId && state.viewRunningTurnId === String(live.turnId)) state.viewRunningTurnId = null;
    }

    if (name === "run.started") {
      if (live && ["normal", "plan", "chat"].includes(data?.mode)) setMode(data.mode, false);
      if (live && !live.ended) showTypingIndicator(live);
      renderSessionList();
      updateConversationChrome();
      updateControlState();
      return;
    }
    if (terminal) {
      untrackRun(runId);
      if (live) {
        finishLiveRun(name.slice("run.".length), data, live);
      } else {
        state.terminalRunIds.add(runId);
        if (state.terminalRunIds.size > 30) state.terminalRunIds.delete(state.terminalRunIds.values().next().value);
        if (name === "run.completed" && data?.session_id && String(data.session_id) === String(state.currentSessionId || "")) {
          if (data?.context_tokens != null) state.context.tokens = Math.max(0, asFiniteNumber(data.context_tokens));
          state.context.window = data?.context_window == null ? state.context.window : Math.max(0, asFiniteNumber(data.context_window));
          updateContext();
        }
        renderSessionList();
      }
      return;
    }
    if (!live) return;

    if (name === "turn.started") {
      live.turnId = String(data?.turn_id || "");
      if (state.viewRunningTurnId === live.turnId) state.viewRunningTurnId = null;
      removeRunningStatus(live.turnId);
      ensureActiveTurnUser(live, live.turnId);
    } else if (name === "assistant.delta") appendAssistantDelta(live, data?.delta);
    else if (name.startsWith("reasoning.")) handleReasoningEvent(name, live, data);
    else if (name === "queue.consumed") consumeLiveQueue(live, data);
    else if (name.startsWith("tool.")) handleToolEvent(name, live, data);
    else if (name === "question.requested") createQuestion(live, data);
    else if (name === "question.answered") {
      const question = live.questions.get(String(data?.question_id || ""));
      if (question) markQuestionAnswered(question, data?.answers);
    } else if (name.startsWith("context.")) handleContextEvent(name, live, data);
  }

  function eventShouldBeHandled(name, data, eventId) {
    if (name === "resync_required") {
      if (eventId > 0) state.lastEventId = eventId;
      return true;
    }
    if (eventId > 0 && eventId <= state.lastEventId) return false;
    if (eventId > 0) state.lastEventId = eventId;
    if (state.replayRunIds && eventId > 0 && eventId <= state.replayCutoff) {
      // 重放窗口内只重建正在恢复的 run，其余事件已经反映在快照里。
      if (!RUN_EVENTS.has(name)) return false;
      return state.replayRunIds.has(String(data?.run_id || ""));
    }
    if (state.replayRunIds && eventId > state.replayCutoff) state.replayRunIds = null;
    return true;
  }

  function handleSseEvent(name, event) {
    let data;
    try {
      data = event.data ? JSON.parse(event.data) : {};
    } catch (_) {
      showToast("收到无法解析的事件，正在重新同步", "error");
      loadBootstrap();
      return;
    }
    const eventId = Math.max(0, asFiniteNumber(event.lastEventId));
    if (!eventShouldBeHandled(name, data, eventId)) return;
    if (name === "resync_required") {
      if (!state.resyncing) {
        state.resyncing = true;
        loadBootstrap().finally(() => {
          state.resyncing = false;
        });
      }
      return;
    }
    if (name.startsWith("session.")) {
      handleSessionEvent(name, data);
      return;
    }
    if (name === "queue.added") {
      const prompt = data?.prompt;
      if (queueEventTargetsView(data) && prompt && !state.queuedPrompts.some((item) => String(item?.id) === String(prompt?.id))) {
        state.queuedPrompts.push(prompt);
        renderQueueTray();
      }
      return;
    }
    if (name === "queue.removed") {
      if (queueEventTargetsView(data)) {
        state.queuedPrompts = state.queuedPrompts.filter((prompt) => String(prompt?.id) !== String(data?.prompt_id));
        renderQueueTray();
      }
      return;
    }
    if (name === "conversation.reset" || name === "conversation.pop") {
      // 这两个事件作用于全局默认会话；仅当视图正停在默认会话时才需要重载。
      if (!state.viewSessionId || state.viewSessionId === state.currentSessionId) loadBootstrap();
      else refreshSessions();
      return;
    }
    handleRunEvent(name, data);
  }

  function queueEventTargetsView(data) {
    const explicit = typeof data?.session_id === "string" && data.session_id ? data.session_id : "";
    if (explicit) return explicit === state.viewSessionId;
    const runId = String(data?.run_id || "");
    if (runId) {
      if (state.liveRuns.has(runId)) return true;
      const sessionId = runSessionId(runId);
      if (sessionId) return sessionId === state.viewSessionId;
    }
    const turnId = String(data?.turn_id || "");
    if (turnId) {
      if (state.viewRunningTurnId && turnId === state.viewRunningTurnId) return true;
      for (const live of state.liveRuns.values()) {
        if (String(live.turnId || "") === turnId) return true;
      }
      return state.turns.some((turn) => String(turn?.id) === turnId && turn?.status === "running");
    }
    return false;
  }

  function closeEventSource() {
    if (state.eventSource) {
      state.eventSource.close();
      state.eventSource = null;
    }
    if (state.healthTimer) {
      window.clearTimeout(state.healthTimer);
      state.healthTimer = null;
    }
  }

  async function refineConnectionHealth(source) {
    if (state.eventSource !== source || source.readyState === EventSource.OPEN) return;
    try {
      const response = await fetch("/api/health", { cache: "no-store", credentials: "same-origin" });
      if (!response.ok) throw new Error("health check failed");
      if (state.eventSource === source && source.readyState !== EventSource.OPEN) setConnectionStatus("connecting");
    } catch (_) {
      if (state.eventSource === source && source.readyState !== EventSource.OPEN) setConnectionStatus("offline");
    }
  }

  function connectEventSource(after) {
    closeEventSource();
    if (state.blocked) return;
    const source = new EventSource(`/api/events?after=${encodeURIComponent(Math.max(0, asFiniteNumber(after)))}`);
    state.eventSource = source;
    source.onopen = () => {
      if (state.eventSource !== source) return;
      setConnectionStatus("online");
      if (state.healthTimer) window.clearTimeout(state.healthTimer);
      state.healthTimer = null;
    };
    source.onerror = () => {
      if (state.eventSource !== source) return;
      setConnectionStatus("connecting");
      if (state.healthTimer) window.clearTimeout(state.healthTimer);
      state.healthTimer = window.setTimeout(() => refineConnectionHealth(source), 1200);
    };
    for (const name of EVENT_NAMES) source.addEventListener(name, (event) => handleSseEvent(name, event));
  }

  function showBlockedState(unauthorized, message = "") {
    state.blocked = true;
    state.viewRunningTurnId = null;
    clearViewSyncTimer();
    disposeAllLiveRuns();
    clearQuestionDock();
    closeEventSource();
    elements.loadingState.hidden = true;
    elements.timeline.hidden = true;
    elements.emptyState.hidden = true;
    elements.blockedState.hidden = false;
    elements.blockedTitle.textContent = unauthorized ? "登录 Miyu" : "无法载入 Miyu WebUI";
    elements.blockedMessage.textContent = unauthorized ? "输入访问密码以继续。" : message || "本地服务暂时无法访问";
    elements.loginForm.hidden = !unauthorized;
    elements.retryBootstrapButton.hidden = unauthorized;
    elements.loginError.textContent = "";
    elements.loginError.hidden = true;
    setLoginSubmitting(false);
    setConnectionStatus(unauthorized ? "blocked" : "offline");
    updateControlState();
    if (unauthorized) window.requestAnimationFrame(() => elements.loginPassword.focus());
  }

  function applyBootstrap(snapshot) {
    state.blocked = false;
    clearViewSyncTimer();
    disposeAllLiveRuns();
    state.bootId = String(snapshot?.boot_id || "");
    state.latestEventId = Math.max(0, asFiniteNumber(snapshot?.latest_event_id));
    state.models = Array.isArray(snapshot?.models) ? snapshot.models : [];
    applyPersona(snapshot?.persona);
    state.display = snapshot?.display && typeof snapshot.display === "object" ? snapshot.display : state.display;
    state.context = snapshot?.context && typeof snapshot.context === "object" ? snapshot.context : { tokens: 0, window: null };
    state.usage = snapshot?.usage && typeof snapshot.usage === "object" ? snapshot.usage : {};
    state.capabilities = snapshot?.capabilities && typeof snapshot.capabilities === "object" ? snapshot.capabilities : {};
    state.sessions = Array.isArray(snapshot?.sessions) ? snapshot.sessions.filter((session) => !session?.archived) : [];
    state.currentSessionId = typeof snapshot?.current_session_id === "string" && snapshot.current_session_id ? snapshot.current_session_id : null;
    state.sessionMenuFor = null;
    state.sessionRenaming = null;
    if (state.archivedOpen) loadArchivedSessions();
    state.version = snapshot?.version ?? null;
    state.pendingSubmission = null;
    const allRuns = (Array.isArray(snapshot?.runs) ? snapshot.runs : []).filter((run) => run?.run_id && run?.session_id);
    state.runsBySession = new Map();
    for (const run of allRuns) trackRun(String(run.session_id), String(run.run_id));
    elements.loginForm.hidden = true;
    elements.retryBootstrapButton.hidden = false;
    elements.loginPassword.value = "";
    elements.loginError.textContent = "";
    elements.loginError.hidden = true;
    setLoginSubmitting(false);
    elements.versionLabel.textContent = state.version ? `v${state.version}` : "--";
    clearInlineError();
    renderModelMenu();
    updateCapabilities();
    updateContext();
    state.replayRunIds = null;
    state.replayCutoff = 0;
    const keepView = state.viewSessionId && state.viewSessionId !== state.currentSessionId && findSession(state.viewSessionId);
    if (keepView) {
      // 视图停留在非默认会话：全局重载不改变浏览位置，改用会话接口回填。
      state.lastEventId = state.latestEventId;
      connectEventSource(state.latestEventId);
      loadSessionView(state.viewSessionId, { quiet: true });
    } else if (state.currentSessionId) {
      applySessionView({
        session_id: state.currentSessionId,
        turns: snapshot?.turns,
        queued_prompts: snapshot?.queued_prompts,
        running_turn_id: snapshot?.running_turn_id,
        runs: allRuns.filter((run) => String(run.session_id) === String(state.currentSessionId))
      });
      if (state.liveRuns.size === 0) {
        state.lastEventId = state.latestEventId;
        connectEventSource(state.latestEventId);
      }
    } else {
      // 单会话兜底：没有会话指针时直接使用 bootstrap 快照。
      state.viewSessionId = null;
      state.viewRunningTurnId = typeof snapshot?.running_turn_id === "string" && snapshot.running_turn_id ? snapshot.running_turn_id : null;
      state.turns = Array.isArray(snapshot?.turns) ? snapshot.turns.sort((a, b) => asFiniteNumber(a?.seq) - asFiniteNumber(b?.seq)) : [];
      state.queuedPrompts = Array.isArray(snapshot?.queued_prompts) ? snapshot.queued_prompts : [];
      renderConversation();
      renderQueueTray();
      state.lastEventId = state.latestEventId;
      connectEventSource(state.latestEventId);
    }
    setConnectionStatus("connecting");
    updateRuntimeUsage();
    updateConversationChrome();
    updateControlState();
  }

  async function loadBootstrap() {
    if (state.bootstrapPromise) return state.bootstrapPromise;
    state.bootstrapPromise = (async () => {
      clearViewSyncTimer();
      closeEventSource();
      state.adminBusy = false;
      state.submitting = false;
      if (!state.turns.length && state.liveRuns.size === 0) {
        elements.loadingState.hidden = false;
        elements.blockedState.hidden = true;
        elements.emptyState.hidden = true;
        elements.timeline.hidden = true;
      }
      setConnectionStatus("connecting");
      updateControlState();
      try {
        const response = await apiRequest("/api/bootstrap");
        const snapshot = await response.json();
        applyBootstrap(snapshot);
      } catch (error) {
        showBlockedState(error.status === 401, error.message);
      }
    })();
    try {
      await state.bootstrapPromise;
    } finally {
      state.bootstrapPromise = null;
    }
  }

  function setLoginSubmitting(submitting) {
    state.loginSubmitting = Boolean(submitting);
    elements.loginPassword.disabled = state.loginSubmitting;
    elements.loginSubmit.disabled = state.loginSubmitting;
    elements.loginSubmit.classList.toggle("is-loading", state.loginSubmitting);
    elements.loginSubmitLabel.textContent = state.loginSubmitting ? "正在登录" : "登录";
    const icon = elements.loginSubmit.querySelector(".icon-slot");
    if (icon) icon.replaceChildren(createIcon(state.loginSubmitting ? "loader-circle" : "log-in"));
  }

  async function submitLogin() {
    if (state.loginSubmitting) return;
    const password = elements.loginPassword.value;
    if (!password) {
      elements.loginError.textContent = "请输入访问密码";
      elements.loginError.hidden = false;
      elements.loginPassword.focus();
      return;
    }
    elements.loginError.textContent = "";
    elements.loginError.hidden = true;
    setLoginSubmitting(true);
    try {
      await apiRequest("/api/auth/login", {
        method: "POST",
        body: JSON.stringify({ password })
      });
      elements.loginPassword.value = "";
      await loadBootstrap();
    } catch (error) {
      elements.loginError.textContent = error.status === 401 ? "密码不正确，请重试" : error.message || "登录失败";
      elements.loginError.hidden = false;
      window.requestAnimationFrame(() => {
        elements.loginPassword.focus();
        elements.loginPassword.select();
      });
    } finally {
      setLoginSubmitting(false);
    }
  }

  async function confirmModelSelection() {
    if (!(state.stagedModelKeys instanceof Set) || conversationRunning() || state.adminBusy || state.submitting) return;
    const selected = state.models.filter((model) => state.stagedModelKeys.has(modelKey(model)));
    if (selected.length === 0) {
      state.modelMenuError = "至少选择一个模型";
      updateModelMenuState();
      return;
    }
    state.modelSelectionSubmitting = true;
    state.adminBusy = true;
    state.modelMenuError = "";
    clearInlineError();
    updateControlState();
    let applied = false;
    try {
      const response = await apiRequest("/api/models/active", {
        method: "PUT",
        body: JSON.stringify({
          models: selected.map((model) => ({
            provider_id: String(model.provider_id || ""),
            model: String(model.model || "")
          }))
        })
      });
      const payload = await response.json();
      state.models = Array.isArray(payload?.models) ? payload.models : state.models;
      if (payload?.display && typeof payload.display === "object") state.display = payload.display;
      state.context = payload?.context && typeof payload.context === "object" ? payload.context : state.context;
      applied = true;
    } catch (error) {
      state.modelMenuError = error.message || "模型设置未保存";
      showInlineError(error.message);
      showToast(error.message, "error");
    } finally {
      state.adminBusy = false;
      state.modelSelectionSubmitting = false;
      if (applied) {
        closeModelMenu();
        renderModelMenu();
        updateContext();
        showToast("模型设置已更新");
      }
      updateControlState();
      if (applied) window.requestAnimationFrame(() => elements.modelButton.focus());
      else {
        updateModelMenuState();
        window.requestAnimationFrame(() => elements.modelMenu.querySelector(".model-confirm")?.focus());
      }
    }
  }

  async function submitTurn() {
    if (state.adminBusy || state.submitting || state.blocked) return;
    if (hasPendingQuestion()) return;
    const sessionId = state.viewSessionId;
    const queueing = conversationRunning();
    const content = elements.composerInput.value.trim();
    const count = countCharacters(content);
    if (!content) {
      elements.composerState.textContent = "消息不能为空";
      elements.composerState.classList.add("is-error");
      return;
    }
    if (count > MAX_CONTENT_CHARS) {
      elements.composerState.textContent = "消息不能超过 20,000 个字符";
      elements.composerState.classList.add("is-error");
      return;
    }
    state.submitting = true;
    if (!queueing) state.pendingSubmission = { content, mode: state.mode };
    clearInlineError();
    updateControlState();
    try {
      const body = queueing ? { content } : { content, mode: state.mode };
      if (sessionId) body.session_id = sessionId;
      const response = await apiRequest(queueing ? "/api/queue" : "/api/turns", {
        method: "POST",
        body: JSON.stringify(body)
      });
      const payload = await response.json();
      const queuedPrompt = queueing ? payload : payload?.queued ? payload.prompt : null;
      if (queuedPrompt) {
        if (!state.queuedPrompts.some((prompt) => String(prompt?.id) === String(queuedPrompt?.id))) {
          state.queuedPrompts.push(queuedPrompt);
        }
        state.pendingSubmission = null;
        elements.composerInput.value = "";
        resizeComposer();
        renderQueueTray();
        if (!queueing) {
          // 服务端发现该会话已有 turn 在运行并自动转排队：同步该 run 的 live 状态。
          const runningRunId = String(payload?.run_id || "");
          if (runningRunId && sessionId) {
            trackRun(sessionId, runningRunId);
            if (!state.liveRuns.has(runningRunId) && !state.terminalRunIds.has(runningRunId)) {
              createLiveForRun(runningRunId);
              beginRunReplay();
            }
          } else {
            state.viewRunningTurnId = String(payload?.running_turn_id || "") || state.viewRunningTurnId;
            scheduleViewSync();
          }
          renderSessionList();
          updateConversationChrome();
        }
        return;
      }
      const runId = String(payload?.run_id || "");
      if (!runId) throw new ApiError("服务未返回运行标识", response.status);
      if (state.terminalRunIds.has(runId)) {
        if (sessionId) await loadSessionView(sessionId, { quiet: true });
        else await loadBootstrap();
      } else {
        if (sessionId) trackRun(sessionId, runId);
        const live = createLiveForRun(runId, content);
        live.userText = content;
        ensureLiveUser(live, content);
        showTypingIndicator(live);
        elements.composerInput.value = "";
        resizeComposer();
        updateRuntimeUsage();
        updateConversationChrome();
        renderSessionList();
      }
    } catch (error) {
      if (!queueing) state.pendingSubmission = null;
      showInlineError(error.status === 409
        ? "回复状态刚刚发生变化，正在同步"
        : error.message);
      showToast(error.status === 409 ? "回复状态已同步，请重新发送" : error.message, "error");
      if (error.status === 409) {
        if (sessionId) await loadSessionView(sessionId, { quiet: true });
        else await loadBootstrap();
      }
    } finally {
      state.submitting = false;
      updateControlState();
    }
  }

  function hasHistory() {
    for (const live of state.liveRuns.values()) {
      if (live.userRendered) return true;
    }
    return state.turns.length > 0 || Boolean(elements.timeline.querySelector(".user-message"));
  }

  function openResetDialog() {
    if (typeof elements.resetDialog.showModal === "function") elements.resetDialog.showModal();
    else elements.resetDialog.setAttribute("open", "");
    window.requestAnimationFrame(() => elements.resetCancelButton.focus());
  }

  function requestNewConversation() {
    closeSidebar();
    if (multiSessionEnabled()) {
      createSession();
      return;
    }
    if (!hasHistory()) {
      focusComposerIfDesktop();
      return;
    }
    if (conversationRunning() || state.adminBusy || state.submitting) return;
    openResetDialog();
  }

  function requestClearConversation() {
    if (conversationRunning() || state.adminBusy || state.submitting) return;
    if (!hasHistory()) {
      showToast("当前会话没有可清除的记录");
      return;
    }
    openResetDialog();
  }

  async function resetConversation() {
    if (conversationRunning() || state.adminBusy || state.submitting) return;
    state.adminBusy = true;
    elements.resetConfirmButton.disabled = true;
    elements.resetCancelButton.disabled = true;
    elements.resetConfirmButton.textContent = "正在清除";
    updateControlState();
    try {
      await apiRequest("/api/conversation/reset", { method: "POST" });
      if (elements.resetDialog.open) elements.resetDialog.close("confirmed");
      await loadBootstrap();
      focusComposerIfDesktop();
    } catch (error) {
      showInlineError(error.message);
      showToast(error.message, "error");
      if (error.status === 409) await loadBootstrap();
    } finally {
      state.adminBusy = false;
      elements.resetConfirmButton.disabled = false;
      elements.resetCancelButton.disabled = false;
      elements.resetConfirmButton.textContent = "清空记录";
      updateControlState();
    }
  }

  function handleGlobalKeydown(event) {
    if (elements.settingsDrawer.classList.contains("open") && event.key === "Tab") {
      const focusable = getFocusable(elements.settingsDrawer);
      if (!focusable.length) {
        event.preventDefault();
        elements.settingsDrawer.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
    if (event.key === "Escape") {
      if (elements.resetDialog.open) return;
      if (state.sessionMenuFor) {
        event.preventDefault();
        closeSessionMenu();
        return;
      }
      if (!elements.modelMenu.hidden) {
        event.preventDefault();
        closeModelMenu({ restoreFocus: true });
        return;
      }
      if (elements.settingsDrawer.classList.contains("open")) {
        event.preventDefault();
        closeSettings();
        return;
      }
      if (elements.sidebar.classList.contains("open")) {
        event.preventDefault();
        closeSidebar();
        state.sidebarOpener?.focus?.();
      }
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k" && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      requestNewConversation();
    }
  }

  function bindEvents() {
    elements.mobileMenuButton.addEventListener("click", (event) => openSidebar(event.currentTarget));
    elements.sidebarClose.addEventListener("click", closeSidebar);
    elements.sidebarScrim.addEventListener("click", closeSidebar);
    elements.archivedToggle.addEventListener("click", toggleArchivedSection);
    elements.settingsButton.addEventListener("click", (event) => openSettings(event.currentTarget));
    elements.topbarSettingsButton.addEventListener("click", (event) => openSettings(event.currentTarget));
    elements.settingsClose.addEventListener("click", () => closeSettings());
    elements.drawerScrim.addEventListener("click", () => closeSettings());
    elements.settingsNav.querySelectorAll("[data-settings-view]").forEach((button) => {
      button.addEventListener("click", () => setSettingsView(button.dataset.settingsView));
    });
    elements.addProviderButton.addEventListener("click", () => {
      if (!state.configDraft) return;
      state.configDraft.providers = Array.isArray(state.configDraft.providers) ? state.configDraft.providers : [];
      state.configDraft.providers.push(ensureProviderDefaults());
      state.providerSecretStates.push(false);
      refreshProviderSecretStates();
      markConfigDirty();
      renderConfigEditors();
      setSettingsView("providers");
      const cards = elements.providerEditor.querySelectorAll(".provider-card");
      const card = cards[cards.length - 1];
      if (card) {
        card.open = true;
        card.scrollIntoView({ block: "nearest" });
      }
    });
    elements.reloadConfigButton.addEventListener("click", loadConfigDraft);
    elements.saveConfigButton.addEventListener("click", saveConfigDraft);
    elements.applyAdvancedConfigButton.addEventListener("click", applyAdvancedConfig);
    elements.themeButton.addEventListener("click", () => setTheme(elements.body.dataset.theme === "graphite" ? "linen" : "graphite"));
    elements.sidebarThemeButton.addEventListener("click", () => setTheme(elements.body.dataset.theme === "graphite" ? "linen" : "graphite"));
    document.querySelectorAll("[data-theme-choice]").forEach((button) => button.addEventListener("click", () => setTheme(button.dataset.themeChoice)));
    document.querySelectorAll("[data-scheme-choice]").forEach((button) => button.addEventListener("click", () => setColorScheme(button.dataset.schemeChoice)));
    document.querySelectorAll("[data-chat-font]").forEach((button) => button.addEventListener("click", () => setChatFontSize(button.dataset.chatFont)));
    elements.reasoningExpandToggle?.addEventListener("click", () => setReasoningExpanded(!state.reasoningExpanded));
    elements.toolExpandToggle?.addEventListener("click", () => setToolExpanded(!state.toolExpanded));
    elements.modeSwitch.querySelectorAll("[data-mode]").forEach((button) => button.addEventListener("click", () => setMode(button.dataset.mode)));
    elements.modelButton.addEventListener("click", (event) => {
      event.stopPropagation();
      if (elements.modelMenu.hidden) openModelMenu();
      else closeModelMenu({ restoreFocus: true });
    });
    elements.modelMenu.addEventListener("keydown", (event) => {
      const items = Array.from(elements.modelMenu.querySelectorAll("button:not(:disabled)"));
      const index = items.indexOf(document.activeElement);
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const direction = event.key === "ArrowDown" ? 1 : -1;
        items[(index + direction + items.length) % items.length]?.focus();
      } else if (event.key === "Home" || event.key === "End") {
        event.preventDefault();
        items[event.key === "Home" ? 0 : items.length - 1]?.focus();
      } else if (event.key === "Escape") {
        event.preventDefault();
        closeModelMenu({ restoreFocus: true });
      }
    });
    document.addEventListener("click", (event) => {
      if (!elements.modelMenu.hidden && !event.target.closest("#modelMenuWrap")) closeModelMenu();
      if (state.sessionMenuFor && !event.target.closest(".session-menu") && !event.target.closest(".session-menu-button")) closeSessionMenu();
    });
    elements.promptGrid.querySelectorAll("[data-prompt]").forEach((button) => {
      button.addEventListener("click", () => {
        if (elements.composerInput.disabled) return;
        elements.composerInput.value = button.dataset.prompt || "";
        resizeComposer();
        elements.composerInput.focus();
      });
    });
    elements.composerInput.addEventListener("input", resizeComposer);
    elements.composerInput.addEventListener("compositionstart", () => {
      state.composing = true;
    });
    elements.composerInput.addEventListener("compositionend", () => {
      state.composing = false;
    });
    elements.composerInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !state.composing && event.keyCode !== 229) {
        event.preventDefault();
        if (!elements.sendButton.disabled) elements.composerForm.requestSubmit();
      }
    });
    elements.composerForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitTurn();
    });
    elements.loginForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitLogin();
    });
    elements.newChatButton.addEventListener("click", requestNewConversation);
    elements.retryBootstrapButton.addEventListener("click", loadBootstrap);
    elements.resetConfirmButton.addEventListener("click", resetConversation);
    elements.chatScroll.addEventListener("scroll", () => {
      state.nearBottom = isNearBottom();
      if (state.programmaticScroll) return;
      if (!state.followOutput && isAtBottom()) {
        state.followOutput = true;
        elements.jumpBottomButton.hidden = true;
      } else if (!state.followOutput || !state.nearBottom) {
        suspendOutputFollowing();
      }
    }, { passive: true });
    elements.chatScroll.addEventListener("wheel", (event) => {
      if (event.deltaY < 0) suspendOutputFollowing();
    }, { passive: true });
    elements.chatScroll.addEventListener("touchmove", () => {
      suspendOutputFollowing();
    }, { passive: true });
    elements.jumpBottomButton.addEventListener("click", () => scrollToBottom({ force: true, smooth: true }));
    window.addEventListener("resize", updateJumpButtonOffset, { passive: true });
    if (window.visualViewport) {
      window.visualViewport.addEventListener("resize", syncAppHeight, { passive: true });
      syncAppHeight();
    }
    document.addEventListener("keydown", handleGlobalKeydown);
  }

  function syncAppHeight() {
    const viewport = window.visualViewport;
    if (!viewport) return;
    document.documentElement.style.setProperty("--app-height", `${Math.round(viewport.height * viewport.scale)}px`);
  }

  function initialize() {
    renderIconSlots();
    setTheme(safeStorageGet("miyu.web.theme") || "graphite", false);
    const storedScheme = safeStorageGet("miyu.web.colorScheme");
    if (storedScheme) setColorScheme(storedScheme, false);
    probeMatugenTheme();
    setChatFontSize(safeStorageGet("miyu.web.chatFontSize") || "15px", false);
    setReasoningExpanded(safeStorageGet("miyu.web.reasoningExpanded") === "true", false);
    setToolExpanded(safeStorageGet("miyu.web.toolExpanded") === "true", false);
    setMode(safeStorageGet("miyu.web.mode") || "normal", false);
    setSettingsView("interface");
    bindEvents();
    resizeComposer();
    updateSettingsControls();
    loadBootstrap();
  }

  initialize();
})();

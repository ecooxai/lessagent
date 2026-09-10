"use strict";
const $ = (s) => document.querySelector(s),
  el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  };
let token =
  new URLSearchParams(location.hash.slice(1)).get("token") ||
  sessionStorage.getItem("lessagent-token") ||
  "";
if (token) sessionStorage.setItem("lessagent-token", token);
history.replaceState(null, "", location.pathname);
let state = null,
  selected = null,
  inventory = null,
  viewKey = "",
  chatStamp = "",
  terminalStamp = "",
  drafts = {},
  polling = false,
  peer = null,
  stream = null,
  channel = null,
  voiceWorkspace = null,
  blobUrls = [],
  noticeTimer,
  logFreshTimer,
  logRecentTimer,
  logResumeTimer;
let startupAgentOpened = false,
  lastLogMarker = null,
  logPauseUntil = 0;
function uiActive() { return !document.hidden && document.hasFocus(); }
function surfaceActive(host) { return uiActive() && host.isConnected && !host.closest("[hidden]"); }
function resumeUi() {
  if (!uiActive()) return;
  if (!viewKey.startsWith("standalone/")) render();
  void poll();
  document.dispatchEvent(new Event("lessagent:resume"));
}
window.addEventListener("focus", resumeUi);
document.addEventListener("visibilitychange", resumeUi);
const api = async (path, body, raw = false, signal) => {
  const r = await fetch(path, {
    signal,
    method: body === undefined ? "GET" : "POST",
    headers: {
      ...(token ? {Authorization: `Bearer ${token}`} : {}),
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (r.status === 401 && path !== "/api/login") {
    if (token) {
      token = "";
      sessionStorage.removeItem("lessagent-token");
      return api(path, body, raw, signal);
    }
    showLogin();
  }
  if (!r.ok) {
    let message = await r.text();
    try {
      message = JSON.parse(message).error || message;
    } catch {}
    throw Error(message);
  }
  return raw ? r : r.json();
};
function showLogin() {
  document.body.classList.add("authenticating");
  const dialog = $("#login-dialog");
  if (!dialog.open) { dialog.showModal(); $("#login-password").focus(); }
}
$("#login-dialog").addEventListener("cancel", event => event.preventDefault());
$("#login-form").addEventListener("submit", async event => {
  event.preventDefault();
  const button = event.submitter;
  button.disabled = true;
  $("#login-error").textContent = "";
  try {
    const result = await api("/api/login", {password: $("#login-password").value});
    token = result.token;
    sessionStorage.setItem("lessagent-token", token);
    $("#login-password").value = "";
    $("#login-dialog").close();
    document.body.classList.remove("authenticating");
    await poll();
  } catch (error) { $("#login-error").textContent = error.message; }
  finally { button.disabled = false; }
});
const act = (name, body = {}, signal) => api(`/api/action/${name}`, body, false, signal);
const workspace = () => state?.workspaces.find((w) => w.id === selected);
const guard =
  (fn) =>
  (...args) => {
    try {
      return Promise.resolve(fn(...args)).catch(notice);
    } catch (e) {
      notice(e);
    }
  };
function notice(error) {
  $("#notice").hidden = false;
  $("#notice").textContent = error.message || String(error);
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => ($("#notice").hidden = true), 15000);
}
function button(text, fn, cls) {
  const b = el("button", cls, text);
  b.type = "button";
  b.onclick = guard(fn);
  return b;
}
function ask(title, label, value = "") {
  return new Promise((resolve) => {
    const d = $("#dialog");
    $("#dialog-title").textContent = title;
    $("#dialog-label").textContent = label;
    $("#dialog-value").value = value;
    d.returnValue = "cancel";
    d.onclose = () =>
      resolve(d.returnValue === "ok" ? $("#dialog-value").value : null);
    d.showModal();
    $("#dialog-value").focus();
  });
}
function ui() {
  const w = workspace();
  return w?.ui?.tabs
    ? w.ui
    : { tabs: [{ id: "chat", kind: "chat", title: "Agent" }], active: "chat" };
}
async function saveDraft(id, text, session = "chat") {
  drafts[session === "chat" ? id : `${id}:${session}`] = text;
  const w = state?.workspaces.find((w) => w.id === id);
  if (w) {
    w.ui = session === "chat" ? { ...w.ui, draft: text } : {...w.ui, drafts:{...w.ui?.drafts, [session]:text}};
    await act("draft", { workspace: id, text, session });
  }
}
const localTabState = new Map();
let uiSaveQueue = Promise.resolve();
function retainLocalTabs(next) {
  for (const w of next.workspaces) {
    const tabs = localTabState.get(w.id);
    if (tabs) w.ui = {...w.ui, ...structuredClone(tabs)};
  }
  return next;
}
async function saveUi() {
  if (!workspace()) return;
  const id = selected, saved = structuredClone(workspace().ui);
  localTabState.set(id, {tabs:saved.tabs, active:saved.active});
  // Keep quick tab selections ordered even when HTTP requests finish out of order.
  const write = uiSaveQueue.catch(() => {}).then(() => act("ui", {workspace:id, ui:saved}));
  uiSaveQueue = write;
  await write;
}
async function select(id) {
  const previous = state.workspaces.find(w => w.id === id);
  if (previous) {
    const opened = await act("workspace_open", {path: previous.path});
    Object.assign(previous, opened);
    retainLocalTabs(state);
  }
  selected = id;
  inventory = null;
  viewKey = "";
  await act("ui", { ui: { ...state.ui, selected: id } });
  render();
  await scan();
}
async function openTab(kind, title, extra = {}) {
  if (!workspace() && kind !== "settings" && kind !== "logs") {
    notice("Open a workspace folder first.");
    return;
  }
  if (!workspace()) {
    viewKey = "";
    showStandalone(kind);
    return;
  }
  const u = ui(),
    id = extra.id || kind;
  let t = u.tabs.find((t) => t.id === id);
  if (!t) {
    t = { id, kind, title, ...extra };
    if (kind === "chat") u.tabs.splice(u.tabs.findLastIndex(tab => tab.kind === "chat") + 1, 0, t);
    else u.tabs.push(t);
    u.tabs = [...u.tabs.filter(tab => !["file", "files"].includes(tab.kind)), ...u.tabs.filter(tab => ["file", "files"].includes(tab.kind))];
  }
  u.active = id;
  workspace().ui = u;
  viewKey = "";
  render();
  await saveUi();
}
async function closeTab(id) {
  const u = ui();
  u.tabs = u.tabs.filter((t) => t.id !== id);
  if (u.active === id) u.active = u.tabs[0]?.id || null;
  workspace().ui = u;
  viewKey = "";
  render();
  await saveUi();
}
async function scan() {
  if (!selected) return;
  const id = selected;
  const inv = await api(`/api/inventory/${id}`);
  if (id === selected) {
    inventory = inv;
    if (!uiActive()) return;
    renderContext();
    if (ui().active === "files") renderFiles();
    if (ui().active === "context") renderContextDetails();
  }
}
function renderContext() {
  const c = $("#context");
  c.replaceChildren(el("span", "", "◫  Project context"));
  if (inventory) c.append(el("span", "context-total", formatTokens(inventory.total_tokens)));
  c.title = "View context by folder and file";
}
const formatTokens = n => n >= 100000 ? (n / 1000000).toLocaleString(undefined, {maximumFractionDigits:2}) + "M tokens" : (n / 1000).toLocaleString(undefined, {maximumFractionDigits: n < 100 ? 3 : 1, minimumFractionDigits: 1}) + "k tokens";
const formatBytes = n => n >= 100000 ? (n/1000000).toFixed(2) + " MB" : (n/1024).toFixed(1) + " KB";
function renderContextDetails() {
  const root = $("#view");
  if (root.querySelector(".file-browser[data-context]")) { root.querySelector(".file-browser").refreshTree?.(); return; }
  root.replaceChildren();
  root.append(createFileBrowser({workspaceId:selected, root:workspace().path, context:true}));
}
function showMenu(anchor, items) {
  const menu = $("#menu"); menu.replaceChildren();
  for (const item of items) {
    const b = button(item.label, async () => { menu.hidden = true; await item.run(); });
    b.disabled = !!item.disabled; menu.append(b);
  }
  menu.hidden = false;
  const r = anchor.getBoundingClientRect();
  menu.style.left = Math.max(8, Math.min(r.left, innerWidth-menu.offsetWidth-8)) + "px";
  menu.style.top = Math.min(r.bottom+5, innerHeight-menu.offsetHeight-8) + "px";
  menu.querySelector("button:not(:disabled)")?.focus();
}
function workspaceMenu(anchor, id) {
  const w = state.workspaces.find(w => w.id === id);
  showMenu(anchor, [
    {label: (w.mode === "normal" ? "✓ " : "") + "Normal mode", run: () => setMode(id, "normal")},
    {label:"Close workspace", run:async () => { await act("workspace_close", {workspace:id}); if (selected === id) { selected = null; viewKey = ""; inventory = null; } await poll(); }},
    {label: (w.mode === "light" ? "✓ " : "") + "Light mode", disabled: id === selected && inventory && !inventory.light_allowed, run: () => setMode(id, "light")},
  ]);
}
async function setMode(id, mode) { await act("workspace_mode", {workspace:id, mode}); await poll(); }
function newTabMenu(anchor) {
  showMenu(anchor, [
    {label:"Agent", run: () => openTab("chat", "Agent " + (ui().tabs.filter(t => t.kind === "chat").length + 1), {id:"agent-" + crypto.randomUUID()})},
    {label:"Files", run: async () => { await openTab("files", "Files"); await scan(); }},
    {label:"Terminal", run:newTerminal},
    {label:"Git", run: () => openTab("git", "Git")},
    {label:"History", run: () => openTab("history", "History")},
    {label:"Managed terminals", run: () => openTab("managed", "Managed terminals")},
    {label:"Webview", run:newWebview},
  ]);
}

function render() {
  if (!state || !uiActive()) return;
  const w = workspace();
  document.body.dataset.mode = w?.mode || "normal";
  $("#workspace-title").textContent = w?.path || "Open a folder";
  $("#workspace-title").title = w?.path || "";
  renderAgentStatus();
  const list = $("#workspaces");
  const workspaceStamp = JSON.stringify(state.workspaces.map(x => [x.id, x.path]));
  if (list.dataset.stamp !== workspaceStamp) {
  list.dataset.stamp = workspaceStamp;
  list.replaceChildren();
  for (const x of state.workspaces) {
    const b = button(
      x.path.split("/").filter(Boolean).at(-1) || x.path,
      () => { if (selected !== x.id) return select(x.id); },
      "workspace" + (x.id === selected ? " active" : ""),
    );
    b.dataset.workspace = x.id;
    b.title = x.path;
    b.ondblclick = () => workspaceMenu(b, x.id);
    b.oncontextmenu = e => { e.preventDefault(); workspaceMenu(b, x.id); };
    list.append(b);
  }
  }
  for (const b of list.children) b.classList.toggle("active", b.dataset.workspace === selected);
  renderContext();
  const tabs = $("#tabs");
  const tabStamp = JSON.stringify([selected, ui().active, ui().tabs]);
  if (tabs.dataset.stamp !== tabStamp) {
  tabs.dataset.stamp = tabStamp;
  tabs.replaceChildren();
  const add = button("＋", e => newTabMenu(e.currentTarget), "new-tab");
  add.title = "New tab"; add.setAttribute("aria-label", "New tab"); tabs.append(add);
  if (w) {
    for (const t of ui().tabs) {
      const b = button(
        shortTabTitle(t.title),
        async () => {
          ui().active = t.id;
          viewKey = "";
          render();
          await saveUi();
        },
        ui().active === t.id ? "active" : "",
      );
      b.title = t.title;
      {
        const x = el("span", "close", "×");
        x.onclick = guard(async (e) => {
          e.stopPropagation();
          await closeTab(t.id);
        });
        b.append(x);
      }
      tabs.append(b);
    }
  }
  }
  const t = ui().tabs.find((t) => t.id === ui().active) || {
    kind: "empty",
    id: "empty",
  };
  const key = `${selected}/${t.id}`;
  if (key !== viewKey) {
    viewKey = key;
    chatStamp = "";
    terminalStamp = "";
    document.querySelector(".file-preview-popup")?.close();
    for (const u of blobUrls) URL.revokeObjectURL(u);
    blobUrls = [];
    rememberAgentDetails($("#view"));
    $("#view").querySelectorAll(".terminal-surface").forEach(n=>n.terminalCleanup?.());
    $("#view").replaceChildren();
    renderView(t);
  } else if (t.kind === "chat") updateChat();
  else if (["terminals", "managed"].includes(t.kind)) updateTerminals();
  else if (t.kind === "logs") renderLogs();
  else if (t.kind === "history") renderHistory();
  else if (t.kind === "session") renderSession(t);
}
function showStandalone(kind) {
  $("#view").querySelectorAll(".terminal-surface").forEach(n=>n.terminalCleanup?.());
    $("#view").replaceChildren();
  viewKey = "standalone/" + kind;
  renderView({ kind });
}
function renderView(t) {
  ({
    empty: () => $("#view").append(el("p", "empty", "Open a new tab with + to continue.")),
    chat: renderChat,
    history: renderHistory,
    git: () => guard(renderGit)(),
    session: () => renderSession(t),
    terminals: renderTerminals,
    managed: () => renderTerminals(true),
    context: renderContextDetails,
    settings: renderSettings,
    logs: renderLogs,
    files: renderFiles,
    file: () => guard(renderFile)(t),
    web: () => renderWeb(t),
  })[t.kind]?.();
}
const expandedRequests = new Set();
const agentScrollPositions = new Map();
function requestContextTree(request, fold) {
  const root = {children:new Map(), tokens:0, unknown:false};
  const recorded = new Map((request.project_file_tokens || []).map(f => [f.path, f.tokens]));
  for (const path of request.project_files || []) {
    const tokens = recorded.get(path);
    let node = root;
    for (const name of path.split("/").filter(Boolean)) {
      if (!node.children.has(name)) node.children.set(name, {children:new Map(), tokens:0, unknown:false});
      node = node.children.get(name);
      node.tokens += Number.isFinite(tokens) ? tokens : 0;
      node.unknown ||= !Number.isFinite(tokens);
    }
  }
  const tree = el("div", "request-context-tree");
  function append(parent, node, prefix) {
    for (const [name, child] of [...node.children].sort((a,b) => b[1].tokens-a[1].tokens || a[0].localeCompare(b[0]))) {
      const path = prefix ? prefix + "/" + name : name;
      const label = el("span", "context-tree-name", name);
      const count = el("span", "context-tree-tokens", child.unknown ? "Estimate unavailable" : child.tokens.toLocaleString() + " tokens");
      if (child.children.size) {
        const folder = fold(request.step + ":folder:" + path, "");
        folder.classList.add("context-tree-folder");
        folder.firstChild.append(label, count);
        append(folder, child, path);
        parent.append(folder);
      } else {
        const row = el("div", "context-tree-file"); row.title = path;
        row.append(label, count); parent.append(row);
      }
    }
  }
  append(tree, root, "");
  if (!root.children.size) tree.append(el("p", "muted", "No files"));
  return tree;
}
function requestDetails(job) {
  const details = el("details", "request-details");
  details.open = expandedRequests.has(job.id);
  details.ontoggle = () => { if (!details.isConnected) return; if (details.open) expandedRequests.add(job.id); else expandedRequests.delete(job.id); };
  const requests = job.events.filter(e => e.kind === "request");
  details.append(el("summary", "", `What was sent to AI · ${requests.length} requests`));
  const count = n => Number.isFinite(n) ? `${n.toLocaleString()} estimated tokens` : "Token estimate unavailable";
  const fold = (key, label, value) => {
    const part = el("details", "request-part");
    const id = `${job.id}:${key}`;
    part.open = expandedRequests.has(id);
    part.ontoggle = () => { if (!part.isConnected) return; if (part.open) expandedRequests.add(id); else expandedRequests.delete(id); };
    part.append(el("summary", "", label));
    if (value !== undefined) part.append(el("pre", "", value || ""));
    return part;
  };
  for (const request of requests) {
    const tokens = request.part_tokens || {};
    const fileCounts = new Map((request.project_file_tokens || []).map(f => [f.path, f.tokens]));
    const paths = request.project_files || [];
    const filesTotal = paths.every(path => Number.isFinite(fileCounts.get(path))) ? paths.reduce((sum, path) => sum + fileCounts.get(path), 0) : null;
    const extraContext = Number.isFinite(filesTotal) && Number.isFinite(tokens.context) ? Math.max(0, tokens.context - filesTotal) : null;
    const item = fold(`${request.step}`, `Request ${request.step} · ${request.model} · ${request.thinking} · ${count(tokens.total)}`);
    const parts = [
      ["context", "Context files sent to AI", (request.project_files || []).join("\n") || "No files"],
      ["prompt", "User prompt", request.prompt],
      ["system", "System prompt", request.system],
      ["instructions", "Instruction prompt", request.instructions],
    ];
    for (const [key, label, value] of parts) {
      const part = fold(`${request.step}:${key}`, `${label} · ${count(key === "context" ? filesTotal : tokens[key])}`, key === "context" ? undefined : value);
      if (key === "context") {
        const source = `Source: ${request.context_directory || "Unknown"}. ${request.context_note || "File contents hidden."}`;
        const subtotal = filesTotal == null ? "File content subtotal unavailable." : `File content subtotal: ${count(filesTotal)}.`;
        part.append(requestContextTree(request, fold), el("p", "muted", `${source} ${subtotal}`));
      }
      item.append(part);
    }
    if (request.recent_ai_messages?.length) {
      const limit = Number.isFinite(request.recent_ai_limit) ? request.recent_ai_limit : request.recent_ai_messages.length;
      const recent = fold(`${request.step}:recent-ai`, `Previous AI messages · ${count(tokens.recent_ai)} · up to ${limit}`);
      request.recent_ai_messages.forEach((message, index) => recent.append(fold(`${request.step}:recent-ai:${index}`, `AI message ${index + 1} of ${request.recent_ai_messages.length} · ${count(tokens.recent_ai_messages?.[index])}`, message)));
      item.append(recent);
    }
    item.append(fold(`${request.step}:evidence`, `${["files_only", "files_and_recent_ai"].includes(request.context_policy) ? "Context labels and formatting" : "Additional context and evidence"} · ${count(extraContext)}`,
      ["files_only", "files_and_recent_ai"].includes(request.context_policy) ? `Only selected file contents and attachments are sent as context. This small difference accounts for file labels, separators, iteration/output location, and token boundaries. Full context message: ${count(tokens.context)}.` :
      `Saved task knowledge, tool observations, the previous assistant response, extra screenshots, and context labels/separators. This is the remaining context-message estimate after subtracting file estimates; token boundaries can affect the difference.\nFull context message: ${count(tokens.context)}.\nRequest total includes files, this additional context, the user prompt, system prompt, and instruction prompt.`));
    item.append(fold(`${request.step}:metadata`, "Request details", [
      `Provider message order: ${(request.message_order || []).join(" → ")}`,
      `Iteration: ${request.iteration || request.step}`,
      `Output directory: ${request.output_directory || "Unknown"}`,
      `${request.history_turns} message turns · ${request.images} images`,
      tokens.method || "Token estimates were not recorded for this request.",
    ].join("\n")));
    details.append(item);
  }
  return details;
}
function renderChat() {
  const root = el("div", "chat"),
    messages = el("div", "messages");
  messages.id = "messages";
  root.append(messages);
  const form = el("form", "composer"),
    input = el("textarea");
  input.placeholder = "Give your agent a task…";
  input.setAttribute("aria-label", "Task prompt");
  const session = ui().active || "chat";
  messages.dataset.session = session;
  messages.dataset.workspace = selected;
  messages.onscroll = () => agentScrollPositions.set(`${draftWorkspace}/${session}`, {top:messages.scrollTop, bottom:messages.scrollHeight-messages.scrollTop-messages.clientHeight < 80});
  input.value = drafts[session === "chat" ? selected : `${selected}:${session}`] ?? (session === "chat" ? workspace()?.ui?.draft : workspace()?.ui?.drafts?.[session]) ?? "";
  const draftWorkspace = selected;
  input.oninput = guard(() => saveDraft(draftWorkspace, input.value, session));
  input.onkeydown = (e) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      form.requestSubmit();
    }
  };
  const row = el("div", "composer-actions"),
    left = el("div", "row");
  const model = el("select", "composer-model");
  model.setAttribute("aria-label", "Model");
  const selectedModel = state.settings.model;
  const option = el("option", "", selectedModel || "Default model"); option.value = selectedModel; model.append(option);
  model.onchange = guard(async () => { await act("settings", {...state.settings, model:model.value}); await poll(); });
  api("/api/models/" + state.settings.provider).then(models => {
    if (!model.isConnected) return;
    for (const m of models) if (m.id !== selectedModel) { const o = el("option", "", m.name || m.id); o.value = m.id; model.append(o); }
  }).catch(e => { model.title = "Model discovery unavailable: " + e.message; });
  left.append(model, iconButton(peer ? "◉" : "◎", peer ? "End voice" : "Voice", toggleVoice), iconButton("♫", "Upload audio", uploadAudio));
  const right = el("div", "row");
  const stop = iconButton("■", "Stop agent", async () => {
    const j = state.jobs.find(
      (j) => j.workspace === draftWorkspace && (j.session || "chat") === session && j.status === "running",
    );
    if (j) await act("stop", { job_id: j.id });
  });
  stop.id = "stop-job";
  const submit = el("button", "primary icon-button", "↑");
  submit.title = "Send task"; submit.setAttribute("aria-label", "Send task");
  submit.type = "button";
  installThinkingSend(submit, form);
  submit.id = "run-job";
  right.append(stop, submit);
  row.append(left, right);
  form.append(input, row);
  form.onsubmit = guard(async (e) => {
    e.preventDefault();
    if (!workspace()) {
      notice("Use the folder button in the sidebar to open a folder.");
      return;
    }
    await act("run", { workspace: draftWorkspace, prompt: input.value, session, thinking:state.settings.thinking });
    input.value = "";
    await saveDraft(draftWorkspace, "", session);
    await poll();
  });
  root.append(form);
  $("#view").append(root);
  updateChat();
}
function updateChat() {
  const m = $("#messages");
  if (!m) return;
  const workspaceId = m.dataset.workspace;
  const w = state.workspaces.find(w => w.id === workspaceId);
  const session = m.dataset.session;
  const messages = (w?.messages || []).filter(m => (m.session || "chat") === session);
  const jobs = state.jobs.filter((j) => j.workspace === workspaceId && (j.session || "chat") === session);
  const current = jobs.at(-1),
    running = current?.status === "running";
  $("#run-job").disabled = state.jobs.some(j => j.workspace === selected && j.status === "running") || !w;
  $("#stop-job").hidden = !running;
  const stamp = JSON.stringify([workspaceId, session, messages, jobs]);
  if (stamp === chatStamp) return;
  chatStamp = stamp;
  const scrollKey = `${workspaceId}/${session}`;
  const firstRender = !m.dataset.rendered;
  const savedScroll = agentScrollPositions.get(scrollKey);
  const nearBottom = firstRender && !savedScroll;
  const scrollTop = firstRender ? (savedScroll?.top ?? 0) : m.scrollTop;
  m.dataset.rendered = "true";
  rememberAgentDetails(m);
  m.replaceChildren();
  if (!messages.length) {
    const welcome = el("div", "welcome");
    welcome.append(

      el(
        "h2",
        "",
        w ? "What shall we work on?" : "Your workspace, always ready.",
      ),
      el(
        "p",
        "",
        w
          ? ""
          : "Open a local folder to start. Connect a model, give it a task, and follow the work in persistent terminals.",
      ),
    );
    const chips = el("div", "chips");
    for (const text of [
      "Explore this project",
      "Run the tests",
      "Find improvements",
    ])
      chips.append(
        button(text, () => {
          const input = $(".composer textarea");
          input.value = text;
          guard(saveDraft)(selected, text, session);
          input.focus();
        }),
      );
    welcome.append(chips);
    m.append(welcome);
  }
  for (const msg of messages) {
    const block = el("div", "message " + msg.role);
    block.append(
      el("span", "role", msg.role === "user" ? "You" : "Lessagent"),
      msg.role === "assistant" ? renderAgentTags(msg.text, `${msg.job_id || "message"}:assistant`) : document.createTextNode(msg.text),
    );
    if (msg.role === "assistant")
      block.append(button("Listen", () => speak(msg.text)));
    const completed = jobs.find(j => j.id === msg.job_id && j.status !== "running");
    if (msg.role === "assistant" && completed) block.append(el("p", "muted task-usage", taskMetrics(completed)));
    if (msg.role === "user") { const job = jobs.find(j => j.id === msg.job_id); if (job) block.append(requestDetails(job)); }
    m.append(block);
    if (msg.role === "user") {
      const job = jobs.find(j => j.id === msg.job_id);
      if (job?.events.length) {
        const details = el("section", "agent-activity");
        details.append(
          el(
            "h3",
            "muted",
            `${job.status} · ${job.events.length} events`,
          ),
        );
        const events = renderAgentEvents(job.events, job.id);
        details.append(events);
        m.append(details);
      }
    }

  }
  for (const job of jobs.filter(j => j.status !== "running" && !(w?.messages || []).some(m => m.role === "assistant" && m.job_id === j.id))) {
    m.append(el("div", "muted task-usage", `${shortTabTitle(job.prompt)} · ${taskMetrics(job)}`));
  }
  m.scrollTop = nearBottom ? m.scrollHeight : scrollTop;
  agentScrollPositions.set(scrollKey, {top:m.scrollTop, bottom:m.scrollHeight-m.scrollTop-m.clientHeight < 80});
}
function cleanAgentText(text = "") {
  return text.replace(/<summary>[\s\S]*?<\/summary>/gi, "").replace(/<scores?>[\s\S]*?<\/scores?>/gi, "").replace(/<test>[\s\S]*?<\/test>/gi, "").replace(/<done>/gi, "").trim();
}
// Build Markdown with DOM nodes: model HTML is never interpreted as executable markup.
function renderMarkdown(text) {
  const root = el("div", "markdown");
  function inline(parent, value) {
    const pattern = /(`[^`]+`|\*\*[^*]+\*\*|__[^_]+__|\*[^*\n]+\*|\[[^\]]+\]\([^\s)]+\))/g;
    let start = 0;
    for (const match of value.matchAll(pattern)) {
      parent.append(document.createTextNode(value.slice(start, match.index)));
      const token = match[0];
      let node;
      if (token.startsWith('`')) node = el('code', '', token.slice(1,-1));
      else if (token.startsWith('**') || token.startsWith('__')) { node = el('strong'); inline(node, token.slice(2,-2)); }
      else if (token.startsWith('*')) { node = el('em'); inline(node, token.slice(1,-1)); }
      else {
        const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(token);
        if (/^(https?:\/\/|mailto:|\/[^/]|#)/i.test(link[2])) {
          node = el('a', '', link[1]); node.href = link[2]; node.target = '_blank'; node.rel = 'noopener noreferrer';
        } else node = document.createTextNode(token);
      }
      parent.append(node); start = match.index + token.length;
    }
    parent.append(document.createTextNode(value.slice(start)));
  }
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  for (let i = 0; i < lines.length;) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    const fence = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      const code = []; i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) code.push(lines[i++]);
      if (i < lines.length) i++;
      const pre = el('pre'); pre.append(el('code', '', code.join('\n'))); root.append(pre); continue;
    }
    const heading = /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading) { const node = el('h'+heading[1].length); inline(node, heading[2]); root.append(node); i++; continue; }
    if (/^\s*([-*_])(?:\s*\1){2,}\s*$/.test(line)) { root.append(el('hr')); i++; continue; }
    if (/^>\s?/.test(line)) {
      const quote = []; while (i < lines.length && /^>\s?/.test(lines[i])) quote.push(lines[i++].replace(/^>\s?/, ''));
      const node = el('blockquote'); node.append(renderMarkdown(quote.join('\n'))); root.append(node); continue;
    }
    const listItem = /^\s*(?:([-+*])|(\d+)[.)])\s+(.+)$/.exec(line);
    if (listItem) {
      const ordered = !!listItem[2], list = el(ordered ? 'ol' : 'ul');
      if (ordered) list.start = Number(listItem[2]);
      while (i < lines.length) {
        const item = /^\s*(?:([-+*])|(\d+)[.)])\s+(.+)$/.exec(lines[i]);
        if (!item || !!item[2] !== ordered) break;
        const node = el('li'); inline(node, item[3]); list.append(node); i++;
      }
      root.append(list); continue;
    }
    const paragraph = [line]; i++;
    while (i < lines.length && lines[i].trim() && !/^(#{1,6}\s|>|\s*([-+*]|\d+[.)])\s|\s*(`{3,}|~{3,}))/.test(lines[i])) paragraph.push(lines[i++]);
    const node = el('p'); inline(node, paragraph.join('\n')); root.append(node);
  }
  return root;
}
const expandedAgentDetails = new Map();
function rememberAgentDetails(root) {
  root?.querySelectorAll('details[data-agent-detail]').forEach(row => expandedAgentDetails.set(row.dataset.agentDetail, row.open));
}
function trackAgentDetail(row, key) {
  row.dataset.agentDetail = key;
  row.open = expandedAgentDetails.get(key) ?? false;
  row.ontoggle = () => { if (row.isConnected) expandedAgentDetails.set(key, row.open); };
}

// Render the Light-mode Markdown protocol without allowing tag contents to be
// interpreted as HTML. Each protocol section is independently collapsible and
// keeps its expansion state while the event list is refreshed.
function renderAgentTags(text = "", keyPrefix = "agent-tag") {
  const root = el("div", "markdown agent-tagged");
  const source = String(text ?? "").replace(/\r\n/g, "\n");
  const pattern = /<(review|summary|scores?|plan|bash|python|computer|done)>([\s\S]*?)<\/\1>/gi;
  let cursor = 0;
  let index = 0;
  const appendMarkdown = value => {
    if (!value || !value.trim()) return;
    const rendered = renderMarkdown(value);
    while (rendered.firstChild) root.append(rendered.firstChild);
  };
  const appendTag = (kind, value) => {
    const row = el("details", "agent-tag");
    trackAgentDetail(row, `${keyPrefix}:${index++}:${kind.toLowerCase()}`);
    const normalized = kind.toLowerCase();
    const title = normalized === "review" ? "Review" :
      normalized === "summary" ? "Summary" :
      normalized === "plan" ? "Plan" :
      normalized === "bash" ? "Bash command" :
      normalized === "python" ? "Python code" :
      normalized === "computer" ? "Computer action" :
      normalized === "done" ? "Completion" : "Score";
    row.append(el("summary", "", title));
    const body = el("div", "agent-tag-body");
    if (normalized === "bash" || normalized === "python" || normalized === "computer") {
      const pre = el("pre", "agent-tag-code");
      pre.append(el("code", "", value.trim()));
      body.append(pre);
    } else if (normalized === "done") {
      const done = /^(true|yes|1|done)$/i.test(value.trim());
      body.append(el("p", done ? "agent-tag-done" : "agent-tag-pending", done ? "Done" : "Not done"));
    } else if (normalized === "scores" || normalized === "score") {
      const parsed = Number(value.trim());
      body.append(el("p", "agent-tag-score", Number.isFinite(parsed) ? `Score · ${parsed}/10` : `Score · ${value.trim()}`));
    } else {
      const rendered = renderMarkdown(value.trim());
      body.append(rendered);
    }
    row.append(body);
    root.append(row);
  };
  for (const match of source.matchAll(pattern)) {
    appendMarkdown(source.slice(cursor, match.index));
    appendTag(match[1], match[2]);
    cursor = match.index + match[0].length;
  }
  // Remove dangling protocol markers from older replies such as `<done><done>`
  // while keeping their surrounding Markdown readable.
  appendMarkdown(source.slice(cursor).replace(/<\/?(?:review|summary|scores?|plan|bash|python|computer|done)>/gi, ""));
  return root;
}

function renderAgentEvents(events, jobId = "preview") {
  const root = el("div", "events agent-events");
  const occurrences = new Map();
  let group = root, step = null;
  for (const e of events) {
    if (e.archived_event) {
      const row = el("div", "event-detail");
      row.append(button(`Load ${e.kind || "event"} details (${Math.ceil(e.bytes / 1024)} KB)`, guard(async () => {
        const full = await act("event_read", {id:e.archived_event});
        row.replaceChildren(renderAgentEvents([full], jobId));
      })));
      group.append(row);
      continue;
    }

    if ((e.kind === "request" || e.kind === "model") && e.step !== step) {
      step = e.step;
      group = el("article", "message assistant iteration-message");
      group.dataset.step = step;
      group.append(el("h4", "event-step", `Step ${e.step} · ${e.provider || ""}`));
      root.append(group);
    }
    if (e.kind === "request") {
      group.append(requestDetails({id:`${jobId}/iteration/${e.step}`,events:[e]}));
      continue;
    }
    if (e.kind === "usage") {
      group.append(el("p", "muted request-usage", `${e.source} · ${e.model} · ${tokenMetrics(e.usage)}`));
      continue;
    }
    const signature = JSON.stringify(e);
    const occurrence = occurrences.get(signature) || 0;
    occurrences.set(signature, occurrence + 1);
    if (e.kind === "model") { group.querySelector(".event-step").textContent = `Step ${e.step} · ${e.provider}`; continue; }
    if (e.kind === "review") { group.append(el("p", "event-review", e.score == null ? "No quality score supplied" : `Quality: ${e.score}/10 · Target: ${e.threshold}`)); continue; }
    if (e.kind === "text") { const text = String(e.text || ""); if (text.trim()) { const answer = renderAgentTags(text, `${jobId}:text:${occurrence}`); answer.classList.add("event-answer"); group.append(answer); } continue; }
    const row = el("details", "event-detail");
    trackAgentDetail(row, `${jobId}/${signature}/${occurrence}`);
    const r = e.result || {};
    let title = e.kind;
    if (e.kind === "tool") title = `Run ${e.name}`;
    if (e.kind === "result") title = `${e.name || "Tool"} · ${r.error ? "Failed" : r.exit_code != null ? `Exit ${r.exit_code}` : "Result"}`;
    if (e.kind === "test") title = `Verification · ${r.interrupted ? "Interrupted" : r.exit_code === 0 ? "Passed" : "Failed"}`;
    if (e.kind === "assessment") title = "Quality score requested · reviewing task evidence";
    if (e.kind === "compaction") title = `Context compaction · ${e.status}`;
    if (e.kind === "light_action") title = `Light ${e.language || "code"} action`;
    if (e.kind === "light_result") title = `Light ${e.language || "code"} · ${r.error || (r.exit_code != null && r.exit_code !== 0) ? "Failed" : r.exit_code != null ? `Exit ${r.exit_code}` : "Result"}`;
    row.append(el("summary", "", title));
    const data = e.kind === "tool" || e.kind === "light_action" ? (e.arguments || e) : e.result || e;
    if (data.command || data.code) row.append(el("pre", "event-command", data.command || data.code));
    if (data.directory) row.append(el("p", "", `Artifacts: ${data.directory}`));
    if (data.log) row.append(el("p", "", `Log: ${data.log}`));
    if (data.output != null) row.append(el("pre", "event-output", data.output));
    if (data.stdout != null) row.append(el("pre", "event-output", data.stdout));
    if (data.stderr != null && data.stderr) row.append(el("pre", "event-error", data.stderr));
    if (data.error) row.append(el("p", "", data.error));
    if (data.output == null && data.stdout == null && !data.command && !data.code && !data.error && !data.stderr) row.append(el("pre", "", JSON.stringify(data, null, 2)));
    group.append(row);
  }
  return root;
}
function shortTabTitle(title = "") {
  const chars = Array.from(title);
  return chars.length > 25 ? chars.slice(0, 15).join("") + "…" + chars.slice(-10).join("") : title;
}
function tokenMetrics(u) {
  return u ? `Input (excluding cached): ${u.input_tokens.toLocaleString()} · Output: ${u.output_tokens.toLocaleString()} · Cached input: ${u.cached_input_tokens.toLocaleString()}` : "Token usage unavailable";
}
function taskMetrics(job) {
  return `Task total · ${tokenMetrics(job.usage)}${job.usage_incomplete ? " (partial usage)" : ""} · Time: ${job.elapsed_ms == null ? "unavailable" : (job.elapsed_ms / 1000).toFixed(2) + " s"}`;
}

function renderHistory() {
  const root = $("#view");
  root.replaceChildren(toolbar("History"));
  const jobs = state.jobs.filter(j => j.workspace === selected).slice().reverse();
  if (!jobs.length) root.append(el("p", "empty", "Previous chat tasks will appear here."));
  for (const job of jobs) {
    const row = button(`${job.prompt} · ${job.status} · ${new Date(job.started).toLocaleString()}`, () => openTab("session", job.prompt, {id: "session-" + job.id, jobId: job.id}), "history-session");
    root.append(row);
  }
}
function renderSession(tab) {
  const root = $("#view");
  const job = state.jobs.find(j => j.id === tab.jobId && j.workspace === selected);
  rememberAgentDetails(root);
  root.replaceChildren(toolbar("Chat session"));
  if (!job) { root.append(el("p", "empty", "Session unavailable.")); return; }
  root.append(el("div", "message user", job.prompt), renderAgentTags(job.output || job.status, `${job.id}:session-output`));
  const details = el("section", "agent-activity");
  details.append(el("h3", "muted", `${job.status} · ${job.events.length} events`), renderAgentEvents(job.events, job.id));
  root.append(details);
  if (job.status !== "running") root.append(el("p", "muted task-usage", taskMetrics(job)));
}
function toolbar(title, ...actions) {
  const t = el("div", "toolbar");
  t.append(el("h2", "", title));
  const row = el("div", "row");
  row.append(...actions);
  t.append(row);
  return t;
}
function renderTerminals(managed = false) {
  $("#view").append(
    toolbar(managed ? "Managed terminals" : "Terminals", ...(managed ? [] : [button("＋", newTerminal)])),
  );
  const grid = el("div", "terminal-grid");
  grid.id = "terminal-grid";
  grid.dataset.managed = String(managed);
  $("#view").append(grid);
  updateTerminals();
}
async function newTerminal() {
  if (!workspace()) return notice("Open a workspace first.");
  await act("terminal_new", { workspace: selected });
  await openTab("terminals", "Terminals");
  await poll();
}
function updateTerminals() {
  const grid = $("#terminal-grid");
  if (!grid) return;
  const ts = state.terminals.filter(t => t.workspace === selected && !!t.managed === (grid.dataset.managed === "true"))
    .sort((a,b) => Number(b.running)-Number(a.running) || Number(a.exited)-Number(b.exited) || b.created-a.created);
  grid.querySelector(".empty")?.remove();
  if (!ts.length) grid.append(el("p", "empty", grid.dataset.managed === "true" ? "Terminals started by the agent appear here." : "No terminals yet."));
  for (const node of [...grid.children])
    if (node.dataset.id && !ts.some((t) => t.id === node.dataset.id)) { node.querySelector(".terminal-surface")?.terminalCleanup?.(); node.remove(); }
  for (const t of ts) {
    let card = [...grid.children].find((n) => n.dataset.id === t.id);
    if (!card) {
      card = el("div", "terminal");
      card.dataset.id = t.id;
      const height = workspace()?.ui?.terminal_heights?.[t.id];
      if (height) card.style.height = `${height}px`;
      const h = el("header"),
        title = el("span", "", t.title);
      title.title = t.title;
      h.append(
        title,
        button("Split", newTerminal),
        button("Stop", () =>
          act("tool", {
            workspace: selected,
            name: "terminal_stop",
            arguments: { terminal_id: t.id },
          }),
        ),
        button("×", async () => {
          await act("terminal_remove", { terminal_id: t.id });
          await poll();
        }),
      );
      const screen = el("div", "terminal-surface");
      card.append(h, screen);
      grid.append(card);
      mountTerminal(screen, t.id, selected);
    }
    card.style.order = ts.indexOf(t);
    card.querySelector("header span").textContent =
      `${t.running ? "●" : "○"} ${t.title}${!t.exited ? (t.running ? " · running" : " · idle") : ""}${t.exited ? " · exit " + (t.exit_code ?? "archived") : ""}`;
    card.querySelector("header button:nth-of-type(2)").disabled = t.exited;
  }
}
const sendTerminal = (id, text) =>
  act("tool", {
    workspace: selected,
    name: "terminal_write",
    arguments: { terminal_id: id, text },
  });
function field(label, control, hint) {
  const l = el("label", "field");
  l.append(el("span", "", label), control);
  if (hint) l.append(el("small", "", hint));
  return l;
}
function input(value, type = "text") {
  const i = el("input");
  i.type = type;
  i.value = value;
  return i;
}
function renderSettings() {
  const root = el("div", "settings");
  root.append(
    el("h2", "", "Settings"),
    el("p", "muted", "Connections and preferences are stored by the backend."),
  );
  const provider = el("select");
  for (const p of ["codex", "openai", "gemini", "claude"]) {
    const o = el("option", "", p);
    o.value = p;
    provider.append(o);
  }
  provider.value = state.settings.provider;
  const model = input(state.settings.model),
    list = el("datalist");
  list.id = "models";
  model.setAttribute("list", "models");
  const refresh = button("Discover models", async () => {
    refresh.disabled = true;
    try {
      const models = await api("/api/models/" + provider.value);
      list.replaceChildren();
      for (const m of models) {
        const o = el("option");
        o.value = m.id;
        o.label = m.name;
        list.append(o);
      }
      model.placeholder = `${models.length} models available`;
      notice(`${models.length} models loaded. Choose one in the Model field.`);
    } finally {
      refresh.disabled = false;
    }
  });
  provider.onchange = () => {
    model.value = "";
    list.replaceChildren();
  };
  const compactModel = input(state.settings.compact_model || "gpt-5.6-luna");
  const steps = input(state.settings.max_steps, "number");
  steps.min = 1;
  steps.max = 100;
  const thinking = el("select");
  for (const level of ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]) {
    const o = el("option", "", level); o.value = level; thinking.append(o);
  }
  thinking.value = state.settings.thinking || "medium";
  const threshold = input(state.settings.quality_threshold ?? 9, "number");
  threshold.min = 0; threshold.max = 10; threshold.step = 0.1;
  const computer = input("", "checkbox");
  computer.checked = state.settings.computer_enabled;
  const ignores = el("textarea", "ignore-editor");
  ignores.value = (state.settings.context_ignores || []).join("\n");
  ignores.setAttribute("aria-label", "Context ignore patterns");
  root.append(
    field(
      "Provider",
      provider,
      "Requests go directly to the provider. Codex uses your existing Codex file login; other providers use API keys. Choose a model.",
    ),
    field(
      "Model",
      model,
      "Discover available models or enter a model id. Blank uses the default for Codex only.",
    ),
    list,
    refresh,
    field("Context ignore patterns", ignores, "One gitignore-style pattern per line. Applies to AI context only; Files still shows excluded files. Project .gitignore rules also apply."),
    button("Reset ignore defaults", () => { ignores.value = state.context_ignore_defaults.join("\n"); }),
    field("Thinking level", thinking, "OpenAI and Codex reasoning effort. Supported levels depend on the selected model; other providers use their default."),
    field("Completion score target", threshold, "Model self-assessment from 0–10, after a passing test. Stops at or above the target. Default: 9. Explicit <done><done> also stops the task."),
    field("Compaction model", compactModel, "Normal mode summarizes context every 3 task requests. Use a model available on the selected provider; default gpt-5.6-luna. Originals are archived in agent/msgs."),
    field("Maximum task requests (excluding compaction)", steps),
    field(
      "Enable mouse, keyboard, and screenshots",
      computer,
      JSON.stringify(state.computer),
    ),
    button(
      "Save settings",
      async () => {
        await act("settings", {
          provider: provider.value,
          model: model.value,
          max_steps: Number(steps.value),
          compact_model: compactModel.value,
          thinking: thinking.value,
          quality_threshold: Number(threshold.value),
          computer_enabled: computer.checked,
          context_ignores: ignores.value.split("\n").map(s => s.trim()).filter(Boolean),
        });
        await poll();
        await scan();
        notice("Settings saved.");
      },
      "primary",
    ),
    el("hr"),
  );
  root.append(el("h3", "", "API connections"));
  for (const p of ["openai", "claude", "gemini"]) {
    const key = input("", "password");
    key.autocomplete = "new-password";
    key.placeholder = state.keys[p]
      ? "Connected · enter a replacement key"
      : "API key";
    const row = el("div", "row");
    row.append(
      key,
      button("Save", async () => {
        await act("key", { provider: p, key: key.value });
        key.value = "";
        await poll();
        notice(`${p} key saved.`);
      }),
    );
    root.append(
      field(
        p,
        row,
        "Saved keys are local files with owner-only permissions. Environment variables take precedence.",
      ),
    );
  }
  root.append(el("hr"), el("h3", "", "OpenAI audio"));
  for (const [name, label, fallback] of [
    ["realtimeModel", "Realtime model", "gpt-realtime"],
    ["speechModel", "Speech model", "gpt-4o-mini-tts"],
    ["transcribeModel", "Transcription model", "gpt-4o-mini-transcribe"],
  ]) {
    const i = input(state.ui?.[name] || fallback);
    i.onchange = guard(async () => {
      state.ui = { ...state.ui, [name]: i.value };
      await act("ui", { ui: state.ui });
    });
    root.append(field(label, i));
  }
  root.append(
    el(
      "p",
      "muted",
      "Voice uses your microphone and OpenAI API key. AI-generated speech plays in the browser. Realtime tool calls use the workspace selected when the call starts.",
    ),
  );
  $("#view").append(root);
}
const LOG_PREVIEW_LIMIT = 500;
const expandedLogs = new Set();
function logMarker(log) {
  return log ? JSON.stringify([log.at, log.kind, log.message]) : "";
}
function noteLogActivity(log) {
  const marker = logMarker(log);
  if (lastLogMarker === null) {
    lastLogMarker = marker;
    return;
  }
  if (!marker || marker === lastLogMarker) return;
  lastLogMarker = marker;
  const nav = $("#logs-nav");
  nav.classList.remove("log-activity-recent");
  nav.classList.add("log-activity-new");
  clearTimeout(logFreshTimer);
  clearTimeout(logRecentTimer);
  logFreshTimer = setTimeout(() => {
    nav.classList.remove("log-activity-new");
    nav.classList.add("log-activity-recent");
  }, 3000);
  logRecentTimer = setTimeout(() => {
    nav.classList.remove("log-activity-new", "log-activity-recent");
  }, 10000);
}
function logText(value) {
  if (typeof value === "string") return value;
  if (value === undefined) return "";
  return JSON.stringify(value);
}
function clipLogPreview(value, limit = LOG_PREVIEW_LIMIT) {
  const text = logText(value);
  return text.length > limit ? text.slice(0, limit) + "…" : text;
}
function logToolPreview(log) {
  const detail = log.details;
  if (!detail) return "";
  const tool = detail.tool || "tool";
  const args = detail.arguments || {};
  if (tool === "python" && typeof args.code === "string")
    return `code: ${clipLogPreview(args.code)}`;
  if (["bash", "shell"].includes(tool) && typeof args.command === "string")
    return `command: ${clipLogPreview(args.command)}`;
  const pointerFields = [
    "action", "x", "y", "to_x", "to_y", "target_x", "target_y",
    "start", "end", "from", "to", "button", "distance", "delta",
    "duration", "duration_ms", "screen_width", "screen_height", "window_id", "pid",
  ];
  const keyboardFields = ["action", "key", "text", "window_id", "pid"];
  const commonFields = [
    "url", "app", "path", "capture_path", "terminal_id", "wait_ms", "offset",
    "limit", "width", "height", "new_instance", "mode", "show_pointer",
  ];
  const wanted = tool === "virtual_pointer"
    ? pointerFields
    : tool === "virtual_keyboard"
      ? keyboardFields
      : tool === "computer"
        ? [...pointerFields, ...keyboardFields, ...commonFields]
        : commonFields;
  const parts = [];
  const seen = new Set();
  for (const field of wanted) {
    if (seen.has(field) || args[field] === undefined) continue;
    seen.add(field);
    parts.push(`${field}=${clipLogPreview(args[field], 120)}`);
  }
  if (Array.isArray(args.path)) parts.push(`path_points=${args.path.length}`);
  if (!parts.length) return clipLogPreview(JSON.stringify(args, null, 2));
  return parts.join(" · ");
}
function logEntryKey(log) {
  return `${log.at}|${log.kind}|${log.message}`;
}
function renderLogEntry(log) {
  const detail = log.details;
  const head = el("span", "log-head");
  head.append(
    el("time", "log-time", new Date(log.at).toLocaleTimeString()),
    el("span", "log-kind", log.kind),
    el("span", "log-message", log.message),
  );
  if (!detail) {
    const row = el("div", "log-entry log-entry-static");
    row.append(head);
    return row;
  }
  const key = logEntryKey(log);
  const row = el("details", "log-entry");
  const summary = el("summary", "log-summary");
  summary.append(head);
  const preview = logToolPreview(log);
  if (preview) summary.append(el("code", "log-preview", preview));
  row.append(summary, el("pre", "log-full", JSON.stringify(detail, null, 2)));
  row.open = expandedLogs.has(key);
  row.addEventListener("toggle", () => {
    if (row.open) expandedLogs.add(key);
    else expandedLogs.delete(key);
  });
  return row;
}
function renderLogs(force = false) {
  const v = $("#view");
  let logs = v.querySelector(".logs");
  if (logs && !force && Date.now() < logPauseUntil) return;
  if (!logs) {
    v.replaceChildren(toolbar("Backend logs", button("Refresh", async () => {
      logPauseUntil = 0;
      await poll();
      renderLogs(true);
    })));
    logs = el("div", "logs");
    v.append(logs);
  }
  const entries = state.logs || [];
  const latest = entries.at(-1);
  const stamp = JSON.stringify([entries.length, latest?.at, latest?.kind, latest?.message, latest?.details]);
  if (!force && logs.dataset.stamp === stamp) return;
  const oldHeight = v.scrollHeight, oldTop = v.scrollTop;
  logs.replaceChildren(...(entries.length
    ? entries.slice().reverse().map(renderLogEntry)
    : [el("p", "muted", "No backend events yet.")]));
  logs.dataset.stamp = stamp;
  if (oldTop > 0 && v.scrollHeight > oldHeight)
    v.scrollTop = oldTop + (v.scrollHeight - oldHeight);
}
function pauseLogUpdates() {
  const active = ui().tabs.find(tab => tab.id === ui().active);
  if (active?.kind !== "logs") return;
  logPauseUntil = Date.now() + 2000;
  clearTimeout(logResumeTimer);
  logResumeTimer = setTimeout(() => {
    logPauseUntil = 0;
    const current = ui().tabs.find(tab => tab.id === ui().active);
    if (current?.kind === "logs") renderLogs(true);
  }, 2000);
}
function renderFiles() {
  if ($("#view .file-browser")) return;
  $("#view").replaceChildren(createFileBrowser({workspaceId:selected, root:workspace().path}));
}
async function previewFile(workspaceId, path, kind) {
  document.querySelector(".file-preview-popup")?.close();
  const popup = el("dialog", "file-preview-popup");
  const body = el("div", "file-preview-body", "Loading…");
  let url;
  popup.append(toolbar(path, button("Open in tab", async () => {
    popup.close();
    await openTab("file", path.split("/").at(-1), {id:"file:"+path, path, kindOfFile:kind});
  }), button("Close", () => popup.close())), body);
  popup.onclose = () => { if (url) URL.revokeObjectURL(url); popup.remove(); };
  document.body.append(popup);
  popup.onkeydown = event => { if (event.key === "Escape") { event.preventDefault(); popup.close(); } };
  popup.show();
  try {
    const response = await api("/api/media/" + workspaceId, {path}, true);
    const mime = response.headers.get("content-type") || "";
    if (!popup.isConnected) return;
    if (/^(image|audio|video)\//.test(mime)) {
      const blob = await response.blob();
      if (!popup.isConnected) return;
      url = URL.createObjectURL(blob);
      const node = el(mime.startsWith("image/") ? "img" : mime.startsWith("audio/") ? "audio" : "video", "preview");
      node.src = url; node.controls = true; node.alt = path; body.replaceChildren(node);
    } else if (kind === "binary") body.textContent = "Binary file. Preview is not available.";
    else {
      const text = await response.text();
      if (popup.isConnected) body.replaceChildren(el("pre", "", text));
    }
  } catch (error) { if (popup.isConnected) body.textContent = error.message; }
}
async function renderFile(t) {
  const w = selected,
    key = viewKey;
  const v = $("#view");
  v.append(toolbar(t.path));
  const r = await api("/api/media/" + w, { path: t.path }, true);
  if (key !== viewKey) return;
  const mime = r.headers.get("content-type") || "";
  if (
    mime.startsWith("image/") ||
    mime.startsWith("audio/") ||
    mime.startsWith("video/")
  ) {
    const url = URL.createObjectURL(await r.blob());
    blobUrls.push(url);
    const tag = mime.startsWith("image/")
      ? "img"
      : mime.startsWith("audio/")
        ? "audio"
        : "video";
    const node = el(tag, "preview");
    node.src = url;
    if (tag !== "img") node.controls = true;
    else node.alt = t.path;
    v.append(node);
  } else if (t.kindOfFile === "binary") {
    v.append(el("p", "muted", "Binary file. Preview is not available."));
  } else {
    const editor = el("textarea", "editor");
    editor.setAttribute("aria-label", t.path);
    editor.spellcheck = false;
    editor.value = await r.text();
    v.firstChild.append(
      button(
        "Save file",
        async () => {
          await act("tool", {
            workspace: w,
            name: "write_file",
            arguments: { path: t.path, text: editor.value },
          });
          notice("File saved.");
          await scan();
        },
        "primary",
      ),
    );
    v.append(editor);
  }
}
function renderWeb(t) {
  const v = $("#view");
  const link = el("a", "", "Open in browser ↗");
  link.href = t.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  v.append(toolbar(t.url, link));
  const frame = el("iframe", "webframe");
  frame.src = t.url;
  frame.title = t.url;
  frame.setAttribute("sandbox", "allow-scripts allow-forms allow-popups");
  frame.referrerPolicy = "no-referrer";
  v.append(
    el(
      "p",
      "muted",
      "Some websites block embedding. Use “Open in browser” if the page is unavailable.",
    ),
    frame,
  );
}
async function uploadAudio() {
  const picker = el("input");
  picker.type = "file";
  picker.accept = "audio/*";
  picker.onchange = guard(async () => {
    const file = picker.files[0];
    if (!file) return;
    notice("Transcribing audio…");
    const data = await new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result.split(",")[1]);
      r.onerror = reject;
      r.readAsDataURL(file);
    });
    const result = await api("/api/audio/transcribe", {
      filename: file.name,
      data,
      model: state.ui?.transcribeModel || "gpt-4o-mini-transcribe",
    });
    const text = $(".composer textarea");
    if (text) {
      text.value += (text.value ? "\n" : "") + result.text;
      await saveDraft(selected, text.value);
    }
    notice("Transcription ready.");
  });
  picker.click();
}
async function speak(text) {
  notice("Generating AI speech…");
  const r = await api(
    "/api/audio/speech",
    { text, model: state.ui?.speechModel || "gpt-4o-mini-tts" },
    true,
  );
  const url = URL.createObjectURL(await r.blob());
  blobUrls.push(url);
  const a = el("audio", "audio-result");
  a.src = url;
  a.controls = true;
  $("#view").append(a);
  await a.play();
}
function endVoice() {
  peer?.close();
  stream?.getTracks().forEach((t) => t.stop());
  peer = null;
  stream = null;
  channel = null;
  $("#voice-audio")?.remove();
  voiceWorkspace = null;
  notice("Voice session ended.");
}
async function toggleVoice() {
  if (peer) {
    endVoice();
    return;
  }
  if (!selected) return notice("Open a workspace first.");
  voiceWorkspace = selected;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    peer = new RTCPeerConnection();
    const audio = el("audio");
    audio.id = "voice-audio";
    audio.autoplay = true;
    document.body.append(audio);
    peer.ontrack = (e) => (audio.srcObject = e.streams[0]);
    for (const track of stream.getTracks()) peer.addTrack(track, stream);
    channel = peer.createDataChannel("oai-events");
    channel.onmessage = guard(async (event) => {
      const m = JSON.parse(event.data);
      if (m.type === "error") notice(m.error?.message || "Realtime error");
      if (m.type === "response.function_call_arguments.done") {
        let result;
        try {
          result = await act("tool", {
            workspace: voiceWorkspace,
            name: m.name,
            arguments: JSON.parse(m.arguments),
          });
        } catch (e) {
          result = { error: e.message };
        }
        if (result.image) {
          channel.send(
            JSON.stringify({
              type: "conversation.item.create",
              item: {
                type: "message",
                role: "user",
                content: [
                  {
                    type: "input_image",
                    image_url: `data:${result.image.mime};base64,${result.image.data}`,
                  },
                ],
              },
            }),
          );
          delete result.image;
        }
        channel.send(
          JSON.stringify({
            type: "conversation.item.create",
            item: {
              type: "function_call_output",
              call_id: m.call_id,
              output: JSON.stringify(result),
            },
          }),
        );
        channel.send(JSON.stringify({ type: "response.create" }));
      }
      if (m.type === "response.output_audio_transcript.done")
        notice(m.transcript);
    });
    peer.onconnectionstatechange = () => {
      if (peer?.connectionState === "failed") endVoice();
    };
    const offer = await peer.createOffer();
    await peer.setLocalDescription(offer);
    const r = await api(
      "/api/audio/realtime",
      { sdp: offer.sdp, model: state.ui?.realtimeModel || "gpt-realtime" },
      true,
    );
    await peer.setRemoteDescription({ type: "answer", sdp: await r.text() });
    notice("Voice connected. Microphone is active. Click Voice again to end.");
  } catch (e) {
    endVoice();
    throw e;
  }
}
async function poll() {
  if (polling || !uiActive() || $("#login-dialog").open) return;
  polling = true;
  try {
    const terminalView = state && ["terminals", "managed"].includes(ui().tabs.find(t=>t.id===ui().active)?.kind);
    const incoming = await api("/api/state?summary=true" + (terminalView ? "&terminal=true" : ""));
    if (typeof noteLogActivity === "function")
      noteLogActivity(incoming.latest_log || incoming.logs?.at(-1));
    if (incoming.partial) {
      incoming.workspaces = incoming.workspaces.map(w=>({...state.workspaces.find(old=>old.id===w.id),...w}));
      state = retainLocalTabs({...state,...incoming});
    } else state = retainLocalTabs(incoming);
    state.workspaces = state.workspaces.filter(w => !w.closed);
    if (!selected || !state.workspaces.some((w) => w.id === selected))
      selected = state.workspaces.find(w => w.id === state.ui?.selected)?.id || state.workspaces[0]?.id || null;
    if (selected && !startupAgentOpened) {
      startupAgentOpened = true;
      await openTab("chat", "Agent", {id: "agent-" + crypto.randomUUID()});
    }
    if (!uiActive()) return;
    $("#connection").textContent = "●";
    $("#connection").title = "Backend connected";
    if (!viewKey.startsWith("standalone/")) render();
    refreshUsage();
  } catch (e) {
    $("#connection").textContent = "○";
    $("#connection").title = "Backend disconnected";
    if (!state) notice(e);
  } finally {
    polling = false;
  }
}
$("#add-workspace").onclick = guard(async () => {
  const home = await act("browse", {});
  const dialog = el("dialog", "folder-picker");
  dialog.append(createFileBrowser({root:home.path, picker:true, onChoose:async path => {
    const w = await act("workspace_open", {path});
    dialog.close(); await poll(); await select(w.id);
  }}));
  dialog.append(button("Cancel", () => dialog.close()));
  dialog.onclose = () => dialog.remove();
  document.body.append(dialog); dialog.showModal();
});
$("#settings-nav").onclick = guard(() => openTab("settings", "Settings"));
$("#logs-nav").onclick = guard(async () => { await openTab("logs", "Logs"); await poll(); });
$("#view").addEventListener("scroll", pauseLogUpdates, {passive:true});
async function newWebview() {
  const value = await ask("New webview", "Website URL", "https://");
  if (!value) return;
  const url = new URL(value);
  if (!["http:", "https:"].includes(url.protocol))
    throw Error("Use an HTTP or HTTPS URL");
  await openTab("web", url.hostname, {
    id: crypto.randomUUID(),
    url: url.href,
  });
}
$("#context").onclick = guard(async () => { await openTab("context", "Context"); await scan(); });
document.addEventListener("pointerdown", e => { if (!$("#menu").contains(e.target)) $("#menu").hidden = true; });
document.addEventListener("keydown", e => { if (e.key === "Escape") $("#menu").hidden = true; });
window.addEventListener("beforeunload", () => {
  peer?.close();
  stream?.getTracks().forEach((t) => t.stop());
});
(async () => {
  await poll();
  if (selected) await guard(select)(selected);
  setInterval(poll, 1000);
  setInterval(() => {
    if (selected && uiActive() && ["files", "context"].includes(ui().tabs.find(t=>t.id===ui().active)?.kind)) scan().catch(() => {});
  }, 15000);
})();

function iconButton(symbol, label, run) {
  const b = button(symbol, run, "icon-button"); b.title = label; b.setAttribute("aria-label", label); return b;
}
function mountTerminal(host, id, workspaceId) {
  const viewport=el("div","vt-viewport"), screen=el("pre","vt-screen"), cursor=el("i","vt-cursor"), input=el("textarea","vt-input");
  input.setAttribute("aria-label","Terminal input"); input.autocapitalize="off"; input.autocomplete="off"; input.spellcheck=false;
  viewport.append(screen,cursor); host.append(viewport,input);
  const keys=el("div","vt-keys"); host.append(keys);
  let revision, modes={}, disposed=false, busy=false, writing=false, pendingInput="", scrollback=0, ctrl=false, copyTimer, resizeTimer;
  let viewGeneration=0, timer, requestController;
  const rowStamps=[];
  let selectedScreen;
  const scroll=delta=>{scrollback=Math.max(0,Math.min(1000,scrollback+delta));revision=undefined;viewGeneration++;requestController?.abort();schedule(0);};
  const send=text=>{
    if (scrollback) { viewGeneration++; scrollback=0; revision=undefined; }
    pendingInput+=text;

    void flushInput();
  };
  async function flushInput() {
    if (writing) return;
    writing=true;
    try {
      while (pendingInput) {
        const text=pendingInput; pendingInput="";
        await act("terminal_input",{workspace:workspaceId,terminal_id:id,text});
        schedule(0);
      }
    } catch(e) { pendingInput=""; notice(e); }
    finally { writing=false; }
  }
  for(const [label,value] of [["Esc","\x1b"],["Tab","\t"],["Ctrl",null],["↑","\x1b[A"],["↓","\x1b[B"],["←","\x1b[D"],["→","\x1b[C"]]) keys.append(iconButton(label,label,()=>{if(value)send(value);else {ctrl=!ctrl;keys.classList.toggle("ctrl-active",ctrl);} input.focus();}));
  keys.append(iconButton("Copy","Copy terminal selection",()=>navigator.clipboard.writeText(getSelection()?.toString()||screen.innerText).catch(notice)), iconButton("⇞","Scroll terminal up",()=>{scroll(10);}),iconButton("⇟","Scroll terminal down",()=>{scroll(-10);}));
  const palette=["#17221d","#df6c75","#99c794","#fac863","#6699cc","#c594c5","#5fb3b3","#d8dee9","#65737e","#ec5f67","#99c794","#fac863","#6699cc","#c594c5","#5fb3b3","#ffffff"];
  function color(value,fallback){
    if(value==="Default")return fallback;
    const rgb=value.match(/Rgb\((\d+), (\d+), (\d+)\)/); if(rgb)return `rgb(${rgb[1]},${rgb[2]},${rgb[3]})`;
    const n=Number(value.match(/Idx\((\d+)\)/)?.[1]); if(n<16)return palette[n];
    if(n>=232)return `rgb(${Array(3).fill(8+(n-232)*10).join(",")})`;
    const k=n-16, level=x=>x?55+x*40:0;return `rgb(${level(Math.floor(k/36))},${level(Math.floor(k/6)%6)},${level(k%6)})`;
  }
  function render(data){
    modes=data; input.disabled=data.exited;
    const selection=getSelection(); if(selection?.toString() && screen.contains(selection.anchorNode)){selectedScreen=data;revision=data.revision;return;}
    selectedScreen=null;
    data.rows.forEach((cells,i)=>{
      const stamp=JSON.stringify(cells);
      if(rowStamps[i]===stamp)return;
      rowStamps[i]=stamp;
      const row=el("div","vt-row");let span,last,run="";
      const flush=()=>{if(span)span.textContent=run;run="";};
      for(const [text,fg,bg,bold,italic,underline,inverse] of cells){
        const key=JSON.stringify([fg,bg,bold,italic,underline,inverse]);
        if(key!==last){flush();span=el("span");span.style.color=color(inverse?bg:fg,inverse?"#17221d":"#e0ebe5");span.style.backgroundColor=color(inverse?fg:bg,inverse?"#e0ebe5":"transparent");span.style.fontWeight=bold?"bold":"normal";span.style.fontStyle=italic?"italic":"normal";span.style.textDecoration=underline?"underline":"none";row.append(span);last=key;}
        run+=text;
      }
      flush();
      screen.children[i]?screen.children[i].replaceWith(row):screen.append(row);
    });
    while(screen.children.length>data.rows.length)screen.lastChild.remove();
    rowStamps.length=data.rows.length;
    cursor.style.left=`${data.cursor[1]}ch`;cursor.style.top=`${data.cursor[0]*19}px`;cursor.hidden=data.hide_cursor||data.exited||data.scrollback>0; revision=data.revision;
  }
  function schedule(delay=50) {
    clearTimeout(timer);
    if(!disposed)timer=setTimeout(tick,delay);
  }
  async function tick(){
    if(!host.isConnected){cleanup();return;}
    if(disposed)return;
    if(busy)return;
    if(!surfaceActive(host))return;
    busy=true;
    const generation=viewGeneration;
    requestController=new AbortController();
    let delay=33;
    try{
      const data=await act("terminal_screen",{terminal_id:id,revision,scrollback,compact:true,wait_ms:25000},requestController.signal);
      if(!disposed && surfaceActive(host) && generation===viewGeneration){
        input.disabled=data.exited;
        if(!data.unchanged){scrollback=data.scrollback;render(data);}
        if(data.exited){cursor.hidden=true;delay=undefined;}
      }
    }catch(e){if(e.name!=="AbortError"){notice(e);delay=2000;}}
    finally{busy=false;requestController=null;if(delay!==undefined && surfaceActive(host))schedule(generation!==viewGeneration?0:delay);}
  }
  input.onkeydown=e=>{
    if(e.isComposing||e.metaKey||(e.ctrlKey&&e.shiftKey))return;
    const map={Enter:"\r",Backspace:"\x7f",Tab:e.shiftKey?"\x1b[Z":"\t",Escape:"\x1b",ArrowUp:"A",ArrowDown:"B",ArrowRight:"C",ArrowLeft:"D",Home:"\x1b[H",End:"\x1b[F",Delete:"\x1b[3~",Insert:"\x1b[2~",PageUp:"\x1b[5~",PageDown:"\x1b[6~",F1:"\x1bOP",F2:"\x1bOQ",F3:"\x1bOR",F4:"\x1bOS",F5:"\x1b[15~",F6:"\x1b[17~",F7:"\x1b[18~",F8:"\x1b[19~",F9:"\x1b[20~",F10:"\x1b[21~",F11:"\x1b[23~",F12:"\x1b[24~"};
    let text=map[e.key];if(e.key.startsWith("Arrow"))text="\x1b"+(modes.application_cursor?"O":"[")+text;
    if((e.ctrlKey||ctrl)&&e.key.length===1)text=String.fromCharCode(e.key.toUpperCase().charCodeAt(0)&31);
    else if(e.altKey&&e.key.length===1)text="\x1b"+e.key;
    if(text){e.preventDefault();send(text);ctrl=false;keys.classList.remove("ctrl-active");}
  };
  input.oninput=e=>{if(e.isComposing)return;if(input.value)send(ctrl?String.fromCharCode(input.value.toUpperCase().charCodeAt(0)&31):input.value);else if(e.inputType==="deleteContentBackward")send("\x7f");input.value="";ctrl=false;};
  input.oncompositionend=()=>{if(input.value)send(input.value);input.value="";};
  input.onpaste=e=>{e.preventDefault();let text=e.clipboardData.getData("text");send(modes.bracketed_paste?"\x1b[200~"+text+"\x1b[201~":text);};
  viewport.onclick=()=>{if(!getSelection()?.toString())input.focus({preventScroll:true});};
  viewport.addEventListener("wheel",e=>{if(!e.deltaY)return;e.preventDefault();scroll(e.deltaY<0?3:-3);},{passive:false});
  function selectionChanged(){if(selectedScreen && !getSelection()?.toString() && surfaceActive(host))render(selectedScreen);clearTimeout(copyTimer);const sel=getSelection(),text=sel?.toString();if(text?.length>4&&screen.contains(sel.anchorNode)&&screen.contains(sel.focusNode))copyTimer=setTimeout(async()=>{if(getSelection()?.toString()!==text)return;try{await navigator.clipboard.writeText(text);host.dataset.copy="Copied";}catch{host.dataset.copy="Use Copy to copy selection";}},3000);}
  document.addEventListener("selectionchange",selectionChanged);
  let lastSize="";
  const resize=()=>{if(!surfaceActive(host))return;const cols=Math.max(10,Math.min(500,Math.floor(viewport.clientWidth/7.83))),rows=Math.max(2,Math.min(200,Math.floor(viewport.clientHeight/19)));const height=Math.round(host.closest(".terminal")?.getBoundingClientRect().height||300),size=`${rows}/${cols}/${height}`;if(size===lastSize)return;lastSize=size;revision=undefined;viewGeneration++;requestController?.abort();act("terminal_resize",{terminal_id:id,rows,cols,height}).catch(notice);};
  const observer=new ResizeObserver(()=>{clearTimeout(resizeTimer);resizeTimer=setTimeout(resize,100);});observer.observe(viewport);
  function cleanup(){if(disposed)return;disposed=true;requestController?.abort();clearTimeout(timer);clearTimeout(copyTimer);clearTimeout(resizeTimer);observer.disconnect();document.removeEventListener("selectionchange",selectionChanged);document.removeEventListener("lessagent:resume",resume);window.removeEventListener("blur",suspend);document.removeEventListener("visibilitychange",suspend);}
  const suspend=()=>{if(!surfaceActive(host)){clearTimeout(timer);requestController?.abort();}};
  window.addEventListener("blur",suspend);
  document.addEventListener("visibilitychange",suspend);
  const resume=()=>{resize();schedule(0);};
  document.addEventListener("lessagent:resume",resume);
  host.terminalCleanup=cleanup;
  schedule(0);return cleanup;
}

function createFileBrowser(options) {
  const {workspaceId, picker=false, context=false, onChoose} = options;
  const root = el("div", "file-browser"); if (context) root.dataset.context = "true";
  let cwd = options.root, inv = null, busy = false, disposed = false, terminalId = null, terminalVisible=false, dockCleanup=null;
  const expanded = new Set(), cache = new Map(), seen = new Map(), fresh = new Map();
  const pathLabel = el(picker ? "input" : "span", "browser-path");
  const suggestions = el("div", "path-suggestions"); suggestions.hidden=true;
  let suggestGeneration=0, suggestTimer, showHidden=false;
  if (picker) {
    pathLabel.type="text"; pathLabel.value=cwd; pathLabel.placeholder="Type a folder path…";
    pathLabel.setAttribute("aria-label", "Folder path"); pathLabel.autocomplete="off";
    const choosePath = async path => {
      ++suggestGeneration; clearTimeout(suggestTimer); suggestions.hidden=true;
      await navigate(path.startsWith("/") || path.startsWith("~") ? path : cwd.replace(/\/$/,"")+"/"+path); pathLabel.value=cwd;
    };
    pathLabel.oninput=() => {
      const generation=++suggestGeneration; clearTimeout(suggestTimer);
      suggestTimer=setTimeout(async () => {
        const value=pathLabel.value.trim();
        const slash=value.lastIndexOf("/");
        const parent=slash<0 ? cwd : value.slice(0,slash)||"/";
        const query=(slash<0?value:value.slice(slash+1)).toLowerCase();
        try {
          const data=await act("browse",{path:parent});
          if (generation!==suggestGeneration || !root.isConnected) return;
          const score=name => {
            name=name.toLowerCase(); let at=0, total=0;
            for(const c of query){const i=name.indexOf(c,at);if(i<0)return Infinity;total+=i-at;at=i+1;}
            return total+(name.startsWith(query)?0:100);
          };
          const matches=data.entries.filter(f=>f.directory && (showHidden || query.startsWith(".") || !f.name.startsWith(".")))
            .map(f=>({...f,score:score(f.name)})).filter(f=>Number.isFinite(f.score))
            .sort((a,b)=>a.score-b.score || a.name.localeCompare(b.name)).slice(0,30);
          suggestions.replaceChildren(...matches.map(f=>button(f.path,()=>choosePath(f.path))));
          if(!matches.length) suggestions.append(el("p","muted","No matching folders"));
          suggestions.hidden=false;
        } catch(e) {if(generation===suggestGeneration){suggestions.replaceChildren(el("p","muted",e.message));suggestions.hidden=false;}}
      },150);
    };
    pathLabel.onkeydown=guard(async e=>{
      if(e.key==="Enter"){e.preventDefault();await choosePath(pathLabel.value.trim());}
      if(e.key==="Escape"){e.preventDefault();e.stopPropagation();++suggestGeneration;clearTimeout(suggestTimer);suggestions.hidden=true;}
      if(e.key==="ArrowDown"){e.preventDefault();suggestions.querySelector("button")?.focus();}
    });
  }
  const rows = el("div","tree-rows");
  const summary = el("p","muted context-summary");
  const dock = el("div","file-terminal-dock"); dock.hidden=true;
  const selectedPath = () => cwd.slice(options.root.length).replace(/^\//,"") || ".";
  const bar = el("div","browser-toolbar");
  bar.append(iconButton("↑","Parent folder", () => navigate(cwd.slice(0,cwd.lastIndexOf("/")) || "/")), pathLabel,
    iconButton("↻","Refresh files", () => refresh(true)));
  if (!picker) bar.append(iconButton(">_","Toggle folder terminal", async () => {
    terminalVisible=!terminalVisible; dock.hidden=!terminalVisible;
    if (terminalVisible) await syncTerminal();
    else dockCleanup?.();
  }));
  if (picker) bar.append(button("Open this folder", () => onChoose(cwd), "primary"));
  if (picker) {
    const toggle=button("Show hidden folders",()=>{showHidden=!showHidden;toggle.textContent=showHidden?"Hide hidden folders":"Show hidden folders";draw();});
    bar.append(toggle);
  }
  root.append(bar,suggestions,summary,rows,dock);
  function relative(path) { return path.slice(options.root.length).replace(/^\//,""); }
  function stats(path, directory) {
    if (!inv) return null;
    const rel = relative(path);
    const files = inv.files.filter(f => directory ? (!rel || f.path.startsWith(rel+"/")) : f.path === rel);
    return files.length ? {bytes:files.reduce((n,f)=>n+f.bytes,0), tokens:files.reduce((n,f)=>n+f.tokens,0)} : null;
  }
  async function entries(path) {
    return (await act("browse", {path,...(workspaceId?{workspace:workspaceId}:{})})).entries;
  }
  async function load(path) {
    try {
      const list=await entries(path), previous=seen.get(path), now=Date.now();
      if (previous) for (const f of list) if (!previous.has(f.path)) fresh.set(f.path,now);
      seen.set(path,new Set(list.map(f=>f.path))); cache.set(path,list);
    } catch(e) { cache.set(path,[]); notice(e); }
  }
  async function navigate(path) {
    if (!picker && path !== options.root && !path.startsWith(options.root+"/")) return;
    if (!context) path=(await act("browse",{path,...(workspaceId?{workspace:workspaceId}:{})})).path;
    cwd=path; expanded.clear(); await load(cwd); if(!disposed) draw();
    if (terminalVisible) await syncTerminal();
  }
  function draw() {
    if(picker) {if(document.activeElement!==pathLabel) pathLabel.value=cwd;} else pathLabel.textContent=cwd; pathLabel.title=cwd;
    summary.hidden=!context;
    if (context && inv) summary.textContent = `${formatTokens(inv.total_tokens)} · text ${formatTokens(inv.text_tokens)} · images ${formatTokens(inv.image_tokens)} · image estimate $${(inv.image_cost_usd || 0).toFixed(5)} at $5/1M · largest first · gray items are not included`;
    const fragment=document.createDocumentFragment();
    function branch(path,depth) {
      const list=(cache.get(path)||[]).filter(f=>!picker || (f.directory && (showHidden || !f.name.startsWith("."))));
      const output = /^(agent\/)?output(\/|$)/.test(relative(path));
      const newest = f => f.modified_ms ?? f.created_ms ?? 0;
      list.sort((a,b) => context ? (stats(b.path,b.directory)?.tokens||0)-(stats(a.path,a.directory)?.tokens||0) || a.name.localeCompare(b.name) :
        output ? newest(b)-newest(a) || (b.created_ms || 0)-(a.created_ms || 0) || a.name.localeCompare(b.name) :
        Number(fresh.has(b.path))-Number(fresh.has(a.path)) || Number(b.directory)-Number(a.directory) || Number(a.name.startsWith("."))-Number(b.name.startsWith(".")) || a.name.localeCompare(b.name));
      for (const f of list) {
        const row=el("div","tree-row"+(fresh.has(f.path)?" new-file":"")); row.style.paddingLeft=depth*18+"px"; row.dataset.path=f.path;
        const open=expanded.has(f.path), label=el("span","tree-name",`${f.directory?(open?"▾":"▸"):f.kind==="image"?"▧":"·"}  ${f.name}`);
        row.title=f.path; row.setAttribute("role","treeitem"); row.tabIndex=0;
        if (f.directory) row.setAttribute("aria-expanded",String(open));
        const st=stats(f.path,f.directory);
        if (context && !st) { row.classList.add("context-excluded"); row.title += " · Not included in context"; }
        const metrics=el("span","tree-metrics",st ? `${formatBytes(st.bytes)} · ${formatTokens(st.tokens)}` : context ? "Not included · 0 tokens" : f.directory ? "—" : `${formatBytes(f.bytes)} · 0.0k tokens`);
        row.append(label,metrics);
        let clickTimer;
        const expand = async () => { if (expanded.has(f.path)) expanded.delete(f.path); else { expanded.add(f.path); await load(f.path); } draw(); };
        const openFile = async () => {
          if (picker) return;
          await openTab("file",f.name,{id:"file:"+relative(f.path),path:relative(f.path),kindOfFile:f.kind || "text"});
        };
        row.onclick=guard(async () => {
          if (f.directory) { clearTimeout(clickTimer); clickTimer=setTimeout(() => expand().catch(notice),230); }
          else { clearTimeout(clickTimer); clickTimer=setTimeout(() => { if (row.isConnected) previewFile(workspaceId, relative(f.path), f.kind || "text").catch(notice); }, 260); }
        });
        row.ondblclick=guard(async () => { clearTimeout(clickTimer); if (f.directory) await navigate(f.path); else { document.querySelector(".file-preview-popup")?.close(); await openFile(); } });
        row.onkeydown=guard(async e => { if (e.key==="Enter") {e.preventDefault(); if (f.directory) await navigate(f.path); else await openFile();} else if (f.directory && ["ArrowRight","ArrowLeft"," "].includes(e.key)) {e.preventDefault(); await expand();} });
        fragment.append(row); if(open && f.directory) branch(f.path,depth+1);
      }
    }
    branch(cwd,0);
    if (!fragment.childNodes.length) fragment.append(el("p","muted","This folder is empty."));
    // Preserve focused row across automatic refreshes.
    const focus=rows.contains(document.activeElement)?document.activeElement.dataset.path:null;
    rows.replaceChildren(fragment); rows.setAttribute("role","tree");
    if(focus) [...rows.children].find(n=>n.dataset.path===focus)?.focus({preventScroll:true});
  }
  async function refresh(rescan=false) {
    if(busy) return; busy=true;
    try {
      if(workspaceId) inv=await api("/api/inventory/"+workspaceId);
      for(const [p,time] of fresh) if(Date.now()-time>30000) fresh.delete(p);
      await load(cwd);
      for(const path of expanded) await load(path);
      if(!disposed && (!root.isConnected || surfaceActive(root))) draw();
    } finally {busy=false;}
  }
  async function syncTerminal(force=false) {
    const t=await act(force?"terminal_new":"terminal_cd",{workspace:workspaceId,path:selectedPath(),terminal_id:terminalId});
    terminalId=t.id; await poll(); renderDock();
  }
  function renderDock() {
    dockCleanup?.(); dock.replaceChildren();
    const choose=el("select"); choose.setAttribute("aria-label","Folder terminal");
    const sessions=state.terminals.filter(t=>t.workspace===workspaceId && (t.cwd===cwd || t.id===terminalId));
    for(const t of sessions){const o=el("option","",`${t.running?"●":"○"} ${t.title} · ${t.id.slice(0,6)}${t.exited?" · exited":""}`);o.value=t.id;choose.append(o);}
    choose.value=terminalId; choose.onchange=()=>{terminalId=choose.value;renderDock();};
    const head=el("div","dock-toolbar"); head.append(choose,iconButton("＋","New folder terminal",()=>syncTerminal(true)),iconButton("×","Hide terminal",()=>{terminalVisible=false;dock.hidden=true;dockCleanup?.();}));
    const surface=el("div","terminal-surface"); dock.append(head,surface); dockCleanup=mountTerminal(surface,terminalId,workspaceId);
  }
  root.refreshTree=()=>refresh(true).catch(notice);
  const resumeBrowser=()=>{if(surfaceActive(root)){draw();refresh().catch(notice);}};
  document.addEventListener("lessagent:resume",resumeBrowser);
  const timer=setInterval(()=>{if(!root.isConnected){disposed=true;clearInterval(timer);dockCleanup?.();document.removeEventListener("lessagent:resume",resumeBrowser);return;} if(surfaceActive(root))refresh().catch(notice);},15000);
  refresh().catch(notice);
  return root;
}

const gitViews = new Map();
async function renderGit() {
  const workspace = selected, root = $("#view"), request = Symbol();
  root.gitRequest = request;
  const valid = () => root.gitRequest === request && selected === workspace && ui().tabs.some(t => t.id === ui().active && t.kind === "git");
  root.replaceChildren(toolbar("Git", button("Refresh", () => renderGit())));
  let data;
  try { data = await act("git_status", {workspace}); }
  catch (e) { if (valid()) root.append(el("p", "empty", e.message)); return; }
  if (!valid()) return;
  const prefs = gitViews.get(workspace) || {tab:"changes", file:null, branch:null}; gitViews.set(workspace,prefs);
  const mutate = async (action, extra = {}) => { await act(action, {workspace, ...extra}); if (valid()) await renderGit(); };
  root.append(el("p", "", `Current branch: ${data.branch}`));
  const tabs = el("div", "row git-subtabs"), content = el("div", "git-content");
  const changesButton = button(`Changes (${data.files.length})`, () => {prefs.tab="changes"; draw();});
  const branchesButton = button(`Branches (${data.branches.length})`, () => {prefs.tab="branches"; draw();});
  tabs.setAttribute("role","tablist");
  for (const b of [changesButton,branchesButton]) b.setAttribute("role","tab");
  tabs.append(changesButton,branchesButton); root.append(tabs,content);
  let selection = 0;
  function diffBlock(title, text) {
    const wrap = el("section", "git-patch"); wrap.append(el("h3", "", title));
    const pre = el("pre", "git-diff");
    for (const line of (text || "No differences").split("\n")) pre.append(el("div", line.startsWith("+") ? "diff-add" : line.startsWith("-") ? "diff-delete" : line.startsWith("@@") ? "diff-hunk" : "",line || " "));
    wrap.append(pre); return wrap;
  }
  function draw() {
    selection++; content.replaceChildren();
    const changes = prefs.tab === "changes";
    changesButton.setAttribute("aria-selected",String(changes)); branchesButton.setAttribute("aria-selected",String(!changes));
    const controls = el("div","row"), panes = el("div","git-panes"), list = el("div","git-list"), detail = el("div","git-detail");
    panes.append(list,detail); content.append(controls,panes);
    if (changes) {
      const discard = button("Discard all changes", async () => {
        if (confirm("Permanently discard ALL staged and unstaged changes and delete untracked files? Ignored files are kept. This cannot be undone.")) await mutate("git_discard",{confirmed:true});
      }); discard.disabled = !data.files.length; controls.append(discard);
      if (!data.files.length) list.append(el("p","muted","Working tree clean"));
      for (const file of data.files) {
        const row = button("", async () => {
          const current = ++selection; prefs.file = file.path;
          list.querySelectorAll("button").forEach(b => b.classList.toggle("chosen",b===row));
          detail.replaceChildren(el("p","muted","Loading diff…"));
          try {
            const d = await act("git_file",{workspace,path:file.path});
            if (!valid() || current !== selection) return;
            detail.replaceChildren(el("h3","",file.path));
            for (const [label,key] of [["Unstaged","unstaged"],["Staged","staged"],["Untracked","untracked"]]) if(d[key]) detail.append(diffBlock(label,d[key]));
            if (!d.unstaged && !d.staged && !d.untracked) detail.append(el("p","muted","No text diff (for example, a mode change)."));
          } catch(e) {if(valid() && current === selection) detail.replaceChildren(el("p","empty",e.message));}
        },"git-file");
        row.title=file.path;
        row.append(el("span","git-filename",`${file.status} ${file.path}`),el("span","git-count",file.binary ? "binary / large" : `+${file.added} −${file.deleted}`));list.append(row);
        if (file.path===prefs.file) row.click();
      }
      if (!prefs.file || !data.files.some(f=>f.path===prefs.file)) detail.append(el("p","muted","Select a file to see changed lines. Counts include staged and unstaged changes."));
    } else {
      const name = input(""); name.placeholder="New branch name"; name.setAttribute("aria-label","New branch name");
      controls.append(name,button("Create branch",()=>mutate("git_create",{branch:name.value})));
      for (const branch of data.branches) {
        const row = button(`${branch.name}${branch.remote ? " · remote" : branch.name === data.branch ? " · current" : ""}`,async () => {
          const current = ++selection; prefs.branch=branch.ref;
          list.querySelectorAll("button").forEach(b=>b.classList.toggle("chosen",b===row));
          detail.replaceChildren(el("h3","",branch.name),el("p","",`${branch.commit} · ${branch.subject}`),el("p","muted",`${branch.date}${branch.upstream ? " · tracks " + branch.upstream : ""}`));
          const switchButton=button(branch.remote ? "Create tracking branch and switch" : "Switch to this branch",()=>mutate("git_switch",{branch:branch.remote ? branch.ref : branch.name}));
          switchButton.disabled=!branch.remote && branch.name===data.branch; detail.append(switchButton);
          try {
            const d=await act("git_branch",{workspace,branch:branch.ref});
            if(!valid() || current!==selection)return;
            detail.append(el("p","muted",`${d.ahead} commits ahead · ${d.behind} behind ${data.branch}`),diffBlock(`Changes from ${data.branch} to ${branch.name}`,d.diff));
          } catch(e) {if(valid() && current===selection) detail.append(el("p","empty",e.message));}
        },"git-branch"); list.append(row); if(branch.ref===prefs.branch)row.click();
      }
      if(!prefs.branch) detail.append(el("p","muted","Select a branch to see its details and compare it with the current branch. Remote branches reflect the last fetch."));
    }
  }
  draw();
}

let usageSnapshot = null, usageFetched = 0, usagePending = false;
function renderAgentStatus() {
  const status = document.querySelector("footer > span:last-child");
  if (!status || !state || !uiActive()) return;
  const active = ui().tabs.find(t => t.id === ui().active);
  const job = state.jobs.filter(j => j.workspace === selected && (active?.kind !== "chat" || (j.session || "chat") === active.id)).at(-1);
  const thinking = ["codex", "openai"].includes(state.settings.provider) ? (job?.status === "running" ? job.thinking : state.settings.thinking) || "medium" : "provider default";
  const model = job?.status === "running" ? job.model || state.settings.model : state.settings.model;
  const providerLabel = document.querySelector("#provider-label");
  if (providerLabel) providerLabel.textContent = `${state.settings.provider} / ${model || "default model"}-${thinking}`;
  status.title = "";
  const parts = [`Score: ${job?.score == null ? "—" : job.score + "/10"}`];
  if (state.settings.provider === "codex") {
    const windows = usageSnapshot?.rate_limit;
    for (const [label, key] of [["5h", "primary_window"], ["Week", "secondary_window"]]) {
      const used = windows?.[key]?.used_percent;
      parts.push(`${label}: ${typeof used === "number" ? Math.max(0, Math.min(100, 100-used)).toFixed(0) + "% left" : "unavailable"}`);
    }
    status.title = usageSnapshot?.error || ["primary_window", "secondary_window"].map(k => {
      const w = windows?.[k]; return w?.reset_at ? `${k === "primary_window" ? "5h" : "Week"} resets ${new Date(w.reset_at*1000).toLocaleString()}` : "";
    }).filter(Boolean).join(" · ");
  }
  status.textContent = parts.join(" · ");
}
async function refreshUsage() {
  if (state.settings.provider !== "codex" || usagePending || Date.now()-usageFetched < 60000) return;
  usagePending = true;
  try { usageSnapshot = await act("codex_usage", {}); }
  catch (e) { usageSnapshot = {error:e.message}; }
  finally { usagePending = false; usageFetched = Date.now(); renderAgentStatus(); }
}
function installThinkingSend(submit, form) {
  let menu = null, timer = null, controller = null, sending = false, levels = ["low", "medium", "high", "xhigh"];
  const provider = state.settings.provider;
  if (provider === "codex") api("/api/models/codex").then(models => {
    const m = models.find(m => m.id === state.settings.model) || models[0];
    const supported = m?.details?.supported_reasoning_levels?.map(x => x.effort || x.reasoning_effort).filter(Boolean);
    if (supported?.length) levels = supported;
  }).catch(() => {});
  else if (provider === "openai") levels = ["none", "low", "medium", "high", "xhigh"];
  const close = () => { clearTimeout(timer); menu?.remove(); menu = null; controller?.abort(); controller = null; };
  const send = guard(async level => {
    if (sending) return;
    close();
    if (!submit.isConnected || submit.disabled || !form.querySelector("textarea").value.trim()) return;
    sending = true;
    submit.disabled = true;
    try {
      if (level && level !== state.settings.thinking) {
        await act("settings", {...state.settings, thinking:level}); state.settings.thinking = level;
      }
      form.requestSubmit();
    } finally { submit.disabled = false; sending = false; }
  });
  function show() {
    close();
    if (!["codex", "openai"].includes(provider)) { send(); return false; }
    menu = el("div", "thinking-menu"); menu.setAttribute("role", "menu");
    for (const level of levels) {
      const b = el("button", level === state.settings.thinking ? "chosen" : "", level);
      b.type = "button"; b.dataset.level = level; b.setAttribute("role", "menuitem");
      b.onpointerdown = e => { e.preventDefault(); clearTimeout(timer); };
      b.onpointerup = e => { e.preventDefault(); e.stopPropagation(); send(level); };
      b.onclick = e => { e.preventDefault(); send(level); }; menu.append(b);
    }
    const cancel = el("button", "", "Cancel send"); cancel.type = "button"; cancel.onclick = close; menu.append(cancel);
    document.body.append(menu);
    const r = submit.getBoundingClientRect();
    menu.style.left = Math.max(8, Math.min(r.right-menu.offsetWidth, innerWidth-menu.offsetWidth-8)) + "px";
    menu.style.top = Math.max(8, r.top-menu.offsetHeight-8) + "px";
    controller = new AbortController();
    document.addEventListener("keydown", e => { if (e.key === "Escape") close(); }, {signal:controller.signal});
    return true;
  }
  submit.onpointerdown = e => {
    if (e.button !== 0 || submit.disabled) return;
    e.preventDefault();
    if (submit.hasPointerCapture?.(e.pointerId)) submit.releasePointerCapture(e.pointerId);
    if (!show()) return;
    const signal = controller.signal;
    document.addEventListener("pointermove", e => {
      const target = document.elementFromPoint(e.clientX,e.clientY)?.closest("[data-level]");
      menu?.querySelectorAll("[data-level]").forEach(b => b.classList.toggle("hover", b === target));
    }, {signal});
    document.addEventListener("pointerup", e => {
      const target = document.elementFromPoint(e.clientX,e.clientY)?.closest("[data-level]");
      if (target && menu?.contains(target)) send(target.dataset.level);
      else if (menu) { timer = setTimeout(() => send(state.settings.thinking),3000); }
    }, {signal, once:true});
    document.addEventListener("pointercancel", close, {signal, once:true});
  };
  submit.onclick = e => { if (e.detail === 0 && show()) timer = setTimeout(() => send(state.settings.thinking),3000); };
}

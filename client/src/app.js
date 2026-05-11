const room = document.body.dataset.room;
const params = new URLSearchParams(location.search);
const initialRole = params.get("role") === "host" ? "host" : "viewer";

const statusEl = document.getElementById("status");
const logEl = document.getElementById("log");
const connectBtn = document.getElementById("connect-btn");
const shareBtn = document.getElementById("share-btn");
const copyBtn = document.getElementById("copy-link");
const setPasswordBtn = document.getElementById("set-password-btn");
const roleInputs = [...document.querySelectorAll('input[name="role"]')];
const remoteVideo = document.getElementById("remote-video");
const localPreview = document.getElementById("local-preview");
const participantsEl = document.getElementById("participants");
const connectionStatsEl = document.getElementById("connection-stats");
const displayNameInput = document.getElementById("display-name");
const joinPasswordInput = document.getElementById("join-password");
const backendUrlInput = document.getElementById("backend-url");
const useTurnInput = document.getElementById("use-turn");

let socket;
let selfId;
let selfDisplayName = "";
let iceServers = [];
let localStream = null;
let hostPeerDetails = [];
const peers = new Map();

const CLIENT_TOKEN_KEY = "rustdesk-share-client-token";
const DISPLAY_NAME_KEY = "rustdesk-share-display-name";
const BACKEND_URL_KEY = "rustdesk-share-backend-url";
const USE_TURN_KEY = "rustdesk-share-use-turn";
const DEFAULT_BACKEND_URL = window.__ROOM_CONFIG__?.VITE_BACKEND_URL || "";
let statsTimer = null;

for (const input of roleInputs) {
  input.checked = input.value === initialRole;
}

displayNameInput.value = localStorage.getItem(DISPLAY_NAME_KEY) || "";
backendUrlInput.value = localStorage.getItem(BACKEND_URL_KEY) || DEFAULT_BACKEND_URL || location.origin;
useTurnInput.checked = localStorage.getItem(USE_TURN_KEY) !== "false";

function log(message) {
  const line = `[${new Date().toLocaleTimeString()}] ${message}`;
  logEl.textContent = `${line}\n${logEl.textContent}`.trim();
  statusEl.textContent = message;
}

function selectedRole() {
  return roleInputs.find((input) => input.checked)?.value || "viewer";
}

function clientToken() {
  let value = localStorage.getItem(CLIENT_TOKEN_KEY);
  if (!value) {
    value = crypto.randomUUID();
    localStorage.setItem(CLIENT_TOKEN_KEY, value);
  }
  return value;
}

function currentDisplayName() {
  const value = displayNameInput.value.trim();
  if (value) {
    localStorage.setItem(DISPLAY_NAME_KEY, value);
  }
  return value;
}

function shouldUseTurn() {
  const enabled = useTurnInput.checked;
  localStorage.setItem(USE_TURN_KEY, enabled ? "true" : "false");
  return enabled;
}

function wsUrl() {
  const base = backendBaseUrl();
  const scheme = base.protocol === "https:" ? "wss" : "ws";
  const query = new URLSearchParams({
    client_token: clientToken(),
    display_name: currentDisplayName(),
  });
  const password = joinPasswordInput.value;
  if (password) {
    query.set("password", password);
  }
  const path = `${base.pathname.replace(/\/$/, "")}/ws/${encodeURIComponent(room)}/${selectedRole()}`;
  return `${scheme}://${base.host}${path}?${query.toString()}`;
}

function viewerLink() {
  return `${location.origin}/room/${encodeURIComponent(room)}?role=viewer`;
}

function backendBaseUrl() {
  const raw = backendUrlInput.value.trim() || DEFAULT_BACKEND_URL || `${location.origin}/backend`;
  const url = new URL(raw, location.origin);
  const normalized = url.toString().replace(/\/$/, "");
  localStorage.setItem(BACKEND_URL_KEY, normalized);
  backendUrlInput.value = normalized;
  return url;
}

function filteredIceServers(servers) {
  if (shouldUseTurn()) {
    return servers;
  }

  return (servers || [])
    .map((server) => ({
      ...server,
      urls: (server.urls || []).filter(
        (url) => !url.startsWith("turn:") && !url.startsWith("turns:")
      ),
    }))
    .filter((server) => server.urls.length > 0);
}

function renderParticipants() {
  const peersList = [...peers.values()].sort((a, b) => a.display_name.localeCompare(b.display_name));
  const isHost = selectedRole() === "host";
  const cards = peersList.map((peer) => {
    const admin = hostPeerDetails.find((item) => item.peer_id === peer.peer_id);
    const kickActions =
      isHost && peer.role === "viewer"
        ? `
        <div class="participant-actions">
          <button data-kick="${peer.peer_id}">Kick</button>
          <button data-kickban="${peer.peer_id}" class="secondary">Kick + token ban</button>
          <button data-kicknet="${peer.peer_id}" class="secondary">Kick + network ban</button>
        </div>
      `
        : "";
    const networkInfo =
      isHost && admin
        ? `<div class="participant-meta">IP: ${admin.remote_addr || "unknown"}</div>
           <div class="participant-meta">FP: ${(admin.network_fingerprints || []).join(", ") || "none"}</div>`
        : "";
    return `
      <article class="participant-card">
        <div class="participant-title">${escapeHtml(peer.display_name)} <span class="participant-role">${peer.role}</span></div>
        <div class="participant-meta">${escapeHtml(peer.peer_id)}</div>
        ${networkInfo}
        ${kickActions}
      </article>
    `;
  });
  participantsEl.innerHTML = cards.join("") || `<p class="participant-empty">No peers yet.</p>`;

  for (const button of participantsEl.querySelectorAll("[data-kick]")) {
    button.onclick = () => kickPeer(button.dataset.kick, false, false);
  }
  for (const button of participantsEl.querySelectorAll("[data-kickban]")) {
    button.onclick = () => kickPeer(button.dataset.kickban, true, false);
  }
  for (const button of participantsEl.querySelectorAll("[data-kicknet]")) {
    button.onclick = () => kickPeer(button.dataset.kicknet, true, true);
  }
}

function renderConnectionStats() {
  const cards = [...peers.values()].map((peer) => {
    const stats = peer.stats;
    if (!stats) {
      return `
        <article class="participant-card">
          <div class="participant-title">${escapeHtml(peer.display_name || peer.peer_id)}</div>
          <div class="participant-meta">Gathering connection stats…</div>
        </article>
      `;
    }

    const mode = stats.relay
      ? `TURN relay via ${escapeHtml(stats.turnServer || "unknown relay")}`
      : "P2P direct";
    const path = stats.relay
      ? `<div class="participant-mono">relay peer ${escapeHtml(stats.remote || "unknown")}</div>`
      : `<div class="participant-mono">local ${escapeHtml(stats.local || "unknown")} -> remote ${escapeHtml(stats.remote || "unknown")}</div>`;

    return `
      <article class="participant-card">
        <div class="participant-title">${escapeHtml(peer.display_name || peer.peer_id)}</div>
        <div class="participant-meta">${mode}</div>
        ${path}
        <div class="participant-meta">Send ${stats.sendRate} | Receive ${stats.receiveRate}</div>
      </article>
    `;
  });

  connectionStatsEl.innerHTML =
    cards.join("") || `<p class="participant-empty">No active peer connections.</p>`;
}

copyBtn.onclick = async () => {
  await navigator.clipboard.writeText(viewerLink());
  log("Viewer link copied.");
};

connectBtn.onclick = () => {
  connect();
};

setPasswordBtn.onclick = () => {
  send({
    type: "set_join_password",
    password: joinPasswordInput.value || null,
  });
};

displayNameInput.onchange = () => {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    return;
  }
  send({
    type: "set_display_name",
    display_name: displayNameInput.value,
  });
};

backendUrlInput.onchange = () => {
  backendBaseUrl();
};

useTurnInput.onchange = () => {
  const enabled = shouldUseTurn();
  iceServers = filteredIceServers(iceServers);
  log(enabled ? "TURN relay candidates enabled." : "TURN relay candidates disabled.");
};

shareBtn.onclick = async () => {
  await startScreenCapture();
  for (const [peerId] of peers) {
    await ensureOffer(peerId);
  }
};

async function connect() {
  if (socket && socket.readyState <= WebSocket.OPEN) {
    socket.close();
  }

  cleanupPeers();
  peers.clear();
  renderParticipants();
  renderConnectionStats();

  socket = new WebSocket(wsUrl());
  socket.onopen = () => log("Connected to signaling server.");
  socket.onclose = () => log("Disconnected.");
  socket.onerror = () => log("WebSocket error.");
  socket.onmessage = async (event) => {
    const message = JSON.parse(event.data);
    await handleServerEvent(message);
  };
}

async function handleServerEvent(message) {
  switch (message.type) {
    case "welcome":
      selfId = message.peer_id;
      selfDisplayName = message.self_display_name;
      displayNameInput.value = selfDisplayName;
      iceServers = filteredIceServers(message.ice_servers || []);
      hostPeerDetails = message.host_peer_details || [];
      shareBtn.disabled = selectedRole() !== "host";
      setPasswordBtn.disabled = selectedRole() !== "host";
      for (const peer of message.peers || []) {
        upsertPeer(peer);
      }
      renderParticipants();
      renderConnectionStats();
      log(`Joined room as ${message.role}. Password protected: ${message.password_protected}.`);
      break;
    case "peer_joined":
      upsertPeer(message.peer);
      renderParticipants();
      log(`Peer joined: ${message.peer.display_name}`);
      if (selectedRole() === "host" && localStream) {
        await ensureOffer(message.peer.peer_id);
      }
      break;
    case "peer_left":
      log(`Peer left: ${message.peer_id}`);
      destroyPeer(message.peer_id);
      peers.delete(message.peer_id);
      renderParticipants();
      renderConnectionStats();
      break;
    case "peer_updated":
      upsertPeer(message.peer);
      renderParticipants();
      break;
    case "host_peer_details":
      hostPeerDetails = message.peers || [];
      renderParticipants();
      break;
    case "room_updated":
      log(`Room updated. Viewer count: ${message.viewer_count}. Password protected: ${message.password_protected}.`);
      break;
    case "signal":
      await handleSignal(message.from, message.signal);
      break;
    case "kicked":
      log(`Removed from room: ${message.reason}`);
      socket?.close();
      break;
    case "error":
      log(`Server error: ${message.message}`);
      break;
    default:
      log(`Unknown event: ${message.type}`);
  }
}

function upsertPeer(peer) {
  if (peer.peer_id === selfId) {
    return;
  }
  const current = peers.get(peer.peer_id) || { pc: null, stream: null, stats: null, statsSnapshot: null };
  peers.set(peer.peer_id, { ...current, ...peer });
}

async function handleSignal(from, signal) {
  ensurePeerEntry(from, "viewer");
  const pc = await ensurePeerConnection(from);

  if (signal.kind === "offer") {
    await pc.setRemoteDescription({ type: "offer", sdp: signal.sdp });
    const answer = await pc.createAnswer();
    await pc.setLocalDescription(answer);
    send({
      type: "answer",
      target: from,
      sdp: answer.sdp,
    });
    log(`Answered offer from ${from}.`);
    return;
  }

  if (signal.kind === "answer") {
    await pc.setRemoteDescription({ type: "answer", sdp: signal.sdp });
    log(`Applied answer from ${from}.`);
    return;
  }

  if (signal.kind === "ice_candidate" && signal.candidate) {
    await pc.addIceCandidate(signal.candidate);
  }
}

function ensurePeerEntry(peerId, fallbackRole) {
  if (!peers.has(peerId)) {
    peers.set(peerId, {
      peer_id: peerId,
      role: fallbackRole,
      display_name: peerId,
      pc: null,
      stream: null,
      audioEl: null,
    });
  }
  return peers.get(peerId);
}

async function ensurePeerConnection(peerId) {
  const entry = ensurePeerEntry(peerId, "viewer");
  if (entry.pc) {
    return entry.pc;
  }

  const pc = new RTCPeerConnection({ iceServers });
  entry.pc = pc;

  pc.onicecandidate = (event) => {
    if (!event.candidate) return;
    send({
      type: "ice_candidate",
      target: peerId,
      candidate: event.candidate,
    });
  };

  pc.onicegatheringstatechange = () => {
    if (pc.iceGatheringState === "complete") {
      reportNetworkFingerprints(pc);
    }
  };

  pc.onconnectionstatechange = () => {
    if (["failed", "closed", "disconnected"].includes(pc.connectionState)) {
      destroyPeer(peerId);
      peers.delete(peerId);
      renderParticipants();
    }
  };

  if (selectedRole() === "host" && localStream) {
    for (const track of localStream.getTracks()) {
      pc.addTrack(track, localStream);
    }
  }

  pc.ontrack = (event) => {
    const track = event.track;
    const [stream] = event.streams;
    if (track.kind === "video") {
      entry.stream = stream;
      remoteVideo.srcObject = stream;
      log(`Receiving screen stream from ${peerId}.`);
      return;
    }

    if (track.kind === "audio") {
      const audioEl = entry.audioEl || new Audio();
      audioEl.autoplay = true;
      audioEl.srcObject = stream;
      entry.audioEl = audioEl;
      log(`Receiving audio stream from ${peerId}.`);
    }
  };

  startStatsPolling();
  return pc;
}

async function ensureOffer(peerId) {
  const pc = await ensurePeerConnection(peerId);
  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
  send({
    type: "offer",
    target: peerId,
    sdp: offer.sdp,
  });
  log(`Sent offer to ${peerId}.`);
}

async function startScreenCapture() {
  if (selectedRole() !== "host") {
    log("Only the host can share a screen.");
    return;
  }

  if (localStream) {
    return localStream;
  }

  localStream = await navigator.mediaDevices.getDisplayMedia({
    video: {
      frameRate: 30,
      cursor: "always",
      displaySurface: "window",
    },
    audio: false,
    preferCurrentTab: false,
    selfBrowserSurface: "exclude",
    surfaceSwitching: "include",
  });

  localPreview.srcObject = localStream;
  log("Screen capture started. Choose an application window if your browser offers it.");

  for (const entry of peers.values()) {
    if (!entry.pc) {
      continue;
    }
    for (const track of localStream.getTracks()) {
      const alreadySending = entry.pc
        .getSenders()
        .some((sender) => sender.track && sender.track.kind === track.kind);
      if (!alreadySending) {
        entry.pc.addTrack(track, localStream);
      }
    }
  }

  const [videoTrack] = localStream.getVideoTracks();
  if (videoTrack) {
    videoTrack.onended = () => {
      log("Screen capture ended.");
      localStream = null;
      localPreview.srcObject = null;
    };
  }

  return localStream;
}

function kickPeer(target, banClientToken, banNetworks) {
  send({
    type: "kick_peer",
    target,
    ban_client_token: banClientToken,
    ban_networks: banNetworks,
  });
}

function reportNetworkFingerprints(pc) {
  const values = new Set();
  for (const line of (pc.localDescription?.sdp || "").split("\n")) {
    if (!line.includes("candidate:")) {
      continue;
    }
    const candidate = line.trim();
    const match = candidate.match(/candidate:[^ ]+ \d+ \w+ \d+ ([^ ]+) (\d+)/);
    if (match) {
      values.add(`cand:${match[1]}:${match[2]}`);
    }
  }
  if (values.size > 0) {
    send({
      type: "report_network_fingerprints",
      fingerprints: [...values],
    });
  }
}

function send(message) {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    throw new Error("websocket is not connected");
  }
  socket.send(JSON.stringify(message));
}

function destroyPeer(peerId) {
  const entry = peers.get(peerId);
  if (!entry) {
    return;
  }
  if (entry.pc) {
    entry.pc.ontrack = null;
    entry.pc.onicecandidate = null;
    entry.pc.onconnectionstatechange = null;
    entry.pc.close();
  }
  if (entry.audioEl) {
    entry.audioEl.srcObject = null;
  }
  if (remoteVideo.srcObject === entry.stream) {
    remoteVideo.srcObject = null;
  }
  renderConnectionStats();
}

function cleanupPeers() {
  for (const peerId of [...peers.keys()]) {
    destroyPeer(peerId);
  }
}

function startStatsPolling() {
  if (statsTimer) {
    return;
  }
  statsTimer = setInterval(pollPeerStats, 2000);
}

async function pollPeerStats() {
  let active = false;
  for (const [peerId, entry] of peers.entries()) {
    if (!entry.pc) {
      continue;
    }
    active = true;
    entry.stats = await collectPeerStats(entry.pc, entry.statsSnapshot);
    entry.statsSnapshot = entry.stats?.snapshot || entry.statsSnapshot;
    peers.set(peerId, entry);
  }

  renderConnectionStats();

  if (!active && statsTimer) {
    clearInterval(statsTimer);
    statsTimer = null;
  }
}

async function collectPeerStats(pc, previousSnapshot) {
  try {
    const report = await pc.getStats();
    const stats = {
      relay: false,
      turnServer: null,
      local: null,
      remote: null,
      sendRate: "0 bps",
      receiveRate: "0 bps",
      snapshot: {
        timestamp: Date.now(),
        outboundBytes: 0,
        inboundBytes: 0,
      },
    };

    let selectedPair = null;
    let outboundBytes = 0;
    let inboundBytes = 0;

    report.forEach((item) => {
      if (
        (item.type === "transport" || item.type === "candidate-pair") &&
        (item.selectedCandidatePairId || item.nominated || item.selected)
      ) {
        if (item.selectedCandidatePairId) {
          selectedPair = report.get(item.selectedCandidatePairId);
        } else if (!selectedPair && (item.nominated || item.selected)) {
          selectedPair = item;
        }
      }

      if (item.type === "outbound-rtp" && !item.isRemote) {
        outboundBytes += item.bytesSent || 0;
      }

      if (item.type === "inbound-rtp" && !item.isRemote) {
        inboundBytes += item.bytesReceived || 0;
      }
    });

    if (selectedPair) {
      const local = report.get(selectedPair.localCandidateId);
      const remote = report.get(selectedPair.remoteCandidateId);
      stats.local = formatCandidateAddress(local);
      stats.remote = formatCandidateAddress(remote);
      stats.relay =
        local?.candidateType === "relay" || remote?.candidateType === "relay";
      stats.turnServer = local?.url || remote?.url || null;
    }

    stats.snapshot.outboundBytes = outboundBytes;
    stats.snapshot.inboundBytes = inboundBytes;

    if (previousSnapshot) {
      const seconds = Math.max(
        1,
        (stats.snapshot.timestamp - previousSnapshot.timestamp) / 1000
      );
      stats.sendRate = formatBitrate(
        ((outboundBytes - previousSnapshot.outboundBytes) * 8) / seconds
      );
      stats.receiveRate = formatBitrate(
        ((inboundBytes - previousSnapshot.inboundBytes) * 8) / seconds
      );
    }

    return stats;
  } catch {
    return {
      relay: false,
      turnServer: null,
      local: "unavailable",
      remote: "unavailable",
      sendRate: "n/a",
      receiveRate: "n/a",
      snapshot: previousSnapshot || {
        timestamp: Date.now(),
        outboundBytes: 0,
        inboundBytes: 0,
      },
    };
  }
}

function formatCandidateAddress(candidate) {
  if (!candidate) {
    return null;
  }
  const ip = candidate.address || candidate.ip || "unknown";
  const port = candidate.port || "unknown";
  return `${ip}:${port}`;
}

function formatBitrate(bitsPerSecond) {
  if (!Number.isFinite(bitsPerSecond) || bitsPerSecond <= 0) {
    return "0 bps";
  }
  if (bitsPerSecond >= 1_000_000) {
    return `${(bitsPerSecond / 1_000_000).toFixed(2)} Mbps`;
  }
  if (bitsPerSecond >= 1_000) {
    return `${(bitsPerSecond / 1_000).toFixed(1)} Kbps`;
  }
  return `${Math.round(bitsPerSecond)} bps`;
}

function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

log(`Viewer link: ${viewerLink()}`);
renderParticipants();
renderConnectionStats();
connect();

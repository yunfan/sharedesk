use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::{mpsc, RwLock};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<RwLock<HashMap<String, RoomState>>>,
}

#[derive(Clone)]
pub struct Participant {
    pub peer_id: String,
    pub role: PeerRole,
    pub client_token: String,
    pub display_name: String,
    pub remote_addr: Option<String>,
    pub network_fingerprints: HashSet<String>,
    pub tx: mpsc::UnboundedSender<ServerEvent>,
}

struct RoomState {
    join_password: Option<String>,
    host: Option<Participant>,
    viewers: HashMap<String, Participant>,
    banned_client_tokens: HashSet<String>,
    banned_network_fingerprints: HashSet<String>,
    updated_at_epoch: u64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PeerRole {
    Host,
    Viewer,
}

#[derive(Debug, Serialize, Clone)]
pub struct ParticipantPublic {
    pub peer_id: String,
    pub role: PeerRole,
    pub display_name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ParticipantAdmin {
    pub peer_id: String,
    pub role: PeerRole,
    pub display_name: String,
    pub remote_addr: Option<String>,
    pub network_fingerprints: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Welcome {
        peer_id: String,
        room: String,
        role: PeerRole,
        host_present: bool,
        viewer_count: usize,
        password_protected: bool,
        self_display_name: String,
        peers: Vec<ParticipantPublic>,
        host_peer_details: Option<Vec<ParticipantAdmin>>,
        ice_servers: Vec<crate::config::IceServer>,
    },
    PeerJoined {
        peer: ParticipantPublic,
    },
    PeerLeft {
        peer_id: String,
        role: PeerRole,
    },
    PeerUpdated {
        peer: ParticipantPublic,
    },
    HostPeerDetails {
        peers: Vec<ParticipantAdmin>,
    },
    RoomUpdated {
        viewer_count: usize,
        password_protected: bool,
    },
    Signal {
        from: String,
        signal: SignalPayload,
    },
    Kicked {
        reason: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalPayload {
    Offer { sdp: String },
    Answer { sdp: String },
    IceCandidate { candidate: serde_json::Value },
}

#[derive(Debug, Serialize)]
pub struct RoomSnapshot {
    pub room: String,
    pub host_present: bool,
    pub viewer_count: usize,
    pub password_protected: bool,
    pub updated_at_epoch: u64,
}

pub struct JoinRequest {
    pub room: String,
    pub role: PeerRole,
    pub peer_id: String,
    pub client_token: String,
    pub display_name: String,
    pub password: Option<String>,
    pub remote_addr: Option<String>,
    pub tx: mpsc::UnboundedSender<ServerEvent>,
}

pub struct JoinResult {
    pub host_present: bool,
    pub viewer_count: usize,
    pub password_protected: bool,
    pub self_display_name: String,
    pub peers: Vec<ParticipantPublic>,
    pub host_peer_details: Option<Vec<ParticipantAdmin>>,
}

pub struct KickRequest {
    pub room: String,
    pub requester_peer_id: String,
    pub target_peer_id: String,
    pub ban_client_token: bool,
    pub ban_networks: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn online_count(&self) -> usize {
        let rooms = self.inner.read().await;
        rooms
            .values()
            .map(|room| room.viewers.len() + usize::from(room.host.is_some()))
            .sum()
    }

    pub async fn join(&self, request: JoinRequest) -> Result<JoinResult, String> {
        let mut rooms = self.inner.write().await;
        let room_state = rooms
            .entry(request.room.clone())
            .or_insert_with(|| RoomState::new(request.password.clone()));

        room_state.updated_at_epoch = unix_now();

        let remote_fp = request
            .remote_addr
            .as_ref()
            .map(|addr| format!("wsip:{addr}"));

        if request.role == PeerRole::Viewer {
            if room_state
                .banned_client_tokens
                .contains(request.client_token.as_str())
            {
                return Err("client token is banned from this room".to_string());
            }

            if remote_fp
                .as_ref()
                .is_some_and(|fp| room_state.banned_network_fingerprints.contains(fp))
            {
                return Err("network is banned from this room".to_string());
            }

            if room_state.join_password != request.password {
                return Err("invalid room password".to_string());
            }
        }

        let mut participant = Participant {
            peer_id: request.peer_id.clone(),
            role: request.role,
            client_token: request.client_token,
            display_name: normalized_display_name(&request.display_name, &request.peer_id),
            remote_addr: request.remote_addr.clone(),
            network_fingerprints: HashSet::new(),
            tx: request.tx,
        };

        if let Some(fp) = remote_fp {
            participant.network_fingerprints.insert(fp);
        }

        match request.role {
            PeerRole::Host => {
                if room_state.host.is_some() {
                    return Err("room already has an active host".to_string());
                }
                if let Some(password) = request.password {
                    room_state.join_password = Some(password);
                }
                room_state.host = Some(participant.clone());
            }
            PeerRole::Viewer => {
                room_state
                    .viewers
                    .insert(request.peer_id.clone(), participant.clone());
            }
        }

        Ok(JoinResult {
            host_present: room_state.host.is_some(),
            viewer_count: room_state.viewers.len(),
            password_protected: room_state.join_password.is_some(),
            self_display_name: participant.display_name,
            peers: room_state.public_peers_except(&request.peer_id),
            host_peer_details: if request.role == PeerRole::Host {
                Some(room_state.admin_view())
            } else {
                None
            },
        })
    }

    pub async fn update_display_name(
        &self,
        room: &str,
        peer_id: &str,
        display_name: String,
    ) -> Result<ParticipantPublic, String> {
        let mut rooms = self.inner.write().await;
        let Some(room_state) = rooms.get_mut(room) else {
            return Err("room no longer exists".to_string());
        };

        let public = {
            let participant = room_state
                .find_participant_mut(peer_id)
                .ok_or_else(|| "participant not found".to_string())?;
            participant.display_name = normalized_display_name(&display_name, peer_id);
            participant.public_view()
        };
        room_state.updated_at_epoch = unix_now();
        room_state.broadcast_except(
            peer_id,
            ServerEvent::PeerUpdated {
                peer: public.clone(),
            },
        );
        room_state.send_host_details();
        Ok(public)
    }

    pub async fn update_network_fingerprints(
        &self,
        room: &str,
        peer_id: &str,
        fingerprints: Vec<String>,
    ) -> Result<(), String> {
        let mut rooms = self.inner.write().await;
        let Some(room_state) = rooms.get_mut(room) else {
            return Err("room no longer exists".to_string());
        };

        let banned_networks = room_state.banned_network_fingerprints.clone();
        let (banned_match, role, tx) = {
            let participant = room_state
                .find_participant_mut(peer_id)
                .ok_or_else(|| "participant not found".to_string())?;

            for fp in fingerprints {
                let value = sanitize_network_fingerprint(&fp);
                if !value.is_empty() {
                    participant.network_fingerprints.insert(value);
                }
            }

            let banned_match = participant
                .network_fingerprints
                .iter()
                .any(|fp| banned_networks.contains(fp));
            (banned_match, participant.role, participant.tx.clone())
        };

        room_state.updated_at_epoch = unix_now();
        room_state.send_host_details();

        if banned_match {
            let _ = tx.send(ServerEvent::Kicked {
                reason: "your network fingerprint is banned in this room".to_string(),
            });
            drop(rooms);
            self.leave(room, role, peer_id).await;
        }

        Ok(())
    }

    pub async fn set_join_password(
        &self,
        room: &str,
        requester_peer_id: &str,
        password: Option<String>,
    ) -> Result<(usize, bool), String> {
        let mut rooms = self.inner.write().await;
        let Some(room_state) = rooms.get_mut(room) else {
            return Err("room no longer exists".to_string());
        };

        if room_state.host.as_ref().map(|p| p.peer_id.as_str()) != Some(requester_peer_id) {
            return Err("only the host can update the room password".to_string());
        }

        room_state.join_password = password.filter(|p| !p.is_empty());
        room_state.updated_at_epoch = unix_now();
        let viewer_count = room_state.viewers.len();
        let password_protected = room_state.join_password.is_some();

        room_state.broadcast_all(ServerEvent::RoomUpdated {
            viewer_count,
            password_protected,
        });

        Ok((viewer_count, password_protected))
    }

    pub async fn kick_peer(&self, request: KickRequest) -> Result<(), String> {
        let mut rooms = self.inner.write().await;
        let Some(room_state) = rooms.get_mut(&request.room) else {
            return Err("room no longer exists".to_string());
        };

        if room_state.host.as_ref().map(|p| p.peer_id.as_str())
            != Some(request.requester_peer_id.as_str())
        {
            return Err("only the host can kick participants".to_string());
        }

        if room_state.host.as_ref().map(|p| p.peer_id.as_str())
            == Some(request.target_peer_id.as_str())
        {
            return Err("host cannot kick itself".to_string());
        }

        let Some(target) = room_state.viewers.remove(&request.target_peer_id) else {
            return Err("target viewer not found".to_string());
        };

        if request.ban_client_token {
            room_state
                .banned_client_tokens
                .insert(target.client_token.clone());
        }
        if request.ban_networks {
            room_state
                .banned_network_fingerprints
                .extend(target.network_fingerprints.iter().cloned());
        }

        room_state.updated_at_epoch = unix_now();
        let _ = target.tx.send(ServerEvent::Kicked {
            reason: "you were removed by the room host".to_string(),
        });
        room_state.broadcast_all(ServerEvent::PeerLeft {
            peer_id: target.peer_id.clone(),
            role: target.role,
        });
        room_state.broadcast_all(ServerEvent::RoomUpdated {
            viewer_count: room_state.viewers.len(),
            password_protected: room_state.join_password.is_some(),
        });
        room_state.send_host_details();
        Ok(())
    }

    pub async fn broadcast_join(&self, room: &str, peer_id: &str) {
        let rooms = self.inner.read().await;
        let Some(room_state) = rooms.get(room) else {
            return;
        };

        let Some(peer) = room_state.find_participant(peer_id) else {
            return;
        };

        room_state.broadcast_except(
            peer_id,
            ServerEvent::PeerJoined {
                peer: peer.public_view(),
            },
        );
        room_state.send_host_details();
        room_state.broadcast_all(ServerEvent::RoomUpdated {
            viewer_count: room_state.viewers.len(),
            password_protected: room_state.join_password.is_some(),
        });
    }

    pub async fn relay(&self, room: &str, target: &str, event: ServerEvent) -> Result<(), String> {
        let rooms = self.inner.read().await;
        let Some(room_state) = rooms.get(room) else {
            return Err("room no longer exists".to_string());
        };

        let participant = room_state
            .find_participant(target)
            .ok_or_else(|| "target peer not found".to_string())?;
        participant
            .tx
            .send(event)
            .map_err(|_| "target peer is disconnected".to_string())
    }

    pub async fn leave(&self, room: &str, role: PeerRole, peer_id: &str) {
        let mut rooms = self.inner.write().await;
        let Some(room_state) = rooms.get_mut(room) else {
            return;
        };

        let removed = match role {
            PeerRole::Host => {
                if room_state.host.as_ref().map(|p| p.peer_id.as_str()) == Some(peer_id) {
                    room_state.host.take()
                } else {
                    None
                }
            }
            PeerRole::Viewer => room_state.viewers.remove(peer_id),
        };

        if removed.is_none() {
            return;
        }

        room_state.updated_at_epoch = unix_now();
        room_state.broadcast_except(
            peer_id,
            ServerEvent::PeerLeft {
                peer_id: peer_id.to_string(),
                role,
            },
        );
        room_state.broadcast_all(ServerEvent::RoomUpdated {
            viewer_count: room_state.viewers.len(),
            password_protected: room_state.join_password.is_some(),
        });
        room_state.send_host_details();

        if room_state.host.is_none() && room_state.viewers.is_empty() {
            rooms.remove(room);
        }
    }

    pub async fn snapshot(&self, room: &str) -> Option<RoomSnapshot> {
        let rooms = self.inner.read().await;
        let room_state = rooms.get(room)?;
        Some(RoomSnapshot {
            room: room.to_string(),
            host_present: room_state.host.is_some(),
            viewer_count: room_state.viewers.len(),
            password_protected: room_state.join_password.is_some(),
            updated_at_epoch: room_state.updated_at_epoch,
        })
    }
}

impl RoomState {
    fn new(join_password: Option<String>) -> Self {
        Self {
            join_password: join_password.filter(|p| !p.is_empty()),
            host: None,
            viewers: HashMap::new(),
            banned_client_tokens: HashSet::new(),
            banned_network_fingerprints: HashSet::new(),
            updated_at_epoch: unix_now(),
        }
    }

    fn find_participant(&self, peer_id: &str) -> Option<&Participant> {
        if self.host.as_ref().map(|p| p.peer_id.as_str()) == Some(peer_id) {
            return self.host.as_ref();
        }
        self.viewers.get(peer_id)
    }

    fn find_participant_mut(&mut self, peer_id: &str) -> Option<&mut Participant> {
        if self.host.as_ref().map(|p| p.peer_id.as_str()) == Some(peer_id) {
            return self.host.as_mut();
        }
        self.viewers.get_mut(peer_id)
    }

    fn public_peers_except(&self, current_peer_id: &str) -> Vec<ParticipantPublic> {
        let mut peers = Vec::new();
        if let Some(host) = &self.host {
            if host.peer_id != current_peer_id {
                peers.push(host.public_view());
            }
        }
        for viewer in self.viewers.values() {
            if viewer.peer_id != current_peer_id {
                peers.push(viewer.public_view());
            }
        }
        peers
    }

    fn admin_view(&self) -> Vec<ParticipantAdmin> {
        let mut peers = Vec::new();
        if let Some(host) = &self.host {
            peers.push(host.admin_view());
        }
        for viewer in self.viewers.values() {
            peers.push(viewer.admin_view());
        }
        peers
    }

    fn broadcast_except(&self, excluded_peer_id: &str, event: ServerEvent) {
        if let Some(host) = &self.host {
            if host.peer_id != excluded_peer_id {
                let _ = host.tx.send(event.clone());
            }
        }
        for viewer in self.viewers.values() {
            if viewer.peer_id != excluded_peer_id {
                let _ = viewer.tx.send(event.clone());
            }
        }
    }

    fn broadcast_all(&self, event: ServerEvent) {
        self.broadcast_except("", event);
    }

    fn send_host_details(&self) {
        let Some(host) = &self.host else {
            return;
        };
        let _ = host.tx.send(ServerEvent::HostPeerDetails {
            peers: self.admin_view(),
        });
    }
}

impl Participant {
    fn public_view(&self) -> ParticipantPublic {
        ParticipantPublic {
            peer_id: self.peer_id.clone(),
            role: self.role,
            display_name: self.display_name.clone(),
        }
    }

    fn admin_view(&self) -> ParticipantAdmin {
        let mut network_fingerprints = self
            .network_fingerprints
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        network_fingerprints.sort();
        ParticipantAdmin {
            peer_id: self.peer_id.clone(),
            role: self.role,
            display_name: self.display_name.clone(),
            remote_addr: self.remote_addr.clone(),
            network_fingerprints,
        }
    }
}

fn normalized_display_name(input: &str, peer_id: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        format!("guest-{}", &peer_id[..8.min(peer_id.len())])
    } else {
        trimmed.chars().take(48).collect()
    }
}

fn sanitize_network_fingerprint(input: &str) -> String {
    input
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | ':' | '-' | '_' | '/'))
        .take(128)
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after epoch")
        .as_secs()
}

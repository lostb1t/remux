use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use tracing::{info, error};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackState {
    pub position_ms: u64,
    pub is_paused: bool,
    pub is_muted: bool,
    pub playback_speed: f32,
    pub audio_stream_index: u64,
    pub subtitle_stream_index: u64,
    pub volume_level: u64,
}

#[derive(Debug, Clone)]
pub struct PlaybackSession {
    pub id: Uuid,
    pub state: PlaybackState,
    pub media: db::Media,
    pub client_name: String,
    pub user_id: Uuid,
   // pub progress_tx: mpsc::Sender<PlaybackProgress>,
}

impl PlaybackSession {
    pub async fn start(&mut self, item: db::Media) {
        self.item = item;
         
       // self.state = PlaybackState::Playing;
        self.progress.is_paused = false;
        info!("Playback started for session: {}", self.id);
    }

    pub async fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.progress.is_paused = false;
        self.progress.position_ticks = 0;
        info!("Playback stopped for session: {}", self.id);
    }

    pub async fn update(&mut self) {
      
    }

    pub async fn resume(&mut self) {
        if self.state == PlaybackState::Paused {
            self.state = PlaybackState::Playing;
            self.progress.is_paused = false;
            info!("Playback resumed for session: {}", self.id);
        }
    }

    pub async fn seek(&mut self, position: u64) {
        self.progress.position_ticks = position;
        info!("Seeked to position {} for session: {}", position, self.id);
    }

    pub async fn set_volume(&mut self, volume: f32) {
        self.progress.is_muted = volume == 0.0;
        info!("Volume set to {} for session: {}", volume, self.id);
    }

    pub async fn report_progress(&mut self, progress: PlaybackProgress) {
        self.progress = progress;
        info!("Progress reported for session: {}", self.id);
    }
}

#[derive(Debug, Default)]
pub struct PlaybackSessionService {
    sessions: Arc<Mutex<Vec<PlaybackSession>>>,
}

impl PlaybackSessionService {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn add_session(&self, session: PlaybackSession) {
        self.sessions.lock().await.push(session);
    }

    pub async fn get_session(&self, session_id: &str) -> Option<PlaybackSession> {
        let sessions = self.sessions.lock().await;
        sessions.iter().find(|s| s.id == session_id).cloned()
    }

    pub async fn remove_session(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if let Some(pos) = sessions.iter().position(|s| s.id == session_id) {
            sessions.remove(pos);
            info!("Removed session: {}", session_id);
            Ok(())
        } else {
            error!("Session not found: {}", session_id);
            Err("Session not found".to_string())
        }
    }
}
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub type JobId = String;

pub enum JobState {
    Pending,
    Running,
    Complete(Vec<u8>), // stores GeoTIFF bytes
    Failed(String),      // stores error message
}

pub struct JobRecord {
    pub state: JobState,
    pub created_at: Instant,
    pub completed_at: Option<Instant>,
}

#[derive(Clone)]
pub struct JobStore {
    inner: Arc<Mutex<HashMap<JobId, JobRecord>>>,
}

impl JobStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn create(&self) -> JobId {
        let id = Uuid::new_v4().to_string();
        let record = JobRecord {
            state: JobState::Pending,
            created_at: Instant::now(),
            completed_at: None,
        };
        self.inner.lock().unwrap().insert(id.clone(), record);
        id
    }

    pub fn set_running(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(record) = inner.get_mut(id) {
            record.state = JobState::Running;
        }
    }

    pub fn complete(&self, id: &str, bytes: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(record) = inner.get_mut(id) {
            record.state = JobState::Complete(bytes);
            record.completed_at = Some(Instant::now());
        }
    }

    pub fn fail(&self, id: &str, msg: String) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(record) = inner.get_mut(id) {
            record.state = JobState::Failed(msg);
            record.completed_at = Some(Instant::now());
        }
    }

    /// Returns JSON-safe status: "pending", "running", "complete", "failed", "not_found"
    /// Also returns optional error string for failed state
    pub fn get_status(&self, id: &str) -> (String, Option<String>) {
        let inner = self.inner.lock().unwrap();
        match inner.get(id) {
            Some(record) => match &record.state {
                JobState::Pending => ("pending".to_string(), None),
                JobState::Running => ("running".to_string(), None),
                JobState::Complete(_) => ("complete".to_string(), None),
                JobState::Failed(msg) => ("failed".to_string(), Some(msg.clone())),
            },
            None => ("not_found".to_string(), None),
        }
    }

    /// Takes the result bytes AND deletes the record — one-shot retrieval
    pub fn take_result(&self, id: &str) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().unwrap();
        match inner.remove(id) {
            Some(record) => match record.state {
                JobState::Complete(bytes) => Some(bytes),
                _ => None,
            },
            None => None,
        }
    }

    /// Cleanup: removes Complete entries older than 15 min, Failed older than 5 min
    pub fn cleanup(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.retain(|_id, record| {
            let age = record.created_at.elapsed();
            match &record.state {
                JobState::Complete(_) => age < Duration::from_secs(15 * 60),
                JobState::Failed(_) => age < Duration::from_secs(5 * 60),
                JobState::Pending | JobState::Running => true,
            }
        });
    }
}

impl Default for JobStore {
    fn default() -> Self {
        Self::new()
    }
}

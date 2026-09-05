use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRound {
    pub id: String,
    #[serde(rename = "gameSlug")]
    pub game_slug: String,
    pub mode: String,
    #[serde(rename = "eventId")]
    pub event_id: u32,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedRoundsFile {
    #[serde(default)]
    rounds: Vec<SavedRound>,
}

fn file_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("could not resolve local data dir"))?
        .join("stake-dev-tool");
    Ok(dir.join("saved-rounds.json"))
}

/// File-backed saved-round collection. Cloud tenants receive distinct stores;
/// desktop callers continue to use the process-wide default store below.
pub struct SavedRoundsStore {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl SavedRoundsStore {
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: Mutex::new(()),
        }
    }

    pub fn default_local() -> Result<Self> {
        Ok(Self::with_path(file_path()?))
    }

    async fn load(&self) -> Result<SavedRoundsFile> {
        load_from(&self.path).await
    }

    async fn save(&self, file: &SavedRoundsFile) -> Result<()> {
        save_to(&self.path, file).await
    }

    pub async fn list(&self, game_slug: Option<&str>) -> Result<Vec<SavedRound>> {
        let _guard = self.write_lock.lock().await;
        let mut file = self.load().await?;
        if let Some(slug) = game_slug {
            file.rounds.retain(|round| round.game_slug == slug);
        }
        file.rounds
            .sort_by_key(|round| std::cmp::Reverse(round.updated_at));
        Ok(file.rounds)
    }

    pub async fn create(
        &self,
        game_slug: String,
        mode: String,
        event_id: u32,
        description: String,
    ) -> Result<SavedRound> {
        if game_slug.is_empty() {
            return Err(anyhow!("gameSlug is required"));
        }
        if mode.is_empty() {
            return Err(anyhow!("mode is required"));
        }
        if event_id == 0 {
            return Err(anyhow!("eventId must be > 0"));
        }

        let _guard = self.write_lock.lock().await;
        let mut file = self.load().await?;
        let now = now_ms();
        let round = SavedRound {
            id: Uuid::new_v4().to_string(),
            game_slug,
            mode,
            event_id,
            description,
            created_at: now,
            updated_at: now,
        };
        file.rounds.push(round.clone());
        self.save(&file).await?;
        Ok(round)
    }

    pub async fn update_description(&self, id: &str, description: String) -> Result<SavedRound> {
        let _guard = self.write_lock.lock().await;
        let mut file = self.load().await?;
        let round = file
            .rounds
            .iter_mut()
            .find(|round| round.id == id)
            .ok_or_else(|| anyhow!("saved round not found"))?;
        round.description = description;
        round.updated_at = now_ms();
        let updated = round.clone();
        self.save(&file).await?;
        Ok(updated)
    }

    /// Insert or replace a full saved round record. Used by team sync to apply
    /// remote changes without losing timestamps or IDs.
    pub async fn upsert_raw(&self, round: SavedRound) -> Result<SavedRound> {
        let _guard = self.write_lock.lock().await;
        let mut file = self.load().await?;
        if let Some(existing) = file.rounds.iter_mut().find(|item| item.id == round.id) {
            *existing = round.clone();
        } else {
            file.rounds.push(round.clone());
        }
        self.save(&file).await?;
        Ok(round)
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut file = self.load().await?;
        let before = file.rounds.len();
        file.rounds.retain(|round| round.id != id);
        if file.rounds.len() == before {
            return Err(anyhow!("saved round not found"));
        }
        self.save(&file).await
    }
}

async fn load_from(path: &Path) -> Result<SavedRoundsFile> {
    if !fs::try_exists(path).await.unwrap_or(false) {
        return Ok(SavedRoundsFile::default());
    }
    let bytes = fs::read(path).await.context("read saved-rounds.json")?;
    serde_json::from_slice(&bytes).context("parse saved-rounds.json")
}

async fn save_to(path: &Path, file: &SavedRoundsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .context("create saved-rounds dir")?;
    }
    let bytes = serde_json::to_vec_pretty(file).context("serialize saved-rounds")?;
    fs::write(path, bytes)
        .await
        .context("write saved-rounds.json")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn default_store() -> Result<Arc<SavedRoundsStore>> {
    static STORE: OnceLock<Arc<SavedRoundsStore>> = OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(Arc::clone(store));
    }
    let store = Arc::new(SavedRoundsStore::default_local()?);
    Ok(Arc::clone(STORE.get_or_init(|| store)))
}

// Compatibility surface used by desktop team sync.
pub async fn list(game_slug: Option<&str>) -> Result<Vec<SavedRound>> {
    default_store()?.list(game_slug).await
}

pub async fn create(
    game_slug: String,
    mode: String,
    event_id: u32,
    description: String,
) -> Result<SavedRound> {
    default_store()?
        .create(game_slug, mode, event_id, description)
        .await
}

pub async fn update_description(id: &str, description: String) -> Result<SavedRound> {
    default_store()?.update_description(id, description).await
}

pub async fn upsert_raw(round: SavedRound) -> Result<SavedRound> {
    default_store()?.upsert_raw(round).await
}

pub async fn delete(id: &str) -> Result<()> {
    default_store()?.delete(id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_stores_persist_and_remain_isolated() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let store_a = SavedRoundsStore::with_path(tmp.path().join("a/rounds.json"));
        let store_b = SavedRoundsStore::with_path(tmp.path().join("b/rounds.json"));

        store_a
            .create("same-game".into(), "base".into(), 7, "A".into())
            .await
            .expect("save round A");

        assert_eq!(store_a.list(Some("same-game")).await.unwrap().len(), 1);
        assert!(store_b.list(Some("same-game")).await.unwrap().is_empty());
        assert!(tmp.path().join("a/rounds.json").exists());
        assert!(!tmp.path().join("b/rounds.json").exists());
    }

    #[tokio::test]
    async fn concurrent_creates_do_not_lose_rounds() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(SavedRoundsStore::with_path(tmp.path().join("rounds.json")));
        let mut tasks = Vec::new();
        for event_id in 1..=32 {
            let store = Arc::clone(&store);
            tasks.push(tokio::spawn(async move {
                store
                    .create("game".into(), "base".into(), event_id, String::new())
                    .await
                    .expect("concurrent save");
            }));
        }
        for task in tasks {
            task.await.expect("join save task");
        }

        assert_eq!(store.list(Some("game")).await.unwrap().len(), 32);
    }
}

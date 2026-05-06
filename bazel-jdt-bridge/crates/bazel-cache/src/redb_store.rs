use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::io;
use std::path::{Path, PathBuf};

/// Returns the platform-appropriate default cache directory for bazel-jdt.
///
/// - Linux: `$XDG_CACHE_HOME/bazel-jdt/` or `~/.cache/bazel-jdt/`
/// - macOS: `~/Library/Caches/bazel-jdt/`
/// - Windows: `%LOCALAPPDATA%/bazel-jdt/`
pub fn default_cache_dir() -> Result<PathBuf, io::Error> {
    #[cfg(target_os = "linux")]
    {
        let base = std::env::var("XDG_CACHE_HOME")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".cache"))
                    .unwrap_or_else(|_| PathBuf::from(".cache"))
            });
        Ok(base.join("bazel-jdt"))
    }

    #[cfg(target_os = "macos")]
    {
        let base = std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Caches"))
            .unwrap_or_else(|_| PathBuf::from("Library/Caches"));
        Ok(base.join("bazel-jdt"))
    }

    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".").join("cache"));
        Ok(base.join("bazel-jdt"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        // Fallback for other platforms
        let base = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        Ok(base.join(".cache").join("bazel-jdt"))
    }
}

const CLASSPATH_TABLE: TableDefinition<&str, &str> = TableDefinition::new("classpath");
const BUILD_HASH_TABLE: TableDefinition<&str, &str> = TableDefinition::new("build_hash");

/// Persistent cache for Bazel classpath data
pub struct BazelCache {
    db: Database,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] redb::DatabaseError),

    #[error("Storage error: {0}")]
    StorageError(#[from] redb::StorageError),

    #[error("Table error: {0}")]
    TableError(#[from] redb::TableError),

    #[error("Transaction error: {0}")]
    TransactionError(#[from] redb::TransactionError),

    #[error("Commit error: {0}")]
    CommitError(#[from] redb::CommitError),

    #[error("JSON deserialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

impl BazelCache {
    /// Open or create the cache database
    pub fn open(cache_dir: &Path) -> Result<Self, CacheError> {
        std::fs::create_dir_all(cache_dir)?;
        let db_path = cache_dir.join("bazel-jdt-cache.redb");
        let db = Database::create(&db_path)?;
        Ok(Self { db })
    }

    /// Get a cached classpath for a target
    pub fn get_classpath(&self, label: &str) -> Result<Option<String>, CacheError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CLASSPATH_TABLE)?;
        if let Some(value) = table.get(label)? {
            Ok(Some(value.value().to_string()))
        } else {
            Ok(None)
        }
    }

    /// Store a classpath for a target
    pub fn put_classpath(&self, label: &str, classpath_json: &str) -> Result<(), CacheError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(CLASSPATH_TABLE)?;
            table.insert(label, classpath_json)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Get a cached BUILD file hash
    pub fn get_build_hash(&self, path: &str) -> Result<Option<String>, CacheError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(BUILD_HASH_TABLE)?;
        if let Some(value) = table.get(path)? {
            Ok(Some(value.value().to_string()))
        } else {
            Ok(None)
        }
    }

    /// Store a BUILD file hash
    pub fn put_build_hash(&self, path: &str, hash: &str) -> Result<(), CacheError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(BUILD_HASH_TABLE)?;
            table.insert(path, hash)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Invalidate cached entries for specific targets
    pub fn invalidate_targets(&self, labels: &[String]) -> Result<(), CacheError> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(CLASSPATH_TABLE)?;
            for label in labels {
                table.remove(label.as_str())?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Load all cached classpaths (bulk load for IDE restart)
    pub fn load_all_classpaths(&self) -> Result<Vec<(String, String)>, CacheError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CLASSPATH_TABLE)?;
        let mut result = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            result.push((key.value().to_string(), value.value().to_string()));
        }
        Ok(result)
    }

    /// Clear all cached data
    pub fn clear(&self) -> Result<(), CacheError> {
        let txn = self.db.begin_write()?;
        {
            txn.delete_table(CLASSPATH_TABLE)?;
            txn.delete_table(BUILD_HASH_TABLE)?;
            txn.open_table(CLASSPATH_TABLE)?;
            txn.open_table(BUILD_HASH_TABLE)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Validate all cached classpaths and remove corrupted entries.
    /// Returns the number of entries validated and the number of corrupted entries found.
    pub fn validate_and_repair(&self) -> Result<(usize, usize), CacheError> {
        let corrupted_labels = self.find_corrupted_entries()?;
        let corrupted_count = corrupted_labels.len();
        if !corrupted_labels.is_empty() {
            self.invalidate_targets(&corrupted_labels)?;
        }
        let total = self.count_classpath_entries()?;
        Ok((total, corrupted_count))
    }

    fn find_corrupted_entries(&self) -> Result<Vec<String>, CacheError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CLASSPATH_TABLE)?;
        let mut corrupted = Vec::new();
        for entry in table.iter()? {
            match entry {
                Ok((key, value)) => {
                    let label = key.value();
                    let json = value.value();
                    if serde_json::from_str::<serde_json::Value>(json).is_err() {
                        log::warn!("Corrupted cache entry for label '{}': invalid JSON", label);
                        corrupted.push(label.to_string());
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read cache entry: {}", e);
                }
            }
        }
        Ok(corrupted)
    }

    pub fn count_classpath_entries(&self) -> Result<usize, CacheError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CLASSPATH_TABLE)?;
        Ok(table.len()? as usize)
    }

    pub fn list_build_hash_keys(&self) -> Result<Vec<String>, CacheError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(BUILD_HASH_TABLE)?;
        let mut keys = Vec::new();
        for entry in table.iter()? {
            match entry {
                Ok((key, _)) => keys.push(key.value().to_string()),
                Err(e) => log::warn!("Failed to read build hash key: {}", e),
            }
        }
        Ok(keys)
    }

    /// Open the cache, or recreate it if the database file is corrupted.
    pub fn open_or_recreate(cache_dir: &Path) -> Result<Self, CacheError> {
        match Self::open(cache_dir) {
            Ok(cache) => Ok(cache),
            Err(CacheError::DatabaseError(_)) => {
                let db_path = cache_dir.join("bazel-jdt-cache.redb");
                log::warn!(
                    "Cache database corrupted, recreating: {}",
                    db_path.display()
                );
                let _ = std::fs::remove_file(&db_path);
                Self::open(cache_dir)
            }
            Err(e) => Err(e),
        }
    }

    /// Open the cache and validate entries, removing any corrupted ones.
    pub fn open_and_validate(cache_dir: &Path) -> Result<(Self, usize, usize), CacheError> {
        let cache = Self::open_or_recreate(cache_dir)?;
        let (total, corrupted) = cache.validate_and_repair()?;
        Ok((cache, total, corrupted))
    }

    pub fn ensure_accessible(&self) -> Result<(), CacheError> {
        let txn = self.db.begin_read()?;
        let _ = txn.open_table(CLASSPATH_TABLE)?;
        Ok(())
    }
}

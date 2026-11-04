//! Stockage clé-valeur ordonné : en mémoire (tests) ou sur disque (redb).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

pub type KvPair = (Vec<u8>, Vec<u8>);

/// Cache de pages de redb. Celui par défaut (1 Gio) finit par recopier toute la base en
/// mémoire, en plus du cache disque du noyau qui garde déjà les pages lues : un petit cache
/// suffit, et le démon reste léger.
pub const CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Une écriture, appliquée avec les autres d'un même lot de façon atomique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

/// Stockage clé-valeur trié par clé (ordre lexicographique des octets).
pub trait Storage {
    type Error: std::error::Error + Send + Sync + 'static;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error>;
    /// Toutes les paires dont la clé commence par `prefix`, dans l'ordre.
    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<KvPair>, Self::Error>;
    /// Parcourt dans l'ordre les paires dont la clé commence par `prefix`, sans les charger
    /// toutes en mémoire ; `f` renvoie `false` pour arrêter.
    fn visit_prefix(
        &self,
        prefix: &[u8],
        f: &mut dyn FnMut(&[u8], &[u8]) -> bool,
    ) -> Result<(), Self::Error> {
        for (k, v) in self.scan_prefix(prefix)? {
            if !f(&k, &v) {
                break;
            }
        }
        Ok(())
    }
    /// Applique toutes les écritures, ou aucune.
    fn apply(&mut self, ops: Vec<Op>) -> Result<(), Self::Error>;
}

/// Stockage en mémoire, pour les tests et le prototypage.
#[derive(Debug, Default)]
pub struct MemStorage {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Storage for MemStorage {
    type Error = Infallible;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Infallible> {
        Ok(self.map.get(key).cloned())
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<KvPair>, Infallible> {
        Ok(self
            .map
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    fn apply(&mut self, ops: Vec<Op>) -> Result<(), Infallible> {
        for op in ops {
            match op {
                Op::Put(k, v) => {
                    self.map.insert(k, v);
                }
                Op::Delete(k) => {
                    self.map.remove(&k);
                }
            }
        }
        Ok(())
    }
}

/// En dessous, une base n'est jamais réécrite (le gain ne vaut pas l'écriture).
const TIDY_MIN_BYTES: u64 = 16 << 20;

const KV: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");

/// Stockage sur disque : base redb (un fichier, transactions ACID, lectures concurrentes).
pub struct RedbStorage {
    db: Database,
}

impl std::fmt::Debug for RedbStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbStorage").finish_non_exhaustive()
    }
}

impl RedbStorage {
    /// Ouvre la base, en la créant si besoin.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, redb::Error> {
        let db = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .create(path)?;
        let txn = db.begin_write()?;
        txn.open_table(KV)?;
        txn.commit()?;
        Ok(Self { db })
    }
}

/// Occupation d'une base : fichier, octets utiles, fragmentation, et volume par espace de clés
/// (premier octet de la clé ; pour les arêtes `E`, par type d'arête).
#[derive(Debug, Default)]
pub struct StorageReport {
    pub file_bytes: u64,
    pub stored_bytes: u64,
    pub metadata_bytes: u64,
    pub fragmented_bytes: u64,
    /// (espace, nombre de paires, octets de clés, octets de valeurs).
    pub spaces: Vec<(String, u64, u64, u64)>,
}

impl RedbStorage {
    /// Octets perdus (pages vides ou à moitié vides) d'après redb.
    pub fn fragmented_bytes(&self) -> Result<u64, redb::Error> {
        let txn = self.db.begin_write()?;
        let bytes = txn.stats()?.fragmented_bytes();
        txn.abort()?;
        Ok(bytes)
    }

    /// Ouvre la base à `path` ; si plus d'un tiers du fichier est perdu (pages vides ou à moitié
    /// pleines, que redb ne rend jamais), la réécrit d'abord dans l'ordre des clés, à côté, puis
    /// remplace l'ancienne (qui reste intacte en cas d'échec). Renvoie aussi ce qui a été fait.
    /// `force` : réécrit quel que soit l'état (commande `nknown compact`).
    pub fn open_tidy(
        path: impl AsRef<Path>,
        force: bool,
    ) -> Result<(Self, Option<String>), redb::Error> {
        let path = path.as_ref();
        let storage = Self::open(path)?;
        let file = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let lost = storage.fragmented_bytes()?;
        if !force && (file < TIDY_MIN_BYTES || lost * 3 < file) {
            return Ok((storage, None));
        }
        let tmp = path.with_extension("redb.tidy");
        let _ = std::fs::remove_file(&tmp);
        storage.rewrite_to(&tmp)?;
        drop(storage);
        std::fs::rename(&tmp, path).map_err(redb::Error::Io)?;
        let after = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let note = format!(
            "base réécrite : {:.1} Mo → {:.1} Mo",
            file as f64 / 1e6,
            after as f64 / 1e6
        );
        Ok((Self::open(path)?, Some(note)))
    }

    /// Copie toute la base dans `path`, dans l'ordre des clés : pages pleines, sans trou.
    pub fn rewrite_to(&self, path: impl AsRef<Path>) -> Result<(), redb::Error> {
        let out = Database::builder()
            .set_cache_size(CACHE_BYTES)
            .create(path)?;
        let read = self.db.begin_read()?;
        let table = read.open_table(KV)?;
        let txn = out.begin_write()?;
        {
            let mut dst = txn.open_table(KV)?;
            for entry in table.iter()? {
                let (k, v) = entry?;
                dst.insert(k.value(), v.value())?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Parcourt toute la base (lecture seule) et mesure son occupation.
    pub fn report(&self, file_bytes: u64) -> Result<StorageReport, redb::Error> {
        use std::collections::BTreeMap;
        let txn = self.db.begin_write()?;
        let stats = txn.stats()?;
        txn.abort()?;
        let mut spaces: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        let read = self.db.begin_read()?;
        let table = read.open_table(KV)?;
        for entry in table.iter()? {
            let (k, v) = entry?;
            let (k, v) = (k.value(), v.value());
            let name = match k.first() {
                Some(b'E') => format!("E{}", k.get(9).copied().unwrap_or(0)),
                Some(&c) => (c as char).to_string(),
                None => "?".into(),
            };
            let e = spaces.entry(name).or_default();
            e.0 += 1;
            e.1 += k.len() as u64;
            e.2 += v.len() as u64;
        }
        Ok(StorageReport {
            file_bytes,
            stored_bytes: stats.stored_bytes(),
            metadata_bytes: stats.metadata_bytes(),
            fragmented_bytes: stats.fragmented_bytes(),
            spaces: spaces
                .into_iter()
                .map(|(k, (n, kb, vb))| (k, n, kb, vb))
                .collect(),
        })
    }
}

impl Storage for RedbStorage {
    type Error = redb::Error;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, redb::Error> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        Ok(table.get(key)?.map(|v| v.value().to_vec()))
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<KvPair>, redb::Error> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        let mut out = Vec::new();
        for entry in table.range(prefix..)? {
            let (k, v) = entry?;
            if !k.value().starts_with(prefix) {
                break;
            }
            out.push((k.value().to_vec(), v.value().to_vec()));
        }
        Ok(out)
    }

    fn visit_prefix(
        &self,
        prefix: &[u8],
        f: &mut dyn FnMut(&[u8], &[u8]) -> bool,
    ) -> Result<(), redb::Error> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(KV)?;
        for entry in table.range(prefix..)? {
            let (k, v) = entry?;
            if !k.value().starts_with(prefix) || !f(k.value(), v.value()) {
                break;
            }
        }
        Ok(())
    }

    fn apply(&mut self, ops: Vec<Op>) -> Result<(), redb::Error> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            for op in &ops {
                match op {
                    Op::Put(k, v) => {
                        table.insert(k.as_slice(), v.as_slice())?;
                    }
                    Op::Delete(k) => {
                        table.remove(k.as_slice())?;
                    }
                }
            }
        }
        txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise<S: Storage>(s: &mut S) {
        s.apply(vec![
            Op::Put(b"a1".to_vec(), b"x".to_vec()),
            Op::Put(b"b1".to_vec(), b"y".to_vec()),
            Op::Put(b"b2".to_vec(), b"z".to_vec()),
            Op::Put(b"c1".to_vec(), b"w".to_vec()),
        ])
        .unwrap();
        s.apply(vec![Op::Delete(b"c1".to_vec())]).unwrap();
        let keys: Vec<_> = s
            .scan_prefix(b"b")
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, [b"b1".to_vec(), b"b2".to_vec()]);
        assert_eq!(s.get(b"a1").unwrap(), Some(b"x".to_vec()));
        assert_eq!(s.get(b"c1").unwrap(), None);
    }

    #[test]
    fn memory() {
        exercise(&mut MemStorage::default());
    }

    #[test]
    fn redb_persists() {
        let path = std::env::temp_dir().join(format!("nknown-redb-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        exercise(&mut RedbStorage::open(&path).unwrap());
        // Rouverte, la base a gardé ses données.
        let reopened = RedbStorage::open(&path).unwrap();
        assert_eq!(reopened.get(b"b2").unwrap(), Some(b"z".to_vec()));
        drop(reopened);
        std::fs::remove_file(&path).unwrap();
    }
}

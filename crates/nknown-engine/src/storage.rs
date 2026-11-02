//! Stockage clé-valeur ordonné : en mémoire (tests) ou sur disque (redb).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::Path;

use redb::{Database, ReadableDatabase, TableDefinition};

pub type KvPair = (Vec<u8>, Vec<u8>);

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
        let db = Database::create(path)?;
        let txn = db.begin_write()?;
        txn.open_table(KV)?;
        txn.commit()?;
        Ok(Self { db })
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

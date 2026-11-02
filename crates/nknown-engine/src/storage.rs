//! Abstraction clé-valeur ordonnée.

use std::collections::BTreeMap;
use std::convert::Infallible;

pub type KvPair = (Vec<u8>, Vec<u8>);

/// Stockage clé-valeur trié par clé (ordre lexicographique des octets).
pub trait Storage {
    type Error: std::error::Error + Send + Sync + 'static;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error>;
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error>;
    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error>;
    /// Toutes les paires dont la clé commence par `prefix`, dans l'ordre.
    fn scan_prefix<'a>(
        &'a self,
        prefix: &[u8],
    ) -> Box<dyn Iterator<Item = Result<KvPair, Self::Error>> + 'a>;
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

    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Infallible> {
        self.map.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> Result<(), Infallible> {
        self.map.remove(key);
        Ok(())
    }

    fn scan_prefix<'a>(
        &'a self,
        prefix: &[u8],
    ) -> Box<dyn Iterator<Item = Result<KvPair, Infallible>> + 'a> {
        let prefix = prefix.to_vec();
        Box::new(
            self.map
                .range(prefix.clone()..)
                .take_while(move |(k, _)| k.starts_with(&prefix))
                .map(|(k, v)| Ok((k.clone(), v.clone()))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_prefix_is_bounded() {
        let mut s = MemStorage::default();
        for k in [&b"a1"[..], b"b1", b"b2", b"c1"] {
            s.put(k, b"").unwrap();
        }
        let keys: Vec<_> = s.scan_prefix(b"b").map(|r| r.unwrap().0).collect();
        assert_eq!(keys, [b"b1".to_vec(), b"b2".to_vec()]);
    }
}

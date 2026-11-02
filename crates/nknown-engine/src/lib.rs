//! Moteur NKnown : graphe pondéré, index et scoring, au-dessus d'un [`Storage`] abstrait.
//! Le stockage durable (fjall ou redb) viendra derrière ce même trait.

pub mod graph;
pub mod storage;

pub use graph::{Edge, EdgeKind, NodeId, NodeKind};
pub use storage::{MemStorage, Storage};

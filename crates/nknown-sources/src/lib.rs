//! Implémentations du trait [`Source`](nknown_core::source::Source) :
//! fichiers locaux, flux RSS/Atom, Wikipédia.

pub mod files;
pub mod html;
pub mod http;
pub mod rss;
pub mod wikipedia;

pub use files::LocalFiles;
pub use rss::RssSource;
pub use wikipedia::{Selection, WikipediaSource};

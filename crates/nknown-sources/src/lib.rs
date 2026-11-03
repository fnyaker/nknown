//! Implémentations du trait [`Source`](nknown_core::source::Source) :
//! fichiers locaux, flux RSS/Atom, Wikipédia, sites web.

pub mod files;
pub mod html;
pub mod http;
pub mod rss;
pub mod web;
pub mod wikidump;
pub mod wikipedia;
pub mod wikitext;

pub use files::LocalFiles;
pub use rss::RssSource;
pub use web::WebSource;
pub use wikidump::WikiDump;
pub use wikipedia::{Selection, WikipediaSource};

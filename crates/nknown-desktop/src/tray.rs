//! Icône dans la zone de notification, par le protocole StatusNotifierItem (celui de Plasma, lu
//! aussi par les autres barres qui le prennent en charge), sur le bus de session ; menu par
//! `com.canonical.dbusmenu`. En Rust : la zone de notification de Qt exigerait Qt Widgets.
//!
//! Clic : montrer ou cacher la fenêtre. Menu : « Rechercher… », « Quitter ».

use std::collections::HashMap;
use std::sync::Arc;

use zbus::{Variant, interface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Search,
    Quit,
}

type Handler = Arc<dyn Fn(Action) + Send + Sync>;

struct Item {
    on: Handler,
}

/// (largeur, hauteur, pixels ARGB32 grand-boutistes) : signature `(iiay)`.
type Pixmap = (i32, i32, Vec<u8>);

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "nknown"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "NKnown"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        "nknown"
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        [22, 32, 48].into_iter().map(icon).collect()
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        (
            "nknown".into(),
            Vec::new(),
            "NKnown".into(),
            "Rechercher dans vos connaissances".into(),
        )
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> zbus::ObjectPath<'static> {
        zbus::ObjectPath::from_static_str_unchecked(MENU_PATH)
    }

    fn activate(&self, _x: i32, _y: i32) {
        (self.on)(Action::Toggle);
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {
        (self.on)(Action::Search);
    }
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
}

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const SEARCH_ID: i32 = 1;
const QUIT_ID: i32 = 2;

type Layout = (
    i32,
    HashMap<String, Variant<'static>>,
    Vec<Variant<'static>>,
);

struct Menu {
    on: Handler,
}

fn entry(id: i32, label: &str) -> Variant<'static> {
    let mut props: HashMap<String, Variant<'static>> = HashMap::new();
    props.insert("label".into(), Variant::from(label.to_owned()));
    Variant::from((id, props, Vec::<Variant<'static>>::new()))
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(out_args("revision", "layout"))]
    fn get_layout(&self, _parent: i32, _depth: i32, _props: Vec<String>) -> (u32, Layout) {
        let mut root: HashMap<String, Variant<'static>> = HashMap::new();
        root.insert(
            "children-display".into(),
            Variant::from("submenu".to_owned()),
        );
        (
            1,
            (
                0,
                root,
                vec![entry(SEARCH_ID, "Rechercher…"), entry(QUIT_ID, "Quitter")],
            ),
        )
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _props: Vec<String>,
    ) -> Vec<(i32, HashMap<String, Variant<'static>>)> {
        ids.into_iter()
            .filter_map(|id| {
                let label = match id {
                    SEARCH_ID => "Rechercher…",
                    QUIT_ID => "Quitter",
                    _ => return None,
                };
                let mut props: HashMap<String, Variant<'static>> = HashMap::new();
                props.insert("label".into(), Variant::from(label.to_owned()));
                Some((id, props))
            })
            .collect()
    }

    fn event(&self, id: i32, event: &str, _data: Variant<'_>, _timestamp: u32) {
        if event != "clicked" {
            return;
        }
        match id {
            SEARCH_ID => (self.on)(Action::Search),
            QUIT_ID => (self.on)(Action::Quit),
            _ => {}
        }
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }
}

/// Publie l'icône ; `false` s'il n'y a pas de zone de notification (aucun hôte inscrit), auquel
/// cas fermer la fenêtre quitte l'app.
pub fn spawn(on: impl Fn(Action) + Send + Sync + 'static) -> bool {
    let on: Handler = Arc::new(on);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("nknown-zone".into())
        .spawn(move || {
            let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
            let conn = zbus::block_on(
                zbus::connection::Builder::session()
                    .name(name.as_str())
                    .serve_at(ITEM_PATH, Item { on: on.clone() })
                    .serve_at(MENU_PATH, Menu { on })
                    .build(),
            );
            let Ok(conn) = conn else {
                let _ = tx.send(false);
                return;
            };
            let registered = zbus::block_on(conn.call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some("org.kde.StatusNotifierWatcher"),
                "RegisterStatusNotifierItem",
                &(name.as_str(),),
            ))
            .is_ok();
            let _ = tx.send(registered);
            if registered {
                // La connexion reste ouverte tant que l'app vit.
                loop {
                    std::thread::park();
                }
            }
        })
        .expect("fil de la zone de notification");
    rx.recv().unwrap_or(false)
}

/// L'icône dessinée : l'anneau de la requête et trois nœuds (mot, texte, code), couleurs de la
/// charte, avec un anticrénelage simple (couverture selon la distance au bord).
fn icon(size: i32) -> Pixmap {
    let n = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    // (centre x, centre y, rayon, épaisseur d'anneau ou 0 pour un disque, couleur)
    let shapes: [(f32, f32, f32, f32, [u8; 3]); 4] = [
        (0.5, 0.5, 0.22, 0.09, [0x7C, 0xF3, 0xFF]),
        (0.2, 0.22, 0.1, 0.0, [0x6A, 0xA8, 0xFF]),
        (0.82, 0.3, 0.11, 0.0, [0xFF, 0xC4, 0x6B]),
        (0.68, 0.84, 0.09, 0.0, [0x5C, 0xF0, 0xA6]),
    ];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
            for &(cx, cy, r, ring, rgb) in &shapes {
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() * n;
                let (r, ring) = (r * n, ring * n);
                let edge = if ring > 0.0 {
                    (ring / 2.0) - (d - (r - ring / 2.0)).abs()
                } else {
                    r - d
                };
                let cover = (edge + 0.5).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let i = ((y * size + x) * 4) as usize;
                    let a = (cover * 255.0) as u8;
                    if a >= px[i] {
                        px[i..i + 4].copy_from_slice(&[a, rgb[0], rgb[1], rgb[2]]);
                    }
                }
            }
        }
    }
    (size, size, px)
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_the_ring_and_transparent_corners() {
        let (w, h, px) = super::icon(32);
        assert_eq!((w, h, px.len()), (32, 32, 32 * 32 * 4));
        assert_eq!(px[0], 0, "coin transparent");
        // Sur l'anneau (à droite du centre, à son rayon) : opaque, couleur de la requête.
        let i = ((16 * 32 + 16 + 7) * 4) as usize;
        assert!(px[i] > 200 && px[i + 1] == 0x7C, "{:?}", &px[i..i + 4]);
    }
}

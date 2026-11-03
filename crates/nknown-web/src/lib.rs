//! Interface web de NKnown (Leptos, rendu côté navigateur), servie par `nknown-server`.
//!
//! Même charte que l'app native (`docs/ui-guidelines.md`) : accueil réduit au champ, champ qui
//! glisse vers le haut, jauge en trois étapes, carte construite en direct (patchs du flux),
//! liste, inspecteur ; Échap revient en arrière. Authentification : le jeton s'échange contre un
//! cookie de session `HttpOnly` et n'est jamais gardé dans la page.

mod api;
mod map;

use leptos::prelude::*;
use leptos::task::spawn_local;
use nknown_api::{self as a, Graph, Hit, SearchEvent};
use wasm_bindgen::prelude::*;

use map::{Map, MapState, kind_label};

#[wasm_bindgen(start)]
pub fn start() {
    leptos::mount::mount_to_body(App);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Auth {
    Checking,
    Needed,
    Ok,
}

/// Étape de la recherche : 0 rien, 1 analyse, 2 index, 3 carte, 4 terminée.
fn step(phase: a::Phase) -> u8 {
    match phase {
        a::Phase::Analyze => 1,
        a::Phase::Search => 2,
        a::Phase::Graph => 3,
    }
}

#[component]
fn App() -> impl IntoView {
    let auth = RwSignal::new(Auth::Checking);
    let status = RwSignal::new(None::<a::Status>);
    let query = RwSignal::new(String::new());
    let searching = RwSignal::new(false);
    let stage = RwSignal::new(0u8);
    let error = RwSignal::new(String::new());
    let hits = RwSignal::new(Vec::<Hit>::new());
    let took = RwSignal::new(0.0f64);
    let map = RwSignal::new(MapState::default());
    let selected = RwSignal::new(String::new());
    let inspected = RwSignal::new(String::new());
    let generation = StoredValue::new(0u64);

    let refresh = move || {
        spawn_local(async move {
            match api::status().await {
                Ok(Some(s)) => {
                    status.set(Some(s));
                    auth.set(Auth::Ok);
                }
                Ok(None) => auth.set(Auth::Needed),
                Err(e) => {
                    error.set(e);
                    auth.set(Auth::Ok);
                }
            }
        });
    };
    refresh();

    let launch = move |q: String| {
        if q.trim().is_empty() {
            return;
        }
        let mine = generation.get_value() + 1;
        generation.set_value(mine);
        error.set(String::new());
        hits.set(Vec::new());
        selected.set(String::new());
        inspected.set(String::new());
        map.set(MapState::default());
        searching.set(true);
        stage.set(1);
        spawn_local(async move {
            let mut graph = Graph::default();
            let current = move || generation.get_value() == mine;
            let result = api::search(&q, |event| {
                if !current() {
                    return;
                }
                match event {
                    SearchEvent::Phase { phase } => stage.set(step(phase)),
                    SearchEvent::GraphPatch(patch) => {
                        graph.apply(&patch);
                        map.set(MapState::from_graph(&graph));
                    }
                    SearchEvent::Done(resp) => {
                        graph = resp.graph.clone();
                        map.set(MapState::from_graph(&graph));
                        took.set(resp.took_ms);
                        hits.set(resp.hits);
                        stage.set(4);
                    }
                    SearchEvent::Error { message } => {
                        error.set(message);
                        stage.set(4);
                    }
                    SearchEvent::Terms { .. } => {}
                }
            })
            .await;
            if let Err(e) = result
                && current()
            {
                if e.contains("401") {
                    auth.set(Auth::Needed);
                }
                error.set(e);
                stage.set(4);
            }
        });
    };

    let pick = Callback::new(move |id: String| {
        selected.set(id.clone());
        inspected.set(id);
    });

    // Échap : referme l'inspecteur, puis désélectionne, puis revient à l'accueil.
    window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() != "Escape" {
            return;
        }
        if !inspected.get_untracked().is_empty() {
            inspected.set(String::new());
        } else if !selected.get_untracked().is_empty() {
            selected.set(String::new());
        } else if searching.get_untracked() {
            searching.set(false);
            stage.set(0);
            query.set(String::new());
        }
    });

    let busy = Signal::derive(move || (1..4).contains(&stage.get()));

    view! {
        <Show when=move || auth.get() == Auth::Needed fallback={move || view! {
            <div class:searching=move || searching.get()>
                <div class="top">
                    <div class="logo">
                        <svg width="44" height="44" viewBox="-22 -22 44 44">
                            <circle r="18" fill="none" stroke="var(--accent)" stroke-width="6" />
                        </svg>
                        "NKnown"
                    </div>
                    <input class="field" placeholder="Rechercher" autofocus
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                        on:keydown=move |ev| if ev.key() == "Enter" { launch(query.get_untracked()) }
                    />
                    <div class="gauge" class:running=busy class:done=move || stage.get() == 4>
                        {(1..=3u8).map(|i| view! {
                            <span class:on={move || stage.get() >= i}><i></i></span>
                        }).collect_view()}
                    </div>
                    <Show when=move || !error.get().is_empty()>
                        <div class="error">{move || error.get()}</div>
                    </Show>
                </div>
                <div class="content">
                    <div class="map" on:click=move |_| inspected.set(String::new())>
                        <Map state=map selected=selected busy=busy on_pick=pick />
                    </div>
                    <div class="side">
                        <div class="meta">
                            {move || (stage.get() == 4 && !hits.with(Vec::is_empty) && inspected.get().is_empty()).then(|| {
                                let n = hits.with(Vec::len);
                                let ms = took.get();
                                format!("{n} résultat{}  ·  {} ms", if n > 1 { "s" } else { "" },
                                    if ms < 1.0 { "< 1".to_owned() } else { format!("{}", ms.round()) })
                            })}
                        </div>
                        <div class="list" class:hidden=move || !inspected.get().is_empty()>
                            <For each={move || hits.get().into_iter().enumerate().collect::<Vec<_>>()}
                                key={|(_, h): &(usize, Hit)| h.doc}
                                children={move |(i, h): (usize, Hit)| view! { <Card hit=h index=i selected=selected on_pick=pick /> }}
                            />
                            <Show when=move || stage.get() == 4 && hits.with(Vec::is_empty) && error.get().is_empty()>
                                <div class="empty">"Rien de trouvé"</div>
                            </Show>
                        </div>
                        <Show when=move || !inspected.get().is_empty()>
                            <Inspector map=map hits=hits inspected=inspected selected=selected
                                on_search=Callback::new(move |w: String| { query.set(w.clone()); launch(w) }) />
                        </Show>
                    </div>
                </div>
                <div class="dot" style=move || format!("background: {}", match (status.get(), error.get().is_empty()) {
                    (Some(s), _) if s.collecting || matches!(s.models, a::ModelsState::Downloading { .. }) => "var(--accent)",
                    (Some(_), true) => "var(--ok)",
                    _ => "var(--danger)",
                })></div>
            </div>
        }}>
            <Login on_done=Callback::new(move |()| refresh()) />
        </Show>
    }
}

/// Carte de résultat : barre de score, nature, titre, source et date, extrait.
#[component]
fn Card(
    hit: Hit,
    index: usize,
    selected: RwSignal<String>,
    on_pick: Callback<String>,
) -> impl IntoView {
    let node = format!("d{}", hit.doc);
    let kind = if hit.is_code { "code" } else { "prose" };
    let node_sel = node.clone();
    let bar = format!(
        "height: calc((100% - 24px) * {:.3}); background: var(--{kind}); animation-delay: {}ms",
        hit.relative.max(0.08),
        (index * 30).min(300)
    );
    let meta = format!(
        "{}{}{}",
        hit.source,
        if hit.date > 0 {
            format!("  ·  {}", ago(hit.date))
        } else {
            String::new()
        },
        hit.code_lang
            .as_deref()
            .map(|l| format!("  ·  {l}"))
            .unwrap_or_default()
    );
    let snippet = highlight(&hit.snippet, &hit.matched);
    view! {
        <div class=format!("card {kind}") class:selected=move || selected.get() == node_sel
            style=format!("animation-delay: {}ms", (index * 30).min(300))
            on:click=move |_| on_pick.run(node.clone())
            on:dblclick={
                let url = hit.url.clone();
                move |_| if let (Some(u), Some(w)) = (url.as_deref(), web_sys::window()) {
                    let _ = w.open_with_url_and_target(u, "_blank");
                }
            }>
            <div class="bar" style=bar></div>
            <h3><span class=format!("glyph {kind}")></span>{hit.title.clone().unwrap_or_else(|| hit.source.clone())}</h3>
            <div class="src">{meta}</div>
            <p inner_html=snippet></p>
        </div>
    }
}

/// Inspecteur d'un nœud : nature, liens par poids, actions.
#[component]
fn Inspector(
    map: RwSignal<MapState>,
    hits: RwSignal<Vec<Hit>>,
    inspected: RwSignal<String>,
    selected: RwSignal<String>,
    on_search: Callback<String>,
) -> impl IntoView {
    let node = move || map.with(|m| m.nodes.get(&inspected.get()).cloned());
    let url = move || {
        let id = inspected.get();
        hits.with(|h| {
            h.iter()
                .find(|h| format!("d{}", h.doc) == id)
                .and_then(|h| h.url.clone())
        })
    };
    view! {
        <div class="inspector">
            {move || node().map(|n| {
                let kind = n.kind;
                let label = if n.label.is_empty() { "requête".to_owned() } else { n.label.clone() };
                let word = n.label.clone();
                view! {
                    <header>
                        <span class=format!("glyph {kind}")></span>
                        <div style="flex:1;min-width:0">
                            <h2>{label}</h2>
                            <div class="kind" style=format!("color: var(--{})", if kind == "query" { "accent" } else if kind == "related" { "prose" } else { kind })>{kind_label(kind)}</div>
                        </div>
                        <button class="close" aria-label="Fermer" on:click=move |_| inspected.set(String::new())>"✕"</button>
                    </header>
                    <div class="actions">
                        {url().map(|u| view! {
                            <button class="pill" on:click=move |_| if let Some(w) = web_sys::window() {
                                let _ = w.open_with_url_and_target(&u, "_blank");
                            }>"Ouvrir"</button>
                        })}
                        {matches!(kind, "term" | "qualifier").then(|| view! {
                            <button class="pill" on:click=move |_| on_search.run(word.clone())>"Chercher ce mot"</button>
                        })}
                    </div>
                }
            })}
            <div class="links">
                {move || map.with(|m| m.links(&inspected.get())).into_iter().map(|(other, w)| {
                    let id = other.id.clone();
                    let color = match other.kind { "query" => "accent", "related" => "prose", k => k };
                    view! {
                        <div class="link" on:click=move |_| { selected.set(id.clone()); inspected.set(id.clone()); }>
                            <span class=format!("glyph {}", other.kind) style="flex:none"></span>
                            <span>{if other.label.is_empty() { "requête".to_owned() } else { other.label.clone() }}</span>
                            <div class="weight"><i style=format!("width: {:.0}%; background: var(--{color})", (w.max(0.04) * 100.0))></i></div>
                        </div>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
fn Login(on_done: Callback<()>) -> impl IntoView {
    let token = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    let submit = move || {
        let t = token.get_untracked();
        spawn_local(async move {
            match api::login(&t).await {
                Ok(()) => {
                    token.set(String::new());
                    on_done.run(());
                }
                Err(e) => error.set(e),
            }
        });
    };
    view! {
        <div class="login">
            <svg width="44" height="44" viewBox="-22 -22 44 44" style="margin-bottom:16px">
                <circle r="18" fill="none" stroke="var(--accent)" stroke-width="6" />
            </svg>
            <p>"Jeton d'accès de ce serveur"</p>
            <input type="password" autofocus autocomplete="current-password"
                prop:value=move || token.get()
                on:input=move |ev| token.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" { submit() } />
            <button class="pill" on:click=move |_| submit()>"Entrer"</button>
            <Show when=move || !error.get().is_empty()>
                <div class="error">{move || error.get()}</div>
            </Show>
        </div>
    }
}

/// Date relative courte (« il y a 3 h »).
fn ago(seconds: i64) -> String {
    let now = (js_sys::Date::now() / 1000.0) as i64;
    let d = now - seconds;
    if d < 90 {
        "à l'instant".into()
    } else if d < 3600 {
        format!("il y a {} min", d / 60)
    } else if d < 86_400 {
        format!("il y a {} h", d / 3600)
    } else if d < 172_800 {
        "hier".into()
    } else if d < 2_592_000 {
        format!("il y a {} j", d / 86_400)
    } else {
        let date = js_sys::Date::new(&JsValue::from_f64(seconds as f64 * 1000.0));
        date.to_locale_date_string("fr-FR", &JsValue::UNDEFINED)
            .as_string()
            .unwrap_or_default()
    }
}

/// Extrait échappé, mots de la requête en couleur de mot (`<mark>`).
fn highlight(text: &str, words: &[String]) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let lower = escaped.to_lowercase();
    // Minuscules d'une autre longueur (quelques caractères) : positions incomparables, pas de
    // surlignage.
    if lower.len() != escaped.len() {
        return escaped;
    }
    let words: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() > 1)
        .map(|w| w.to_lowercase())
        .collect();
    let mut out = String::with_capacity(escaped.len());
    let mut i = 0;
    while i < escaped.len() {
        let hit = words
            .iter()
            .filter(|w| lower[i..].starts_with(w.as_str()))
            .map(String::len)
            .max();
        match hit {
            Some(n) if escaped.is_char_boundary(i + n) => {
                out.push_str("<mark>");
                out.push_str(&escaped[i..i + n]);
                out.push_str("</mark>");
                i += n;
            }
            _ => {
                let c = escaped[i..].chars().next().unwrap_or(' ');
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::highlight;

    #[test]
    fn highlight_escapes_then_marks() {
        assert_eq!(
            highlight("Le <b>Jeton</b> du jeton", &["jeton".into()]),
            "Le &lt;b&gt;<mark>Jeton</mark>&lt;/b&gt; du <mark>jeton</mark>"
        );
        // Un mot d'une lettre n'est pas surligné.
        assert_eq!(highlight("a b", &["a".into()]), "a b");
    }
}

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

use map::{Contact, LinkView, Map, MapState, kind_label};

/// Clé du niveau de variété dans le stockage local du navigateur.
const VARIETY_KEY: &str = "nknown.variety";

fn variety_key(v: a::Variety) -> &'static str {
    match v {
        a::Variety::Strict => "strict",
        a::Variety::Normal => "normal",
        a::Variety::Diverse => "diverse",
    }
}

fn variety_from_key(value: &str) -> a::Variety {
    match value {
        "strict" => a::Variety::Strict,
        "diverse" => a::Variety::Diverse,
        _ => a::Variety::Normal,
    }
}

fn variety_label(v: a::Variety) -> &'static str {
    match v {
        a::Variety::Strict => "Strict",
        a::Variety::Normal => "Normal",
        a::Variety::Diverse => "Diversifié",
    }
}

fn variety_hint(v: a::Variety) -> &'static str {
    match v {
        a::Variety::Strict => {
            "Liens de sens très proches uniquement ; moins de résultats, plus précis."
        }
        a::Variety::Normal => "Élargissement habituel.",
        a::Variety::Diverse => {
            "Liens plus larges, définitions et voisins de mots ; plus de résultats, plus de bruit possible."
        }
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Dernier niveau choisi, `Normal` si le stockage local est indisponible.
fn load_variety() -> a::Variety {
    storage()
        .and_then(|s| s.get_item(VARIETY_KEY).ok().flatten())
        .map(|v| variety_from_key(&v))
        .unwrap_or_default()
}

fn save_variety(v: a::Variety) {
    if let Some(s) = storage() {
        let _ = s.set_item(VARIETY_KEY, variety_key(v));
    }
}

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

/// Curseur de variété (charte, section 7) : trois positions sous le trait du champ.
#[component]
fn VarietyControl(value: RwSignal<a::Variety>, on_change: Callback<a::Variety>) -> impl IntoView {
    let index = move || match value.get() {
        a::Variety::Strict => 0u8,
        a::Variety::Normal => 1,
        a::Variety::Diverse => 2,
    };
    view! {
        <div class="variety" title=move || variety_hint(value.get())>
            <div class="variety-track">
                <span class="variety-ticks" aria-hidden="true"><i></i><i></i><i></i></span>
                <input
                    type="range" class="variety-range" min="0" max="2" step="1"
                    aria-label="Variété de recherche"
                    aria-valuetext=move || variety_label(value.get())
                    aria-describedby="variety-hint"
                    prop:value=move || index().to_string()
                    on:input=move |ev| {
                        let i = event_target_value(&ev).parse::<u8>().unwrap_or(1);
                        on_change.run(match i {
                            0 => a::Variety::Strict,
                            2 => a::Variety::Diverse,
                            _ => a::Variety::Normal,
                        });
                    }
                />
            </div>
            <span class="variety-label">{move || variety_label(value.get())}</span>
            <span id="variety-hint" class="sr-only">{move || variety_hint(value.get())}</span>
        </div>
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
    let contact = RwSignal::new(Contact::default());
    let generation = StoredValue::new(0u64);
    let translation = RwSignal::new(None::<a::TranslationView>);
    let variety = RwSignal::new(load_variety());
    let set_variety = Callback::new(move |v: a::Variety| {
        variety.set(v);
        save_variety(v);
    });

    let refresh = move || {
        spawn_local(async move {
            match api::status().await {
                Ok(Some(s)) => {
                    status.set(Some(s));
                    auth.set(Auth::Ok);
                    if let Ok(t) = api::translation().await {
                        translation.set(Some(t));
                    }
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
        contact.set(Contact::default());
        map.set(MapState::default());
        searching.set(true);
        stage.set(1);
        let level = variety.get_untracked();
        spawn_local(async move {
            let mut graph = Graph::default();
            let current = move || generation.get_value() == mine;
            let result = api::search(&q, level, |event| {
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

    // Changement de nœud inspecté : aucun contact de l'ancienne fiche ne doit survivre.
    Effect::new(move |_| {
        inspected.get();
        contact.set(Contact::default());
    });
    // Une cible disparue de la carte ne garde pas le contact.
    Effect::new(move |_| {
        map.with(|s| contact.update(|c| c.prune(s)));
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
                    <VarietyControl value=variety on_change=set_variety />
                    <Show when=move || !error.get().is_empty()>
                        <div class="error">{move || error.get()}</div>
                    </Show>
                    <Offer translation=translation hidden=searching />
                </div>
                <div class="content">
                    <div class="map" on:click=move |_| inspected.set(String::new())>
                        <Map state=map selected=selected contact=contact busy=busy on_pick=pick />
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
                        <div class="list" class:hidden=move || !inspected.get().is_empty()
                            inert=move || (!inspected.get().is_empty()).then_some("")
                            aria-hidden=move || if inspected.get().is_empty() { "false" } else { "true" }>
                            <For each={move || hits.get().into_iter().enumerate().collect::<Vec<_>>()}
                                key={|(_, h): &(usize, Hit)| h.doc}
                                children={move |(i, h): (usize, Hit)| view! { <Card hit=h index=i selected=selected contact=contact map=map on_pick=pick /> }}
                            />
                            <Show when=move || stage.get() == 4 && hits.with(Vec::is_empty) && error.get().is_empty()>
                                <div class="empty">"Rien de trouvé"</div>
                            </Show>
                        </div>
                        <Show when=move || !inspected.get().is_empty()>
                            <Inspector map=map hits=hits inspected=inspected selected=selected
                                contact=contact
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

/// Nom français d'une langue (code ISO 639-3).
fn lang_name(code: &str) -> &str {
    match code {
        "eng" => "anglais",
        "deu" => "allemand",
        "spa" => "espagnol",
        "ita" => "italien",
        "nld" => "néerlandais",
        "rus" => "russe",
        "zho" => "chinois",
        other => other,
    }
}

/// Proposition de traduire une source qui publie surtout dans une autre langue (une à la fois ;
/// « Non merci » ne la repropose plus).
#[component]
fn Offer(
    translation: RwSignal<Option<a::TranslationView>>,
    hidden: RwSignal<bool>,
) -> impl IntoView {
    let current = move || {
        translation.with(|t| {
            t.as_ref().filter(|t| t.available).and_then(|t| {
                t.sources
                    .iter()
                    .find(|s| s.state == a::TranslationState::Suggested)
                    .cloned()
            })
        })
    };
    let answer = move |spec: String, accept: bool| {
        spawn_local(async move {
            if let Ok(t) = api::answer_translation(&spec, accept).await {
                translation.set(Some(t));
            }
        });
    };
    view! {
        {move || (!hidden.get()).then(current).flatten().map(|s| {
            let (yes, no) = (s.spec.clone(), s.spec.clone());
            let name = s.spec.rsplit('/').find(|p| !p.is_empty()).unwrap_or(&s.spec).to_owned();
            view! {
                <div class="offer">
                    <span>
                        <b>{name}</b>
                        {format!(" publie surtout en {} ({} % de {} documents). La traduire en français, sur cet ordinateur ?",
                            lang_name(&s.lang), (s.share * 100.0).round(), s.docs)}
                        {(!s.model_ready).then(|| view! { <small>" Modèle de traduction à télécharger (≈ 150 Mo, une fois)."</small> })}
                    </span>
                    <button class="pill" on:click=move |_| answer(yes.clone(), true)>"Traduire"</button>
                    <button class="pill quiet" on:click=move |_| answer(no.clone(), false)>"Non merci"</button>
                </div>
            }
        })}
    }
}

/// Carte de résultat : barre de score, nature, titre, source et date, extrait.
#[component]
fn Card(
    hit: Hit,
    index: usize,
    selected: RwSignal<String>,
    contact: RwSignal<Contact>,
    map: RwSignal<MapState>,
    on_pick: Callback<String>,
) -> impl IntoView {
    let node = format!("d{}", hit.doc);
    let kind = if hit.is_code { "code" } else { "prose" };
    let node_sel = node.clone();
    let node_contact = node.clone();
    let node_focus = node.clone();
    let node_focus_out = node.clone();
    let node_hover = node.clone();
    let node_hover_out = node.clone();
    let node_key = node.clone();
    let bar = format!(
        "height: calc((100% - 24px) * {:.3}); background: var(--{kind}); animation-delay: {}ms",
        hit.relative.max(0.08),
        (index * 30).min(300)
    );
    let meta = format!(
        "{}{}{}{}",
        hit.source,
        if hit.date > 0 {
            format!("  ·  {}", ago(hit.date))
        } else {
            String::new()
        },
        hit.code_lang
            .as_deref()
            .map(|l| format!("  ·  {l}"))
            .unwrap_or_default(),
        hit.translated_from
            .as_deref()
            .map(|l| format!("  ·  traduit de l'{}", lang_name(l)))
            .unwrap_or_default()
    );
    let snippet = highlight(&hit.snippet, &hit.matched);
    let title = hit.title.clone().unwrap_or_else(|| hit.source.clone());
    let aria = format!("{title} — {}", hit.source);
    view! {
        <div class=format!("card {kind}") class:selected=move || selected.get() == node_sel
            class:contact=move || contact.with(|c| map.with(|m| c.nodes(m).contains(&node_contact)))
            tabindex="0" role="button" aria-label=aria
            style=format!("animation-delay: {}ms", (index * 30).min(300))
            on:click=move |_| on_pick.run(node.clone())
            on:mouseenter=move |_| contact.update(|c| c.set_pointer_node(node_hover.clone()))
            on:mouseleave=move |_| contact.update(|c| c.clear_pointer_node(&node_hover_out))
            on:focus=move |_| contact.update(|c| c.set_focus_node(node_focus.clone()))
            on:blur=move |_| contact.update(|c| c.clear_focus_node(&node_focus_out))
            on:keydown=move |ev| {
                if ev.key() == "Enter" || ev.key() == " " {
                    ev.prevent_default();
                    on_pick.run(node_key.clone());
                }
            }
            on:dblclick={
                let url = hit.url.clone();
                move |_| if let (Some(u), Some(w)) = (url.as_deref(), web_sys::window()) {
                    let _ = w.open_with_url_and_target(u, "_blank");
                }
            }>
            <div class="bar" style=bar></div>
            <h3><span class=format!("glyph {kind}")></span>{title}</h3>
            <div class="src">{meta}</div>
            <p inner_html=snippet></p>
        </div>
    }
}

/// Un morceau du passage : texte brut, ou mot de la requête.
#[derive(Debug, PartialEq)]
struct Segment {
    text: String,
    matched: bool,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Découpe le passage aux limites de mot, en conservant exactement espaces et retours.
fn passage_segments(text: &str, words: &[String]) -> Vec<Segment> {
    let lower = text.to_lowercase();
    // Minuscules d'une autre longueur : positions incomparables, pas de surlignage.
    if lower.len() != text.len() {
        return vec![Segment {
            text: text.to_owned(),
            matched: false,
        }];
    }
    let mut terms: Vec<String> = words
        .iter()
        .filter(|w| w.chars().count() > 1)
        .map(|w| w.to_lowercase())
        .collect();
    terms.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    terms.dedup();
    if terms.is_empty() {
        return vec![Segment {
            text: text.to_owned(),
            matched: false,
        }];
    }
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < text.len() {
        let previous_word = text[..i].chars().next_back().is_some_and(is_word_char);
        let mut hit: Option<&String> = None;
        if !previous_word {
            for term in &terms {
                let Some(tail) = lower.get(i..) else { continue };
                if !tail.starts_with(term.as_str()) {
                    continue;
                }
                let end = i + term.len();
                if !text.is_char_boundary(end)
                    || text[end..].chars().next().is_some_and(is_word_char)
                {
                    continue;
                }
                if hit.is_none_or(|h| term.len() > h.len()) {
                    hit = Some(term);
                }
            }
        }
        if let Some(term) = hit {
            if !plain.is_empty() {
                out.push(Segment {
                    text: std::mem::take(&mut plain),
                    matched: false,
                });
            }
            out.push(Segment {
                text: text[i..i + term.len()].to_owned(),
                matched: true,
            });
            i += term.len();
        } else {
            let c = text[i..].chars().next().unwrap_or(' ');
            plain.push(c);
            i += c.len_utf8();
        }
    }
    if !plain.is_empty() {
        out.push(Segment {
            text: plain,
            matched: false,
        });
    }
    out
}

/// Lien `term_hit` correspondant à un mot du passage, le plus lourd s'il y a ambiguïté.
fn link_for<'a>(links: &'a [LinkView], word: &str) -> Option<&'a LinkView> {
    let word = word.to_lowercase();
    links
        .iter()
        .filter(|l| l.kind == "term_hit" && l.other.label.to_lowercase() == word)
        .max_by(|a, b| a.weight.total_cmp(&b.weight))
}

/// Carte de passage : texte trouvé, mots liés au contact, défilement et clavier.
#[component]
fn Passage(hit: Hit, links: Vec<LinkView>, contact: RwSignal<Contact>) -> impl IntoView {
    let text = if hit.passage_text.is_empty() {
        hit.snippet.clone()
    } else {
        hit.passage_text.clone()
    };
    let is_code = hit.is_code;
    let meta = format!(
        "{}{}{}{}",
        hit.source,
        if hit.date > 0 {
            format!("  ·  {}", ago(hit.date))
        } else {
            String::new()
        },
        hit.code_lang
            .as_deref()
            .map(|l| format!("  ·  {l}"))
            .unwrap_or_default(),
        hit.translated_from
            .as_deref()
            .map(|l| format!("  ·  traduit de l'{}", lang_name(l)))
            .unwrap_or_default()
    );
    let segments = passage_segments(&text, &hit.matched);
    let interactive = !is_code;
    let inner = segments
        .into_iter()
        .map(|seg| {
            if !seg.matched {
                return view! { <span>{seg.text}</span> }.into_any();
            }
            match link_for(&links, &seg.text) {
                Some(link) if interactive => {
                    let hot = link.clone();
                    let edge_in = link.edge_id.clone();
                    let edge_out = link.edge_id.clone();
                    let word = seg.text;
                    view! {
                        <mark
                            class=move || if contact.with(|c| c.hot_link(&hot)) { "linked hot" } else { "linked" }
                            on:mouseenter=move |_| contact.update(|c| c.set_pointer_edge(edge_in.clone()))
                            on:mouseleave=move |_| contact.update(|c| c.clear_pointer_edge(&edge_out))
                        >{word}</mark>
                    }
                    .into_any()
                }
                _ => view! { <mark>{seg.text}</mark> }.into_any(),
            }
        })
        .collect_view();
    let content = if is_code {
        view! { <pre class="passage-text code">{inner}</pre> }.into_any()
    } else {
        view! { <p class="passage-text">{inner}</p> }.into_any()
    };
    let body_ref = NodeRef::<leptos::html::Div>::new();
    let body_effect = body_ref;
    Effect::new(move |_| {
        if is_code {
            return;
        }
        let Some(el) = body_effect.get() else { return };
        let Ok(scroller) = el.dyn_into::<web_sys::HtmlElement>() else {
            return;
        };
        if let Ok(Some(first)) = scroller.query_selector("mark")
            && let Ok(mark) = first.dyn_into::<web_sys::HtmlElement>()
        {
            let top = mark.offset_top() - scroller.client_height() / 3;
            scroller.set_scroll_top(top.max(0));
        }
    });
    let body_keys = body_ref;
    view! {
        <section class="passage" tabindex="0" role="region" aria-label="Passage trouvé"
            on:keydown=move |ev| {
                let Some(el) = body_keys.get() else { return };
                let Ok(scroller) = el.dyn_into::<web_sys::HtmlElement>() else {
                    return;
                };
                let page = scroller.client_height().max(48);
                let max = (scroller.scroll_height() - scroller.client_height()).max(0);
                let next = match ev.key().as_str() {
                    "ArrowDown" => scroller.scroll_top() + 24,
                    "ArrowUp" => scroller.scroll_top() - 24,
                    "PageDown" => scroller.scroll_top() + page,
                    "PageUp" => scroller.scroll_top() - page,
                    "Home" => 0,
                    "End" => max,
                    _ => return,
                };
                ev.prevent_default();
                scroller.set_scroll_top(next.clamp(0, max));
            }>
            <div class="passage-head">{meta}</div>
            <div class="passage-body" node_ref=body_ref>
                {content}
            </div>
        </section>
    }
}

/// Inspecteur d'un nœud : nature, liens par poids, actions, passage du résultat.
#[component]
fn Inspector(
    map: RwSignal<MapState>,
    hits: RwSignal<Vec<Hit>>,
    inspected: RwSignal<String>,
    selected: RwSignal<String>,
    contact: RwSignal<Contact>,
    on_search: Callback<String>,
) -> impl IntoView {
    let node = move || map.with(|m| m.nodes.get(&inspected.get()).cloned());
    let hit = move || {
        let id = inspected.get();
        hits.with(|h| h.iter().find(|h| format!("d{}", h.doc) == id).cloned())
    };
    let links = move || map.with(|m| m.links(&inspected.get()));
    let url = move || hit().and_then(|h| h.url);
    view! {
        <div class="inspector">
            {move || node().map(|n| {
                let kind = n.kind;
                let label = if n.label.is_empty() { "requête".to_owned() } else { n.label.clone() };
                let word = n.label.clone();
                let hover_in = n.id.clone();
                let hover_out = n.id.clone();
                view! {
                    <header>
                        <span class=format!("glyph {kind}")></span>
                        <div class="insp-title"
                            on:mouseenter=move |_| contact.update(|c| c.set_pointer_node(hover_in.clone()))
                            on:mouseleave=move |_| contact.update(|c| c.clear_pointer_node(&hover_out))>
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
            {move || hit()
                .filter(|h| !(h.passage_text.is_empty() && h.snippet.is_empty()))
                .map(|h| view! { <Passage hit=h links=links() contact=contact /> })}
            <div class="links">
                {move || links().into_iter().map(|l| {
                    let label = if l.other.label.is_empty() { "requête".to_owned() } else { l.other.label.clone() };
                    let color = match l.other.kind { "query" => "accent", "related" => "prose", k => k };
                    let hot = l.clone();
                    let mouse_in = l.edge_id.clone();
                    let mouse_out = l.edge_id.clone();
                    let focus_in = l.edge_id.clone();
                    let focus_out = l.edge_id.clone();
                    let sel = l.other.id.clone();
                    let insp = l.other.id.clone();
                    let aria = format!("Lien vers {label}, {}, poids {} %",
                        kind_label(l.other.kind), (l.weight * 100.0).round());
                    view! {
                        <button type="button" class="link" class:hot=move || contact.with(|c| c.hot_link(&hot))
                            aria-label=aria
                            on:click=move |_| { selected.set(sel.clone()); inspected.set(insp.clone()); }
                            on:mouseenter=move |_| contact.update(|c| c.set_pointer_edge(mouse_in.clone()))
                            on:mouseleave=move |_| contact.update(|c| c.clear_pointer_edge(&mouse_out))
                            on:focus=move |_| contact.update(|c| c.set_focus_edge(focus_in.clone()))
                            on:blur=move |_| contact.update(|c| c.clear_focus_edge(&focus_out))>
                            <span class=format!("glyph {}", l.other.kind) style="flex:none"></span>
                            <span>{label}</span>
                            <div class="weight"><i style=format!("width: {:.0}%; background: var(--{color})", (l.weight.max(0.04) * 100.0))></i></div>
                        </button>
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
    use super::passage_segments;
    use super::{variety_from_key, variety_key, variety_label};
    use nknown_api::Variety;

    #[test]
    fn highlight_escapes_then_marks() {
        assert_eq!(
            highlight("Le <b>Jeton</b> du jeton", &["jeton".into()]),
            "Le &lt;b&gt;<mark>Jeton</mark>&lt;/b&gt; du <mark>jeton</mark>"
        );
        // Un mot d'une lettre n'est pas surligné.
        assert_eq!(highlight("a b", &["a".into()]), "a b");
    }

    #[test]
    fn passage_segments_preserve_text_and_word_boundaries() {
        let split = |text: &str, words: &[&str]| {
            let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
            let segments = passage_segments(text, &words);
            let joined: String = segments.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(joined, text, "le texte doit être conservé à l'identique");
            segments
        };
        let segments = split("Le <b>Jeton</b> du jeton", &["jeton"]);
        assert_eq!(segments.iter().filter(|s| s.matched).count(), 2);

        // Pas de correspondance partielle dans « part » ni dans « arts ».
        let segments = split("part arts art, art.", &["art"]);
        let hits: Vec<&str> = segments
            .iter()
            .filter(|s| s.matched)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["art", "art"]);

        // Accents et ponctuation.
        let segments = split("Élection, élection !", &["élection"]);
        assert_eq!(segments.iter().filter(|s| s.matched).count(), 2);

        // Cyrillique et CJK : les limites de mot restent Unicode.
        let segments = split("Привет, мир! 漢字 テスト", &["мир", "漢字"]);
        let hits: Vec<&str> = segments
            .iter()
            .filter(|s| s.matched)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["мир", "漢字"]);

        // Code : retours, tabulations et espaces restent intacts.
        let code = "fn main() {\n\tlet имя = \"漢字\";\n\tprintln!(\"{}\", имя);\n}";
        let segments = split(code, &["имя", "漢字"]);
        assert_eq!(segments.iter().filter(|s| s.matched).count(), 3);
    }

    #[test]
    fn variety_keys_roundtrip() {
        for v in [Variety::Strict, Variety::Normal, Variety::Diverse] {
            assert_eq!(variety_from_key(variety_key(v)), v);
            assert!(!variety_label(v).is_empty());
        }
        // Valeur inconnue ou ancienne : le comportement historique.
        assert_eq!(variety_from_key("inconnu"), Variety::Normal);
        assert_eq!(variety_label(Variety::Diverse), "Diversifié");
    }
}

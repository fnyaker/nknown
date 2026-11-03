// La carte des résultats (charte, section 4). Les positions viennent de Rust (nknown-graph) ;
// ici, on anime : un nœud nouveau éclôt depuis le centre, un lien nouveau se tend depuis son
// origine, un nœud écarté s'efface. Nœuds et extrémités de liens suivent le même ressort, d'où
// des liens qui restent accrochés à leurs nœuds pendant le mouvement.
import QtQuick
import QtQuick.Shapes

Item {
    id: view
    clip: true

    /// Nœud mis en valeur (ses chemins s'allument, le reste passe à 25 %).
    property string selected: ""
    /// Recherche en cours : la requête pulse.
    property bool busy: false
    signal nodeClicked(var node)
    /// Double clic sur un résultat : le lire.
    signal nodeDoubleClicked(var node)
    signal backgroundClicked()

    // Caméra : déplacement et zoom ; recadrée sur la carte tant qu'on n'y a pas touché.
    property real panX: 0
    property real panY: 0
    property real zoom: 1
    property bool userMoved: false
    Behavior on panX { enabled: !drag.active; SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.2 } }
    Behavior on panY { enabled: !drag.active; SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.2 } }
    Behavior on zoom { enabled: !wheelActive; SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.002 } }
    property bool wheelActive: false

    ListModel { id: nodeModel }
    ListModel { id: edgeModel }
    /// id → données du nœud (inspecteur) ; id → voisins.
    property var nodes: ({})
    property var neighbors: ({})
    property var lit: ({})
    property var bounds: ({ x0: -1, y0: -1, x1: 1, y1: 1 })

    function clear() {
        nodeModel.clear()
        edgeModel.clear()
        nodes = {}
        neighbors = {}
        selected = ""
        userMoved = false
        panX = 0; panY = 0; zoom = 1
    }

    function indexOf(model, id) {
        for (let i = 0; i < model.count; i++)
            if (model.get(i).uid === id) return i
        return -1
    }

    /// Nouvel état de la carte (JSON de Rust) : ajouts, mises à jour, retraits animés.
    function apply(map) {
        const seenNodes = {}
        const byId = {}
        let x0 = 0, y0 = 0, x1 = 0, y1 = 0
        for (const n of map.nodes) {
            seenNodes[n.id] = true
            byId[n.id] = n
            x0 = Math.min(x0, n.x - n.r); y0 = Math.min(y0, n.y - n.r)
            x1 = Math.max(x1, n.x + n.r); y1 = Math.max(y1, n.y + n.r)
            const row = { uid: n.id, kind: n.kind, label: n.label, weight: n.weight,
                          tx: n.x, ty: n.y, r: n.r, hit: n.hit, gone: false }
            const i = indexOf(nodeModel, n.id)
            if (i < 0) nodeModel.append(row)
            else nodeModel.set(i, row)
        }
        for (let i = 0; i < nodeModel.count; i++)
            if (!seenNodes[nodeModel.get(i).uid]) nodeModel.setProperty(i, "gone", true)

        const seenEdges = {}
        const adj = {}
        for (const e of map.edges) {
            seenEdges[e.id] = true
            ;(adj[e.from] = adj[e.from] || []).push({ id: e.to, kind: e.kind, weight: e.weight })
            ;(adj[e.to] = adj[e.to] || []).push({ id: e.from, kind: e.kind, weight: e.weight })
            const row = { uid: e.id, from: e.from, to: e.to, kind: e.kind, weight: e.weight,
                          x1: e.x1, y1: e.y1, x2: e.x2, y2: e.y2, gone: false }
            const i = indexOf(edgeModel, e.id)
            if (i < 0) edgeModel.append(row)
            else edgeModel.set(i, row)
        }
        for (let i = 0; i < edgeModel.count; i++)
            if (!seenEdges[edgeModel.get(i).uid]) edgeModel.setProperty(i, "gone", true)

        nodes = byId
        neighbors = adj
        bounds = { x0: x0, y0: y0, x1: x1, y1: y1 }
        sweep.restart()
        updateLit()
        if (!userMoved) fit()
    }

    /// Cadre toute la carte (zoom ≤ 1 : on ne grossit pas une petite carte).
    function fit() {
        const w = bounds.x1 - bounds.x0 + 60, h = bounds.y1 - bounds.y0 + 60
        zoom = Math.min(1.0, view.width / w, view.height / h)
        panX = -(bounds.x0 + bounds.x1) / 2 * zoom
        panY = -(bounds.y0 + bounds.y1) / 2 * zoom
    }

    /// Met un nœud en valeur et y amène la caméra.
    function focusNode(id) {
        selected = id
        updateLit()
        const n = nodes[id]
        if (!n) return
        userMoved = true
        zoom = Math.max(zoom, 0.9)
        panX = -n.x * zoom
        panY = -n.y * zoom
    }

    /// Chemin allumé : le nœud, ses voisins et, pour un résultat, les mots et la requête qui y
    /// mènent.
    function updateLit() {
        const l = {}
        if (selected !== "") {
            l[selected] = true
            for (const nb of (neighbors[selected] || [])) {
                l[nb.id] = true
                if (nb.kind === "term_hit" || nb.kind === "qualifies")
                    for (const up of (neighbors[nb.id] || []))
                        if (up.kind === "query_term") l[up.id] = true
            }
        }
        lit = l
    }

    /// Liens d'un nœud, du plus lourd au plus léger (inspecteur).
    function linksOf(id) {
        return (neighbors[id] || [])
            .map(nb => ({ id: nb.id, kind: nb.kind, weight: nb.weight, node: nodes[nb.id] }))
            .filter(l => l.node)
            .sort((a, b) => b.weight - a.weight)
    }

    // Retrait des éléments effacés, une fois leur sortie jouée.
    Timer {
        id: sweep
        interval: Theme.standard
        onTriggered: {
            for (let i = nodeModel.count - 1; i >= 0; i--) if (nodeModel.get(i).gone) nodeModel.remove(i)
            for (let i = edgeModel.count - 1; i >= 0; i--) if (edgeModel.get(i).gone) edgeModel.remove(i)
        }
    }

    // ------------------------------------------------------------------ interactions
    TapHandler {
        onTapped: view.backgroundClicked()
        onDoubleTapped: { view.userMoved = false; view.fit() }
    }
    DragHandler {
        id: drag
        target: null
        property real startX: 0
        property real startY: 0
        onActiveChanged: if (active) { startX = view.panX; startY = view.panY; view.userMoved = true }
        onTranslationChanged: if (active) { view.panX = startX + translation.x; view.panY = startY + translation.y }
    }
    WheelHandler {
        acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
        onWheel: (ev) => {
            view.wheelActive = true
            const old = view.zoom
            const next = Math.max(0.25, Math.min(3.0, old * Math.pow(1.0015, ev.angleDelta.y)))
            // Zoom centré sur le pointeur : le point sous le curseur ne bouge pas.
            const cx = ev.x - view.width / 2, cy = ev.y - view.height / 2
            view.panX = cx - (cx - view.panX) * next / old
            view.panY = cy - (cy - view.panY) * next / old
            view.zoom = next
            view.userMoved = true
            view.wheelActive = false
        }
    }

    // ------------------------------------------------------------------ monde
    Item {
        id: world
        x: view.width / 2 + view.panX
        y: view.height / 2 + view.panY
        scale: view.zoom
        transformOrigin: Item.TopLeft

        Repeater {
            model: edgeModel
            delegate: Shape {
                id: edge
                required property string uid
                required property string from
                required property string to
                required property string kind
                required property real weight
                required property real x1
                required property real y1
                required property real x2
                required property real y2
                required property bool gone
                readonly property bool dimmed: view.selected !== "" && !(view.lit[from] && view.lit[to])
                property real ax: x1
                property real ay: y1
                property real bx: x1
                property real by: y1
                Behavior on ax { SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.1 } }
                Behavior on ay { SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.1 } }
                Behavior on bx { SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.1 } }
                Behavior on by { SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.1 } }
                // Point de contrôle : le milieu, éloigné du centre (d'autant plus que le lien est
                // long), ou décalé de côté s'il passe trop près du centre.
                readonly property real mx: (ax + bx) / 2
                readonly property real my: (ay + by) / 2
                readonly property real bend: kind === "query_term" ? 0
                    : Math.min(0.45, Math.hypot(bx - ax, by - ay) / 900)
                readonly property real mlen: Math.max(1, Math.hypot(mx, my))
                readonly property real cx: mlen > 30 ? mx * (1 + bend) : mx - (by - ay) * bend
                readonly property real cy: mlen > 30 ? my * (1 + bend) : my + (bx - ax) * bend
                // Le lien se tend depuis son origine.
                Component.onCompleted: {
                    ax = Qt.binding(() => x1); ay = Qt.binding(() => y1)
                    bx = Qt.binding(() => x2); by = Qt.binding(() => y2)
                }
                preferredRendererType: Shape.CurveRenderer
                // Hiérarchie : la requête et ses mots nets, les liens mot → résultat selon leur
                // poids (les négligeables ne sont pas tracés), sources et similarités en retrait.
                visible: !(kind === "term_hit" && weight < 0.12)
                readonly property real strength: kind === "query_term" ? 0.7
                    : kind === "term_hit" ? 0.15 + 0.7 * weight
                    : kind === "qualifies" ? 0.9
                    : kind === "hit_source" ? 0.3
                    : 0.18 + 0.3 * weight
                opacity: gone ? 0 : strength * (dimmed ? Theme.dim : 1)
                Behavior on opacity { NumberAnimation { duration: edge.gone ? Theme.quick : Theme.standard } }
                ShapePath {
                    strokeColor: edge.kind === "query_term" ? Theme.kindColor("query")
                               : edge.kind === "qualifies" ? Theme.kindColor("qualifier")
                               : edge.kind === "hit_source" ? Theme.kindColor("source")
                               : edge.kind === "similar" ? Theme.kindColor("related")
                               : Theme.kindColor("term")
                    strokeWidth: edge.kind === "query_term" ? 1.2
                               : edge.kind === "hit_source" || edge.kind === "similar" ? 1
                               : 0.5 + 3.5 * edge.weight
                    strokeStyle: edge.kind === "hit_source" || edge.kind === "similar" ? ShapePath.DashLine : ShapePath.SolidLine
                    dashPattern: edge.kind === "hit_source" ? [1, 3] : [4, 3]
                    fillColor: "transparent"
                    capStyle: ShapePath.RoundCap
                    startX: edge.ax
                    startY: edge.ay
                    // Liens incurvés vers l'extérieur : ils contournent le centre au lieu de le
                    // traverser. Requête → mot : droit.
                    PathQuad {
                        x: edge.bx
                        y: edge.by
                        controlX: edge.cx
                        controlY: edge.cy
                    }
                }
            }
        }

        Repeater {
            model: nodeModel
            delegate: Item {
                id: node
                required property string uid
                required property string kind
                required property string label
                required property real weight
                required property real tx
                required property real ty
                required property real r
                required property int hit
                required property bool gone
                readonly property bool isSelected: view.selected === uid
                readonly property bool dimmed: view.selected !== "" && !view.lit[uid]
                // Étiquette : toujours pour les mots et les sources ; pour un résultat, au survol,
                // à la sélection, ou s'il est parmi les trois premiers.
                readonly property bool labelled: kind === "term" || kind === "qualifier" || kind === "source"
                    || hover.containsMouse || isSelected || (hit >= 0 && hit < 3)

                property real cx: 0
                property real cy: 0
                Behavior on cx { SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.1 } }
                Behavior on cy { SpringAnimation { spring: Theme.springStiffness; damping: Theme.springDamping; epsilon: 0.1 } }
                // Éclosion depuis la requête.
                Component.onCompleted: { cx = Qt.binding(() => tx); cy = Qt.binding(() => ty); born = true }
                property bool born: false

                x: cx - r
                y: cy - r
                width: 2 * r
                height: 2 * r
                z: isSelected ? 3 : (kind === "query" ? 2 : 1)
                opacity: gone || !born ? 0 : (dimmed ? Theme.dim : 1)
                Behavior on opacity { NumberAnimation { duration: node.gone ? Theme.quick : Theme.standard } }
                scale: hover.containsMouse ? 1.15 : 1
                Behavior on scale { NumberAnimation { duration: Theme.quick; easing.type: Easing.OutCubic } }
                Behavior on width { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }

                // Halo de sélection (la lueur ne sert qu'au focus et à la progression).
                Rectangle {
                    anchors.centerIn: parent
                    visible: node.isSelected
                    width: parent.width + 14
                    height: width
                    radius: width / 2
                    color: "transparent"
                    border.color: Qt.alpha(Theme.accent, 0.55)
                    border.width: 2
                }
                KindGlyph {
                    id: glyph
                    anchors.fill: parent
                    kind: node.kind
                    size: parent.width
                }
                // La requête pulse pendant la recherche.
                SequentialAnimation on scale {
                    running: node.kind === "query" && view.busy
                    loops: Animation.Infinite
                    alwaysRunToEnd: true
                    NumberAnimation { to: 1.12; duration: 520; easing.type: Easing.InOutSine }
                    NumberAnimation { to: 1.0; duration: 520; easing.type: Easing.InOutSine }
                }
                Text {
                    visible: node.labelled && node.label !== ""
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.top: parent.bottom
                    anchors.topMargin: 5
                    width: Math.min(implicitWidth, 180)
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                    text: node.label
                    color: node.isSelected || node.kind === "term" || node.kind === "qualifier"
                           ? Theme.textPrimary : Theme.textSecondary
                    font.family: Theme.fontUi
                    font.pixelSize: node.kind === "term" || node.kind === "qualifier" ? 13 : 11
                    font.weight: node.kind === "term" ? Font.DemiBold : Font.Medium
                    style: Text.Outline
                    styleColor: Theme.bgBase
                }
                // Le nœud garde son clic : le fond (désélection, recadrage) ne le voit pas.
                MouseArea {
                    id: hover
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: view.nodeClicked(view.nodes[node.uid])
                    onDoubleClicked: view.nodeDoubleClicked(view.nodes[node.uid])
                }
                Accessible.role: Accessible.Button
                Accessible.name: Theme.kindName(kind) + " " + label
            }
        }
    }
}

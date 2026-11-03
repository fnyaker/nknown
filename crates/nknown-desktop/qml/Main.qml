// Fenêtre principale (charte, section 6). Accueil : le champ seul, centré. Recherche lancée :
// le champ glisse vers le haut, la carte (≈ 62 %) et la liste (≈ 38 %) prennent la place.
// Échap : referme l'inspecteur, puis désélectionne, puis revient à l'accueil.
import QtQuick
import QtQuick.Controls.Basic
import org.nknown.app

ApplicationWindow {
    id: win
    width: 1360
    height: 860
    minimumWidth: 900
    minimumHeight: 560
    visible: true
    title: "NKnown"
    color: Theme.bgBase
    font.family: Theme.fontUi

    property bool searching: false
    property string phase: ""
    property string intent: ""
    property var hits: []
    property real tookMs: 0
    property string error: ""
    property var engineState: ({})
    /// Nœud ouvert dans l'inspecteur.
    property var inspected: null
    /// Nœud à mettre en valeur dès que les résultats arrivent.
    property string pendingSelect: ""

    function launch(query) {
        error = ""
        hits = []
        intent = ""
        inspected = null
        map.clear()
        searching = true
        phase = "analyze"
        backend.search(query)
    }

    function hitIndexOf(nodeId) {
        for (let i = 0; i < hits.length; i++) if (hits[i].node === nodeId) return i
        return -1
    }

    /// Met un nœud en valeur sur la carte et, si c'est un résultat, dans la liste.
    function select(nodeId) {
        map.focusNode(nodeId)
        const i = hitIndexOf(nodeId)
        if (i >= 0) list.positionViewAtIndex(i, ListView.Contain)
    }

    function inspect(node) {
        if (!node) return
        inspected = node
        select(node.id)
    }

    function back() {
        if (inspected) inspected = null
        else if (map.selected !== "") { map.selected = ""; map.updateLit() }
        else if (searching) { searching = false; phase = ""; top.text = ""; top.focusInput() }
    }

    Backend {
        id: backend
        onStatusChanged: { try { win.engineState = JSON.parse(status) } catch (e) {} }
        onPhase: (name) => win.phase = name
        onTerms: (json) => { const t = JSON.parse(json); win.intent = t.intent || "" }
        onMap: (json) => map.apply(JSON.parse(json))
        onResults: (json, took) => {
            win.hits = JSON.parse(json)
            win.tookMs = took
            // Résultat demandé au lancement (KRunner « Afficher dans NKnown »).
            if (win.pendingSelect !== "" && map.nodes[win.pendingSelect])
                win.inspect(map.nodes[win.pendingSelect])
            win.pendingSelect = ""
        }
        // Demande de lancement : la sienne, puis celles des lancements suivants.
        onRequestChanged: {
            let r
            try { r = JSON.parse(request) } catch (e) { return }
            win.show(); win.raise(); win.requestActivate()
            if (r.query) {
                win.pendingSelect = r.select || ""
                top.text = r.query
                win.launch(r.query)
            }
        }
        onFailed: (message) => { win.error = message; win.phase = "done" }
        Component.onCompleted: start()
    }

    Shortcut { sequence: "Esc"; onActivated: win.back() }
    Shortcut { sequences: ["Ctrl+L", "Ctrl+F", "/"]; onActivated: top.focusInput() }

    // Toute l'interface, dans un élément QML (capturable, voir plus bas).
    Item {
        id: frame
        anchors.fill: parent
        Rectangle { anchors.fill: parent; color: Theme.bgBase }

        // ------------------------------------------------------------------ en-tête
        // Logo : seulement à l'accueil.
        Column {
            anchors.horizontalCenter: parent.horizontalCenter
            y: top.y - height - 36
            spacing: 10
            opacity: win.searching ? 0 : 1
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Theme.standard } }
            KindGlyph { anchors.horizontalCenter: parent.horizontalCenter; kind: "query"; size: 44 }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "NKnown"
                color: Theme.textPrimary
                font.family: Theme.fontUi
                font.pixelSize: 22
                font.weight: Font.Light
                font.letterSpacing: 2
            }
        }

        SearchField {
            id: top
            compact: win.searching
            phase: win.phase
            intent: win.intent
            width: Math.min(win.width - 2 * Theme.margin, win.searching ? 760 : 620)
            anchors.horizontalCenter: parent.horizontalCenter
            y: win.searching ? 18 : win.height * 0.42
            Behavior on y { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
            Behavior on width { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
            onSubmitted: (query) => win.launch(query)
        }

        // Erreur : une ligne sous le champ, couleur danger (seul usage de cette couleur).
        Text {
            anchors.horizontalCenter: top.horizontalCenter
            anchors.top: top.bottom
            anchors.topMargin: 10
            visible: win.error !== ""
            text: win.error
            color: Theme.danger
            font.pixelSize: 12
        }

        // ------------------------------------------------------------------ carte et liste
        Item {
            id: content
            anchors.top: top.bottom
            anchors.topMargin: 20
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            opacity: win.searching ? 1 : 0
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Theme.emphasis } }

            MapView {
                id: map
                anchors.left: parent.left
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: parent.width * 0.62
                busy: win.phase !== "" && win.phase !== "done"
                onNodeClicked: (node) => win.inspect(node)
                onBackgroundClicked: { win.inspected = null }
            }

            Item {
                id: side
                anchors.left: map.right
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                anchors.rightMargin: Theme.margin
                anchors.bottomMargin: Theme.margin

                // Compte et durée : une métadonnée discrète.
                Text {
                    id: meta
                    visible: win.phase === "done" && win.hits.length > 0 && !win.inspected
                    text: win.hits.length + (win.hits.length > 1 ? " résultats" : " résultat")
                          + "  ·  " + (win.tookMs < 1 ? "< 1" : Math.round(win.tookMs)) + " ms"
                    color: Theme.textSecondary
                    font.pixelSize: 11
                    font.weight: Font.Medium
                    font.features: { "tnum": 1 }
                }

                ListView {
                    id: list
                    anchors.top: meta.bottom
                    anchors.topMargin: 10
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    clip: true
                    spacing: 8
                    model: win.hits
                    boundsBehavior: Flickable.StopAtBounds
                    // L'inspecteur prend la place de la liste.
                    opacity: win.inspected ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: Theme.quick } }
                    delegate: ResultCard {
                        required property var modelData
                        required property int index
                        hit: modelData
                        width: ListView.view.width
                        selected: map.selected === modelData.node
                        onPicked: win.inspect(map.nodes[modelData.node])
                        onOpen: if (modelData.url) Qt.openUrlExternally(modelData.url)
                        // Entrées décalées de 30 ms, plafonnées à 300 ms.
                        opacity: 0
                        Component.onCompleted: appear.start()
                        NumberAnimation on opacity {
                            id: appear
                            running: false
                            from: 0; to: 1
                            duration: Theme.standard
                            easing.type: Easing.OutCubic
                        }
                        Timer {
                            interval: Math.min(index * Theme.stagger, 300)
                            running: true
                            onTriggered: appear.start()
                        }
                    }
                }

                Text {
                    anchors.centerIn: parent
                    visible: win.phase === "done" && win.hits.length === 0 && win.error === ""
                    text: "Rien de trouvé"
                    color: Theme.textSecondary
                    font.pixelSize: 13
                }

                Inspector {
                    id: inspector
                    anchors.fill: parent
                    node: win.inspected
                    links: win.inspected ? map.linksOf(win.inspected.id) : []
                    url: {
                        if (!win.inspected) return ""
                        const i = win.hitIndexOf(win.inspected.id)
                        return i >= 0 && win.hits[i].url ? win.hits[i].url : ""
                    }
                    opacity: win.inspected ? 1 : 0
                    visible: opacity > 0
                    scale: win.inspected ? 1 : 0.98
                    Behavior on opacity { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
                    onCloseRequested: win.inspected = null
                    onLinkPicked: (id) => win.inspect(map.nodes[id])
                    onSearchWord: (word) => { top.text = word; win.launch(word) }
                }
            }
        }

        // ------------------------------------------------------------------ état du moteur
        // Un point discret à l'accueil : vert à jour, animé en collecte ou au téléchargement des
        // modèles, rouge si le moteur est injoignable.
        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 22
            spacing: 8
            opacity: win.searching ? 0 : 1
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Theme.standard } }

            readonly property var st: win.engineState.status || null
            readonly property bool down: !win.engineState.engine || win.engineState.engine === "none" || !!win.engineState.error
            readonly property bool downloading: !!st && !!st.models && st.models.state === "downloading"
            readonly property bool busy: downloading || (!!st && st.collecting)

            Rectangle {
                id: dot
                anchors.verticalCenter: parent.verticalCenter
                width: 8
                height: 8
                radius: 4
                color: parent.down ? Theme.danger : parent.busy ? Theme.accent : Theme.ok
                SequentialAnimation on opacity {
                    running: dot.parent.busy
                    loops: Animation.Infinite
                    NumberAnimation { to: 0.3; duration: 700; easing.type: Easing.InOutSine }
                    NumberAnimation { to: 1.0; duration: 700; easing.type: Easing.InOutSine }
                }
            }
            // Texte seulement quand il apporte quelque chose (premier lancement, panne).
            Text {
                anchors.verticalCenter: parent.verticalCenter
                visible: parent.downloading || parent.down || dotHover.hovered
                color: Theme.textSecondary
                font.pixelSize: 11
                font.features: { "tnum": 1 }
                text: {
                    const p = parent
                    if (p.down) return win.engineState.error || "moteur injoignable"
                    const s = p.st
                    if (p.downloading)
                        return "Préparation des modèles · " + Math.floor(100 * s.models.done / Math.max(1, s.models.total)) + " %"
                    if (!s) return ""
                    return s.docs + " documents · " + (win.engineState.engine === "daemon" ? "démon" : "intégré")
                           + (s.collecting ? " · collecte" : "")
                }
            }
            HoverHandler { id: dotHover }
        }
    }

    Component.onCompleted: top.focusInput()

    // ------------------------------------------------------------------ captures pilotées
    // NKNOWN_APP_SNAPSHOT=<préfixe> [NKNOWN_APP_QUERY=<requête>] : accueil, recherche,
    // inspecteur, enregistrés en PNG, puis l'app se ferme (essais visuels, documentation).
    property var auto: null
    property int autoStep: 0
    Connections {
        target: backend
        function onAutomationChanged() { try { win.auto = JSON.parse(backend.automation) } catch (e) {} }
    }
    function snap(name) {
        const ok = frame.grabToImage(r => {
            const path = win.auto.snapshot + "-" + name + ".png"
            console.log("capture", path, r.saveToFile(path))
        })
        if (!ok) console.warn("capture impossible :", name)
    }
    Timer {
        running: !!win.auto
        interval: 1800
        repeat: true
        onTriggered: {
            const q = win.auto.query
            switch (win.autoStep++) {
            case 0: win.snap("1-accueil"); if (q) { top.text = q; win.launch(q) } break
            case 1: break
            case 2: win.snap("2-recherche"); break
            case 3: if (win.hits.length > 0) win.inspect(map.nodes[win.hits[0].node]); break
            case 4: win.snap("3-inspecteur"); break
            default: Qt.quit()
            }
        }
    }
}

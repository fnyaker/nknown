// Fenêtre principale (charte, section 6). Accueil : le champ seul, centré. Recherche lancée :
// le champ glisse vers le haut, la carte (≈ 62 %) et la liste (≈ 38 %) prennent la place.
// Échap : referme l'inspecteur, puis désélectionne, puis revient à l'accueil.
import QtQuick
import QtQuick.Controls.Basic
import QtCore as Core
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
    /// Flux d'info de l'accueil.
    property var feeds: []
    readonly property bool hasFeeds: feeds.length > 0

    // Dernier niveau de variété choisi (le défaut reste Normal).
    Core.Settings {
        id: prefs
        category: "search"
        property string variety: "normal"
    }

    function launch(query) {
        error = ""
        hits = []
        intent = ""
        inspected = null
        map.clear()
        searching = true
        phase = "analyze"
        backend.variety = prefs.variety
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

    /// Ouvre un résultat dans le lecteur.
    function read(hit) {
        if (!hit) return
        reader.doc = null
        reader.matched = hit.matched || []
        reader.focusPassage = hit.passage || ""
        reader.loading = true
        reader.open = true
        backend.openDocument(hit.doc, hit.passage || "")
    }

    function hitOf(node) {
        const i = node ? hitIndexOf(node.id) : -1
        return i >= 0 ? hits[i] : null
    }

    function back() {
        if (importer.open) importer.open = false
        else if (statsPanel.open) statsPanel.open = false
        else if (settings.open) settings.open = false
        else if (reader.open) reader.open = false
        else if (inspected) inspected = null
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
        onCatalog: (json) => { try { importer.catalog = JSON.parse(json) } catch (e) {} }
        onStats: (json) => { try { statsPanel.receive(json) } catch (e) {} }
        onEstimate: (json) => { try { importer.estimate = JSON.parse(json) } catch (e) {} }
        onFeeds: (json) => { try { win.feeds = JSON.parse(json) } catch (e) {} }
        onDocument: (json) => { reader.doc = JSON.parse(json); reader.loading = false }
        onNotice: (message, isError) => toast.show(message, isError)
        Component.onCompleted: start()
    }

    Shortcut { sequence: "Esc"; onActivated: win.back() }
    Shortcut { sequences: ["Ctrl+L", "/"]; onActivated: top.focusInput() }
    // Ctrl+F : dans le lecteur, chercher dans le document ; sinon, le champ de recherche.
    Shortcut { sequence: "Ctrl+F"; onActivated: reader.open ? reader.focusFind() : top.focusInput() }

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
            opacity: win.searching || win.hasFeeds ? 0 : 1
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
            variety: prefs.variety
            width: Math.min(win.width - 2 * Theme.margin, win.searching ? 760 : 620)
            anchors.horizontalCenter: parent.horizontalCenter
            // Accueil : au centre ; avec des flux, en haut, les flux dessous.
            y: win.searching ? 18 : win.hasFeeds ? Math.max(70, win.height * 0.1) : win.height * 0.34
            Behavior on y { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
            Behavior on width { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
            onSubmitted: (query) => win.launch(query)
            onVarietyPicked: (value) => prefs.variety = value
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
                onNodeDoubleClicked: (node) => { const h = win.hitOf(node); if (h) win.read(h) }
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
                    // L'inspecteur prend la place de la liste (qui ne reçoit alors plus rien).
                    opacity: win.inspected ? 0 : 1
                    enabled: !win.inspected
                    Behavior on opacity { NumberAnimation { duration: Theme.quick } }
                    delegate: ResultCard {
                        required property var modelData
                        required property int index
                        hit: modelData
                        width: ListView.view.width
                        selected: map.selected === modelData.node
                        onPicked: win.inspect(map.nodes[modelData.node])
                        onRead: win.read(modelData)
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
                    hit: win.hitOf(win.inspected)
                    opacity: win.inspected ? 1 : 0
                    visible: opacity > 0
                    scale: win.inspected ? 1 : 0.98
                    Behavior on opacity { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
                    Behavior on scale { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
                    onCloseRequested: win.inspected = null
                    onLinkPicked: (id) => win.inspect(map.nodes[id])
                    onSearchWord: (word) => { top.text = word; win.launch(word) }
                    onRead: win.read(inspector.hit)
                    onReveal: (path) => backend.revealFile(path)
                }
            }

        }

        // ------------------------------------------------------------------ traduction
        TranslationOffer {
            id: offer
            anchors.top: top.bottom
            anchors.topMargin: 20
            anchors.horizontalCenter: parent.horizontalCenter
            width: Math.min(parent.width - 2 * Theme.margin, 820)
            translation: win.engineState.translation || null
            opacity: win.searching ? 0 : 1
            visible: shown && opacity > 0
            height: implicitHeight
            Behavior on opacity { NumberAnimation { duration: Theme.standard } }
            onAnswer: (spec, accept) => backend.answerTranslation(spec, accept)
        }

        // ------------------------------------------------------------------ flux d'info
        FeedBoard {
            id: board
            anchors.top: offer.visible ? offer.bottom : top.bottom
            anchors.topMargin: offer.visible ? 20 : win.hasFeeds ? 36 : 48
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.leftMargin: Theme.margin
            anchors.rightMargin: Theme.margin
            anchors.bottomMargin: 56
            feeds: win.feeds
            opacity: win.searching ? 0 : 1
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Theme.standard } }
            onRead: (hit, feedId) => { win.read(hit); backend.feedSeen(feedId) }
            onSearch: (query) => { top.text = query; win.launch(query) }
            onSave: (id, title, keywords) => backend.saveFeed(id, title, keywords)
            onRemove: (id) => backend.removeFeed(id)
        }

        // Lecteur : par-dessus la carte et la liste, glisse depuis la droite.
        Reader {
            id: reader
            property bool open: false
            width: Math.min(parent.width - 2 * Theme.margin, Math.max(760, parent.width * 0.62))
            anchors.top: top.bottom
            anchors.topMargin: 20
            anchors.bottom: parent.bottom
            anchors.bottomMargin: Theme.margin
            z: 5
            x: open ? parent.width - width - Theme.margin : parent.width + 20
            opacity: open ? 1 : 0
            visible: opacity > 0
            Behavior on x { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
            Behavior on opacity { NumberAnimation { duration: Theme.standard } }
            onCloseRequested: reader.open = false
            onSearchText: (text) => { reader.open = false; top.text = text; win.launch(text) }
            onReveal: (path) => backend.revealFile(path)
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

        // ------------------------------------------------------------------ réglages
        // Bouton discret : en bas à droite à l'accueil, en haut à droite pendant une recherche.
        Rectangle {
            id: gear
            width: 36; height: 36; radius: 18
            x: win.width - width - 18
            y: win.searching ? 16 : win.height - height - 16
            Behavior on y { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
            color: gearHover.containsMouse ? Theme.bgHover : "transparent"
            // Trois curseurs.
            Repeater {
                model: [0.3, 0.7, 0.45]
                Item {
                    required property real modelData
                    required property int index
                    x: 9; y: 11 + index * 7; width: 18; height: 2
                    Rectangle { anchors.fill: parent; radius: 1; color: Theme.textSecondary; opacity: 0.6 }
                    Rectangle {
                        width: 6; height: 6; radius: 3
                        x: parent.width * modelData - 3; y: -2
                        color: gearHover.containsMouse ? Theme.accent : Theme.textSecondary
                    }
                }
            }
            MouseArea {
                id: gearHover
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: settings.open = !settings.open
            }
            Accessible.role: Accessible.Button
            Accessible.name: "Réglages"
        }

        // Importer : à côté des réglages.
        Rectangle {
            id: importButton
            width: 36; height: 36; radius: 18
            x: gear.x - width - 6
            y: gear.y
            color: importArea.containsMouse ? Theme.bgHover : "transparent"
            Text {
                anchors.centerIn: parent
                text: "+"
                color: importArea.containsMouse ? Theme.accent : Theme.textSecondary
                font.pixelSize: 22
                font.weight: Font.Light
            }
            MouseArea {
                id: importArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: importer.show(win.engineState.sources && win.engineState.sources.length ? "suivi" : "flux")
            }
            ToolTip.visible: importArea.containsMouse
            ToolTip.delay: 600
            ToolTip.text: "Importer des sources"
        }

        // Statistiques : à côté de l'import. Trois barres.
        Rectangle {
            id: statsButton
            width: 36; height: 36; radius: 18
            x: importButton.x - width - 6
            y: gear.y
            color: statsArea.containsMouse ? Theme.bgHover : "transparent"
            Row {
                anchors.centerIn: parent
                spacing: 3
                Repeater {
                    model: [8, 14, 11]
                    delegate: Rectangle {
                        width: 3; height: modelData; radius: 1.5
                        anchors.bottom: parent.bottom
                        color: statsArea.containsMouse ? Theme.accent : Theme.textSecondary
                    }
                }
            }
            MouseArea {
                id: statsArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: statsPanel.open = !statsPanel.open
            }
            ToolTip.visible: statsArea.containsMouse
            ToolTip.delay: 600
            ToolTip.text: "Statistiques"
            Accessible.role: Accessible.Button
            Accessible.name: "Statistiques"
        }

        Stats {
            id: statsPanel
            anchors.fill: parent
            z: 12
            onLoad: (word) => backend.loadStats(word)
        }

        Import {
            id: importer
            anchors.fill: parent
            z: 15
            state: win.engineState
            onAddSource: (spec) => backend.addSource(spec)
            onAddSources: (specs) => backend.addSources(specs)
            onRemoveSource: (id) => backend.removeSource(id)
            onPauseSource: (id, paused) => backend.pauseSource(id, paused)
            onLoadCatalog: backend.loadCatalog()
            onEstimateWikipedia: (edition, tier, limit) => backend.estimateWikipedia(edition, tier, limit)
        }

        Settings {
            id: settings
            onOpenImport: { settings.open = false; importer.show("suivi") }
            anchors.fill: parent
            z: 10
            state: win.engineState
            onAddSource: (spec) => backend.addSource(spec)
            onRemoveSource: (id) => backend.removeSource(id)
            onSetEngine: (mode, url, token) => backend.setEngine(mode, url, token)
            onSetDaemonEnabled: (enabled) => backend.setDaemonEnabled(enabled)
            onSetAnalysis: (parser, precision, device) => backend.setAnalysis(parser, precision, device)
            onSetTranslateDevice: (device) => backend.setTranslateDevice(device)
        }

        // Message bref (source ajoutée, réglage appliqué, erreur), en bas au centre.
        Rectangle {
            id: toast
            z: 20
            property bool isError: false
            function show(message, error) {
                toastText.text = message
                isError = error
                opacity = 1
                hide.restart()
            }
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 56
            width: toastText.implicitWidth + 32
            height: 36
            radius: 18
            color: Theme.bgOverlay
            border.color: isError ? Qt.alpha(Theme.danger, 0.6) : Theme.lineSubtle
            opacity: 0
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: Theme.standard } }
            Text {
                id: toastText
                anchors.centerIn: parent
                color: toast.isError ? Theme.danger : Theme.textPrimary
                font.pixelSize: 12
            }
            Timer { id: hide; interval: 3500; onTriggered: toast.opacity = 0 }
        }
    }

    // ------------------------------------------------------------------ zone de notification
    // Fermer la fenêtre la range dans la zone (NKnown reste à portée, comme KRunner) ; un clic
    // sur l'icône la montre ou la cache.
    function summon() {
        win.show(); win.raise(); win.requestActivate(); top.focusInput()
    }
    Connections {
        target: backend
        function onTrayAction(action) {
            if (action === "quit") win.quit()
            else if (action === "search") { win.summon(); win.back() }
            else if (win.visible && win.active) win.hide()
            else win.summon()
        }
    }
    /// Quitter vraiment : Qt demande d'abord aux fenêtres de se fermer, ce que `onClosing`
    /// intercepterait pour ranger la fenêtre.
    property bool quitting: false
    function quit() { quitting = true; Qt.quit() }
    onClosing: (close) => {
        if (backend.trayAvailable && !win.quitting) {
            close.accepted = false
            win.hide()
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
            case 4: win.snap("3-inspecteur"); if (win.hits.length > 0) win.read(win.hits[0]); break
            case 5: win.snap("4-lecteur"); reader.open = false; break
            case 6:
                win.inspected = null; settings.open = true
                if (win.auto.source) backend.addSource(win.auto.source)
                break
            case 7: win.snap("5-reglages"); settings.open = false; importer.show("suivi"); break
            case 8: win.snap("6-import-suivi"); importer.tab = "flux"; break
            case 9: win.snap("7-import-flux"); importer.tab = "wiki"; break
            case 10: win.snap("8-import-wiki"); importer.tab = "web"; break
            case 11: win.snap("9-import-web"); importer.open = false; statsPanel.probeWord = q || ""; statsPanel.open = true; break
            case 12: break
            case 13: win.snap("10-stats"); break
            default: Qt.quit()
            }
        }
    }
}

// Écran d'import : une page par type de source (flux du catalogue, Wikipédia par paliers, site
// web, dossier) et le suivi de toutes les sources (état, progression, pause, retrait), pensé
// pour en gérer des centaines. Échap ou ✕ le referme.
import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Dialogs

Rectangle {
    id: root
    property bool open: false
    /// État du moteur (Backend.status) : sources et leur collecte.
    property var state: ({})
    /// Catalogue (Backend.catalog) et dernière estimation de Wikipédia (Backend.estimate).
    property var catalog: null
    property var estimate: null
    property string tab: "suivi"

    signal addSource(string spec)
    signal addSources(string specs)
    signal removeSource(string id)
    signal pauseSource(string id, bool paused)
    signal loadCatalog()
    signal estimateWikipedia(string edition, string tier, real limit)

    readonly property var sources: state.sources || []
    readonly property int collectingCount: sources.filter(s => s.state.state === "collecting").length

    function show(which) { tab = which || tab; open = true }

    color: Theme.bgBase
    visible: opacity > 0
    opacity: open ? 1 : 0
    Behavior on opacity { NumberAnimation { duration: Theme.standard } }
    onOpenChanged: if (open) root.loadCatalog()
    // Rien ne traverse vers l'accueil ou la recherche.
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (w) => w.accepted = true }

    // ------------------------------------------------------------------ outils
    function bytes(n) {
        if (n >= 1e9) return (n / 1e9).toLocaleString(Qt.locale(), "f", n >= 1e10 ? 0 : 1) + " Go"
        if (n >= 1e6) return Math.round(n / 1e6) + " Mo"
        return Math.max(1, Math.round(n / 1e3)) + " ko"
    }
    function duration(s) {
        if (s < 90) return "une minute"
        if (s < 3600) return Math.round(s / 60) + " min"
        if (s < 48 * 3600) { const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60); return h + " h" + (m && h < 10 ? " " + m : "") }
        return Math.round(s / 86400) + " jours"
    }
    function count(n) { return Number(n).toLocaleString(Qt.locale(), "f", 0) }
    function kindOf(spec) {
        if (spec.startsWith("rss:")) return "feed"
        if (spec.startsWith("wiki")) return "encyclopedia"
        if (spec.startsWith("web:")) return "web"
        return "file"
    }
    function nameOf(spec) {
        if (spec.startsWith("wiki-top:")) { const p = spec.split(":"); return "Wikipédia " + p[1] + " · " + count(p[2]) + " plus lus" }
        if (spec.startsWith("wiki-dump:")) { const p = spec.split(":"); return "Wikipédia " + p[1] + " · " + (p[2] ? count(p[2]) + " articles" : "édition complète") }
        if (spec.startsWith("wiki:")) return spec.split(":").slice(2).join(":")
        if (spec.startsWith("wiki-")) return "Wikipédia · " + spec.split(":").slice(2).join(":")
        if (spec.startsWith("rss:") || spec.startsWith("web:"))
            return spec.slice(4).split(" ")[0].replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, "")
        return spec.split("/").filter(p => p).slice(-2).join("/") || spec
    }

    // ------------------------------------------------------------------ en-tête et onglets
    Column {
        id: head
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: Theme.margin
        anchors.topMargin: 20
        spacing: 18

        Item {
            width: parent.width
            height: 34
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "Importer"
                color: Theme.textPrimary
                font.pixelSize: 22
                font.weight: Font.DemiBold
            }
            IconButton {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                tip: "Fermer (Échap)"
                onClicked: root.open = false
            }
        }

        Row {
            spacing: 6
            Repeater {
                model: [
                    { key: "suivi", label: "Suivi", badge: root.sources.length },
                    { key: "flux", label: "Flux" },
                    { key: "wiki", label: "Wikipédia" },
                    { key: "web", label: "Site web" },
                    { key: "dossier", label: "Dossier" }
                ]
                Rectangle {
                    required property var modelData
                    readonly property bool current: root.tab === modelData.key
                    height: 34
                    width: tabText.implicitWidth + 28
                    radius: 17
                    color: current ? Qt.alpha(Theme.accent, 0.16) : tabArea.containsMouse ? Theme.bgHover : "transparent"
                    border.color: current ? Qt.alpha(Theme.accent, 0.45) : Theme.lineSubtle
                    Text {
                        id: tabText
                        anchors.centerIn: parent
                        textFormat: Text.StyledText
                        text: modelData.label + (modelData.badge !== undefined
                              ? "  <font color='" + Theme.textSecondary + "'>" + modelData.badge + "</font>" : "")
                        color: parent.current ? Theme.accent : Theme.textPrimary
                        font.pixelSize: 13
                        font.weight: Font.Medium
                    }
                    MouseArea {
                        id: tabArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.tab = modelData.key
                    }
                }
            }
        }
    }

    Item {
        id: page
        anchors.top: head.bottom
        anchors.topMargin: 20
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: Theme.margin
        anchors.rightMargin: Theme.margin
        anchors.bottomMargin: Theme.margin

        // ============================================================== suivi
        Item {
            anchors.fill: parent
            visible: root.tab === "suivi"
            property string kind: ""
            property string filter: ""
            id: follow

            Row {
                id: followBar
                width: parent.width
                spacing: 6
                Repeater {
                    model: [
                        { key: "", label: "Toutes" },
                        { key: "file", label: "Dossiers" },
                        { key: "feed", label: "Flux" },
                        { key: "encyclopedia", label: "Wikipédia" },
                        { key: "web", label: "Sites" }
                    ]
                    Chip {
                        required property var modelData
                        text: modelData.label + "  " + root.sources.filter(s => modelData.key === "" || root.kindOf(s.spec) === modelData.key).length
                        checked: follow.kind === modelData.key
                        onClicked: follow.kind = modelData.key
                    }
                }
                Item { width: 12; height: 1 }
                Field {
                    width: Math.min(280, followBar.width * 0.3)
                    placeholderText: "Filtrer"
                    onTextChanged: follow.filter = text.toLowerCase()
                }
            }
            Text {
                id: followSummary
                anchors.top: followBar.bottom
                anchors.topMargin: 12
                text: root.collectingCount > 0 ? root.collectingCount + " en collecte" : "Tout est à jour"
                color: Theme.textSecondary
                font.pixelSize: 12
            }
            ListView {
                anchors.top: followSummary.bottom
                anchors.topMargin: 8
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                clip: true
                spacing: 2
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
                model: root.sources.filter(s => (follow.kind === "" || root.kindOf(s.spec) === follow.kind)
                                               && (follow.filter === "" || s.spec.toLowerCase().includes(follow.filter)))
                delegate: Item {
                    id: srow
                    required property var modelData
                    readonly property bool collecting: modelData.state.state === "collecting"
                    readonly property bool failed: modelData.state.state === "failed"
                    width: ListView.view.width
                    height: 52
                    Rectangle {
                        anchors.fill: parent
                        radius: 8
                        color: sHover.hovered ? Theme.bgRaised : "transparent"
                    }
                    HoverHandler { id: sHover }
                    SourceBadge {
                        id: badge
                        anchors.left: parent.left
                        anchors.leftMargin: 10
                        anchors.verticalCenter: parent.verticalCenter
                        kind: root.kindOf(srow.modelData.spec)
                    }
                    Column {
                        anchors.left: badge.right
                        anchors.leftMargin: 12
                        anchors.right: actions.left
                        anchors.rightMargin: 12
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 3
                        Text {
                            width: parent.width
                            text: root.nameOf(srow.modelData.spec)
                            elide: Text.ElideRight
                            color: srow.modelData.paused ? Theme.textSecondary : Theme.textPrimary
                            font.pixelSize: 13
                        }
                        Text {
                            width: parent.width
                            elide: Text.ElideMiddle
                            color: srow.failed ? Theme.danger : Theme.textSecondary
                            font.pixelSize: 11
                            font.features: { "tnum": 1 }
                            text: srow.failed ? srow.modelData.state.message
                                : srow.modelData.paused ? "en pause" + (srow.collecting ? " · arrêt en cours" : "")
                                : srow.collecting ? "collecte · " + root.count(srow.modelData.state.done) + " contenus"
                                : srow.modelData.last_run ? "à jour · " + Theme.ago(srow.modelData.last_run)
                                : "en attente"
                        }
                    }
                    // Collecte en cours : un trait qui pulse sous la ligne.
                    Rectangle {
                        anchors.bottom: parent.bottom
                        anchors.left: badge.right
                        anchors.leftMargin: 12
                        width: parent.width * 0.3
                        height: 2
                        radius: 1
                        color: Theme.accent
                        visible: srow.collecting && !srow.modelData.paused
                        SequentialAnimation on opacity {
                            running: srow.collecting
                            loops: Animation.Infinite
                            NumberAnimation { to: 0.2; duration: 800 }
                            NumberAnimation { to: 1; duration: 800 }
                        }
                    }
                    Row {
                        id: actions
                        anchors.right: parent.right
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 4
                        Pill {
                            quiet: true
                            text: srow.modelData.paused ? "Reprendre" : "Pause"
                            onClicked: root.pauseSource(srow.modelData.id, !srow.modelData.paused)
                        }
                        IconButton {
                            tip: "Retirer (les documents restent consultables)"
                            onClicked: root.removeSource(srow.modelData.id)
                        }
                    }
                }
                Text {
                    anchors.centerIn: parent
                    visible: parent.count === 0
                    text: root.sources.length === 0 ? "Aucune source : choisissez un type d'import ci-dessus." : "Aucune source ne correspond."
                    color: Theme.textSecondary
                    font.pixelSize: 13
                }
            }
        }

        // ============================================================== flux
        Item {
            id: feeds
            anchors.fill: parent
            visible: root.tab === "flux"
            property string theme: ""
            property string lang: ""
            /// Spécifications cochées.
            property var picked: ({})
            readonly property var entries: root.catalog && root.catalog.entries
                ? root.catalog.entries.filter(e => (theme === "" || e.theme === theme) && (lang === "" || e.lang === lang)) : []
            readonly property int pickedCount: Object.keys(picked).filter(k => picked[k]).length

            Column {
                id: feedFilters
                width: parent.width
                spacing: 10
                Flow {
                    width: parent.width
                    spacing: 6
                    Chip { text: "Tous les thèmes"; checked: feeds.theme === ""; onClicked: feeds.theme = "" }
                    Repeater {
                        model: root.catalog && root.catalog.themes ? root.catalog.themes : []
                        Chip {
                            required property var modelData
                            text: modelData[1]
                            checked: feeds.theme === modelData[0]
                            onClicked: feeds.theme = modelData[0]
                        }
                    }
                }
                Row {
                    spacing: 6
                    Repeater {
                        model: [{ k: "", l: "Toutes langues" }, { k: "fr", l: "Français" }, { k: "en", l: "English" }, { k: "ru", l: "Русский" }, { k: "zh", l: "中文" }]
                        Chip {
                            required property var modelData
                            text: modelData.l
                            checked: feeds.lang === modelData.k
                            onClicked: feeds.lang = modelData.k
                        }
                    }
                }
            }

            GridView {
                id: grid
                anchors.top: feedFilters.bottom
                anchors.topMargin: 16
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: feedBar.top
                anchors.bottomMargin: 12
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
                readonly property int columns: Math.max(1, Math.floor(width / 330))
                cellWidth: width / columns
                cellHeight: 84
                model: feeds.entries
                delegate: Item {
                    id: entry
                    required property var modelData
                    readonly property bool added: modelData.added
                    readonly property bool checked: added || !!feeds.picked[modelData.spec]
                    width: grid.cellWidth
                    height: grid.cellHeight
                    Rectangle {
                        anchors.fill: parent
                        anchors.margins: 4
                        radius: Theme.radiusCard
                        color: entry.checked && !entry.added ? Qt.alpha(Theme.accent, 0.08) : eArea.containsMouse ? Theme.bgHover : Theme.bgRaised
                        border.color: entry.checked && !entry.added ? Qt.alpha(Theme.accent, 0.5) : Theme.lineSubtle
                        // Case à cocher.
                        Rectangle {
                            id: box
                            x: 14
                            anchors.verticalCenter: parent.verticalCenter
                            width: 18; height: 18; radius: 5
                            color: entry.checked ? (entry.added ? Theme.lineSubtle : Theme.accent) : "transparent"
                            border.color: entry.checked ? "transparent" : Theme.textSecondary
                            Text {
                                anchors.centerIn: parent
                                visible: entry.checked
                                text: "✓"
                                color: entry.added ? Theme.textSecondary : Theme.bgBase
                                font.pixelSize: 12
                                font.weight: Font.Bold
                            }
                        }
                        Column {
                            anchors.left: box.right
                            anchors.leftMargin: 12
                            anchors.right: parent.right
                            anchors.rightMargin: 12
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 3
                            Row {
                                spacing: 8
                                width: parent.width
                                Text {
                                    width: Math.min(implicitWidth, parent.width - langTag.width - 8)
                                    text: entry.modelData.name
                                    elide: Text.ElideRight
                                    color: Theme.textPrimary
                                    font.pixelSize: 13
                                    font.weight: Font.Medium
                                }
                                Text {
                                    id: langTag
                                    text: entry.modelData.lang
                                    color: Theme.textSecondary
                                    font.pixelSize: 10
                                    font.capitalization: Font.AllUppercase
                                }
                            }
                            Text {
                                width: parent.width
                                text: entry.added ? "déjà suivi" : (entry.modelData.description || entry.modelData.spec.slice(4))
                                elide: Text.ElideRight
                                maximumLineCount: 2
                                wrapMode: Text.Wrap
                                color: Theme.textSecondary
                                font.pixelSize: 11
                            }
                        }
                        MouseArea {
                            id: eArea
                            anchors.fill: parent
                            enabled: !entry.added
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                const p = Object.assign({}, feeds.picked)
                                p[entry.modelData.spec] = !p[entry.modelData.spec]
                                feeds.picked = p
                            }
                        }
                    }
                }
                Text {
                    anchors.centerIn: parent
                    visible: !root.catalog
                    text: "Chargement du catalogue…"
                    color: Theme.textSecondary
                    font.pixelSize: 13
                }
            }

            Row {
                id: feedBar
                anchors.bottom: parent.bottom
                width: parent.width
                spacing: 8
                Pill {
                    implicitHeight: 36
                    enabled: feeds.pickedCount > 0
                    text: feeds.pickedCount > 0 ? "Suivre " + feeds.pickedCount + (feeds.pickedCount > 1 ? " flux" : " flux") : "Cochez des flux"
                    onClicked: {
                        root.addSources(Object.keys(feeds.picked).filter(k => feeds.picked[k]).join("\n"))
                        feeds.picked = ({})
                        reload.restart()
                    }
                }
                Item { width: 16; height: 1 }
                Field {
                    id: customFeed
                    width: Math.min(420, parent.width * 0.45)
                    placeholderText: "Ou l'adresse d'un flux RSS / Atom"
                    onAccepted: addCustom.clicked()
                }
                Pill {
                    id: addCustom
                    quiet: true
                    implicitHeight: 36
                    text: "Ajouter"
                    enabled: customFeed.text.trim() !== ""
                    onClicked: { root.addSource("rss:" + customFeed.text.trim().replace(/^rss:/, "")); customFeed.text = ""; reload.restart() }
                }
            }
            // Le catalogue marque les flux ajoutés : relu peu après.
            Timer { id: reload; interval: 1500; onTriggered: root.loadCatalog() }
        }

        // ============================================================== Wikipédia
        Flickable {
            id: wiki
            anchors.fill: parent
            visible: root.tab === "wiki"
            contentHeight: wikiCol.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            property string edition: "fr"
            /// Palier choisi : { tier: "top" | "dump", limit } (limit 0 = édition complète).
            property var choice: ({ tier: "top", limit: 10000 })
            readonly property string spec: choice.tier === "top" ? "wiki-top:" + edition + ":" + choice.limit
                : "wiki-dump:" + edition + (choice.limit > 0 ? ":" + choice.limit : "")
            readonly property bool estimated: !!root.estimate && root.estimate.spec === spec
            function refresh() { root.estimateWikipedia(edition, choice.tier, choice.limit) }
            onVisibleChanged: if (visible) refresh()
            onSpecChanged: if (visible) refresh()

            Column {
                id: wikiCol
                width: Math.min(wiki.width, 880)
                spacing: 22

                Column {
                    spacing: 10
                    Caption { text: "Édition" }
                    Row {
                        spacing: 6
                        Repeater {
                            model: [["fr", "Français"], ["en", "English"], ["ru", "Русский"], ["zh", "中文"], ["de", "Deutsch"], ["es", "Español"]]
                            Chip {
                                required property var modelData
                                text: modelData[1]
                                checked: wiki.edition === modelData[0]
                                onClicked: wiki.edition = modelData[0]
                            }
                        }
                        Field {
                            width: 90
                            placeholderText: "autre"
                            onAccepted: if (/^[a-z-]{2,12}$/.test(text.trim())) wiki.edition = text.trim()
                        }
                    }
                }

                Column {
                    spacing: 10
                    width: parent.width
                    Caption { text: "Palier" }
                    Flow {
                        width: parent.width
                        spacing: 10
                        Repeater {
                            model: [
                                { tier: "top", limit: 1000, title: "1 000", sub: "les plus lus" },
                                { tier: "top", limit: 10000, title: "10 000", sub: "les plus lus" },
                                { tier: "dump", limit: 100000, title: "100 000", sub: "articles du dump" },
                                { tier: "dump", limit: 1000000, title: "1 million", sub: "articles du dump" },
                                { tier: "dump", limit: 0, title: "Tout", sub: "l'édition complète" }
                            ]
                            Rectangle {
                                required property var modelData
                                readonly property bool current: wiki.choice.tier === modelData.tier && wiki.choice.limit === modelData.limit
                                width: 160
                                height: 76
                                radius: Theme.radiusCard
                                color: current ? Qt.alpha(Theme.accent, 0.1) : tierArea.containsMouse ? Theme.bgHover : Theme.bgRaised
                                border.color: current ? Qt.alpha(Theme.accent, 0.55) : Theme.lineSubtle
                                Column {
                                    anchors.centerIn: parent
                                    spacing: 4
                                    Text {
                                        anchors.horizontalCenter: parent.horizontalCenter
                                        text: modelData.title
                                        color: parent.parent.current ? Theme.accent : Theme.textPrimary
                                        font.pixelSize: 18
                                        font.weight: Font.DemiBold
                                    }
                                    Text {
                                        anchors.horizontalCenter: parent.horizontalCenter
                                        text: modelData.sub
                                        color: Theme.textSecondary
                                        font.pixelSize: 11
                                    }
                                }
                                MouseArea {
                                    id: tierArea
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: wiki.choice = { tier: modelData.tier, limit: modelData.limit }
                                }
                            }
                        }
                    }
                }

                // Estimation, avant de lancer.
                Rectangle {
                    width: parent.width
                    height: estCol.implicitHeight + 32
                    radius: Theme.radiusCard
                    color: Theme.bgRaised
                    border.color: Theme.lineSubtle
                    Column {
                        id: estCol
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 16
                        spacing: 12
                        Row {
                            spacing: 28
                            visible: wiki.estimated && !root.estimate.error
                            Repeater {
                                model: wiki.estimated && !root.estimate.error ? [
                                    { v: root.count(root.estimate.articles), l: "articles" },
                                    { v: root.bytes(root.estimate.download_bytes), l: "téléchargés" },
                                    { v: root.bytes(root.estimate.disk_bytes), l: "dans la base" },
                                    { v: "≈ " + root.duration(root.estimate.seconds), l: "en arrière-plan" }
                                ] : []
                                Column {
                                    required property var modelData
                                    spacing: 2
                                    Text { text: modelData.v; color: Theme.textPrimary; font.pixelSize: 20; font.weight: Font.DemiBold; font.features: { "tnum": 1 } }
                                    Text { text: modelData.l; color: Theme.textSecondary; font.pixelSize: 11 }
                                }
                            }
                        }
                        Text {
                            visible: !wiki.estimated || !!root.estimate.error
                            text: wiki.estimated ? "Estimation impossible : " + root.estimate.error : "Estimation…"
                            color: wiki.estimated ? Theme.danger : Theme.textSecondary
                            font.pixelSize: 12
                            width: parent.width
                            wrapMode: Text.Wrap
                        }
                        Text {
                            width: parent.width
                            wrapMode: Text.Wrap
                            color: Theme.textSecondary
                            font.pixelSize: 12
                            lineHeight: 1.3
                            text: wiki.choice.tier === "top"
                                ? "Les articles les plus consultés ces derniers mois, téléchargés un à un auprès de Wikipédia, puis tenus à jour chaque semaine."
                                : "Lu directement dans l'archive publiée par Wikimedia, bloc par bloc : rien n'est stocké en dehors de la base. Une nouvelle archive est relue quand elle paraît."
                                  + " Indexation en arrière-plan, analyse légère (sans le réseau neuronal) ; une pause dans « Suivi » l'interrompt, la reprise repart où elle s'était arrêtée."
                        }
                        Row {
                            spacing: 8
                            Pill {
                                implicitHeight: 36
                                readonly property bool already: root.sources.some(s => s.spec === wiki.spec)
                                enabled: !already
                                text: already ? "Déjà importé" : "Importer"
                                onClicked: { root.addSource(wiki.spec); root.tab = "suivi" }
                            }
                        }
                    }
                }

                // Articles précis.
                Column {
                    spacing: 10
                    width: parent.width
                    Caption { text: "Ou des articles précis" }
                    Row {
                        spacing: 8
                        Field {
                            id: wikiTitles
                            width: Math.min(520, wikiCol.width - 120)
                            placeholderText: "Titres séparés par | (ex. Tour Eiffel|Lyon)"
                            onAccepted: wikiAdd.clicked()
                        }
                        Pill {
                            id: wikiAdd
                            quiet: true
                            implicitHeight: 36
                            text: "Ajouter"
                            enabled: wikiTitles.text.trim() !== ""
                            onClicked: { root.addSource("wiki:" + wiki.edition + ":" + wikiTitles.text.trim()); wikiTitles.text = "" }
                        }
                    }
                }
            }
        }

        // ============================================================== site web
        Column {
            id: web
            anchors.fill: parent
            visible: root.tab === "web"
            spacing: 18
            property int pages: 200
            property int depth: 3
            Caption { text: "Adresse de départ" }
            Field {
                id: webUrl
                width: Math.min(640, parent.width)
                placeholderText: "https://doc.exemple.org/guide/"
                onAccepted: webGo.clicked()
            }
            Caption { text: "Pages au plus" }
            Row {
                spacing: 6
                Repeater {
                    model: [50, 200, 1000, 5000]
                    Chip { required property int modelData; text: root.count(modelData); checked: web.pages === modelData; onClicked: web.pages = modelData }
                }
            }
            Caption { text: "Profondeur des liens" }
            Row {
                spacing: 6
                Repeater {
                    model: [1, 2, 3, 5, 8]
                    Chip { required property int modelData; text: modelData; checked: web.depth === modelData; onClicked: web.depth = modelData }
                }
            }
            Text {
                width: Math.min(640, parent.width)
                wrapMode: Text.Wrap
                color: Theme.textSecondary
                font.pixelSize: 12
                lineHeight: 1.3
                text: "Seules les pages du même site, sous le dossier de l'adresse, sont lues ; robots.txt est respecté, une page par seconde au plus. Devant un captcha ou une protection anti-robot, l'exploration s'arrête : rien n'est contourné."
            }
            Pill {
                id: webGo
                implicitHeight: 36
                enabled: /^https?:\/\/[^\s]+$/.test(webUrl.text.trim())
                text: "Explorer"
                onClicked: {
                    root.addSource("web:" + webUrl.text.trim() + " pages=" + web.pages + " depth=" + web.depth)
                    webUrl.text = ""
                    root.tab = "suivi"
                }
            }
        }

        // ============================================================== dossier
        Column {
            anchors.fill: parent
            visible: root.tab === "dossier"
            spacing: 18
            Text {
                width: Math.min(640, parent.width)
                wrapMode: Text.Wrap
                color: Theme.textSecondary
                font.pixelSize: 12
                lineHeight: 1.3
                text: "Textes, notes Markdown et code : le dossier est surveillé, un fichier modifié est réindexé aussitôt."
            }
            Row {
                spacing: 8
                Field {
                    id: folderPath
                    width: Math.min(520, page.width - 240)
                    placeholderText: "Chemin du dossier"
                    onAccepted: folderAdd.clicked()
                }
                Pill { quiet: true; implicitHeight: 36; text: "Parcourir"; onClicked: folderDialog.open() }
                Pill {
                    id: folderAdd
                    implicitHeight: 36
                    text: "Ajouter"
                    enabled: folderPath.text.trim() !== ""
                    onClicked: { root.addSource(folderPath.text.trim()); folderPath.text = ""; root.tab = "suivi" }
                }
            }
            FolderDialog {
                id: folderDialog
                onAccepted: folderPath.text = decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, ""))
            }
        }
    }

    // ------------------------------------------------------------------ composants
    component Caption: Text {
        color: Theme.textSecondary
        font.pixelSize: 11
        font.weight: Font.DemiBold
        font.capitalization: Font.AllUppercase
        font.letterSpacing: 0.8
    }

    component Chip: Rectangle {
        id: chip
        property string text: ""
        property bool checked: false
        signal clicked()
        height: 30
        width: chipText.implicitWidth + 24
        radius: 15
        color: checked ? Qt.alpha(Theme.accent, 0.16) : chipArea.containsMouse ? Theme.bgHover : "transparent"
        border.color: checked ? Qt.alpha(Theme.accent, 0.45) : Theme.lineSubtle
        Text {
            id: chipText
            anchors.centerIn: parent
            text: chip.text
            color: chip.checked ? Theme.accent : Theme.textPrimary
            font.pixelSize: 12
            font.features: { "tnum": 1 }
        }
        MouseArea {
            id: chipArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: chip.clicked()
        }
    }

    component Field: TextField {
        height: 36
        color: Theme.textPrimary
        placeholderTextColor: Theme.textSecondary
        selectionColor: Qt.alpha(Theme.accent, 0.35)
        font.family: Theme.fontUi
        font.pixelSize: 13
        leftPadding: 12
        background: Rectangle {
            radius: 8
            color: Theme.bgRaised
            border.color: parent.activeFocus ? Qt.alpha(Theme.accent, 0.6) : Theme.lineSubtle
        }
    }

    /// Pastille de type de source : une lettre sur la teinte du type.
    component SourceBadge: Rectangle {
        property string kind: "file"
        readonly property color tint: ({ file: Theme.kindColor("code"), feed: Theme.kindColor("prose"),
                                         encyclopedia: Theme.kindColor("term"), web: Theme.kindColor("qualifier") })[kind] || Theme.textSecondary
        width: 28; height: 28; radius: 8
        color: Qt.alpha(tint, 0.16)
        Text {
            anchors.centerIn: parent
            text: ({ file: "D", feed: "F", encyclopedia: "W", web: "S" })[parent.kind] || "?"
            color: parent.tint
            font.pixelSize: 12
            font.weight: Font.Bold
        }
    }
}

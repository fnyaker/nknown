// Statistiques : ce que contient la base, ce que le moteur sait des mots (liens du dictionnaire,
// liens appris dans les documents), ce qu'il consomme. Un panneau qui glisse depuis la droite,
// relu toutes les trois secondes tant qu'il est ouvert. « Tester un mot » montre ce qui élargit
// un mot à la recherche, et d'où vient chaque lien.
import QtQuick
import QtQuick.Controls.Basic

Item {
    id: root
    property bool open: false
    /// Dernière réponse de Backend.stats (JSON décodé).
    property var info: null
    property string error: ""
    /// Mot testé (relu avec les statistiques).
    property string probeWord: ""
    signal load(string word)

    visible: open || panel.x < width
    onOpenChanged: if (open) load(probeWord)

    function receive(json) {
        const d = JSON.parse(json)
        if (d.error) { error = d.error; return }
        error = ""
        info = d
    }

    function count(n) { return Number(n || 0).toLocaleString(Qt.locale(), "f", 0) }
    function bytes(n) {
        n = n || 0
        if (n >= 1e9) return (n / 1e9).toLocaleString(Qt.locale(), "f", 2) + " Go"
        if (n >= 1e6) return (n / 1e6).toLocaleString(Qt.locale(), "f", 0) + " Mo"
        return (n / 1e3).toLocaleString(Qt.locale(), "f", 0) + " ko"
    }
    function langName(code) {
        switch (code) {
        case "fra": return "Français"
        case "eng": return "Anglais"
        case "rus": return "Russe"
        case "zho": case "cmn": return "Chinois"
        default: return code
        }
    }
    function originColor(o) {
        switch (o) {
        case "confirmed": return Theme.ok
        case "learned": return Theme.accent
        default: return Theme.kindColor("related")
        }
    }
    function originName(o) {
        switch (o) {
        case "confirmed": return "dictionnaire, confirmé par vos documents"
        case "learned": return "appris dans vos documents"
        default: return "dictionnaire"
        }
    }

    readonly property var learned: info ? info.learned : null
    readonly property var memory: info ? info.memory : null
    readonly property var compute: info ? info.compute : null

    Timer {
        interval: 3000
        repeat: true
        running: root.open
        onTriggered: root.load(root.probeWord)
    }

    // Fond assombri : un clic referme.
    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: root.open ? 0.4 : 0
        Behavior on opacity { NumberAnimation { duration: Theme.standard } }
        MouseArea { anchors.fill: parent; enabled: root.open; onClicked: root.open = false }
    }

    Rectangle {
        id: panel
        width: Math.min(520, root.width - 48)
        height: root.height
        x: root.open ? root.width - width : root.width
        Behavior on x { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
        color: Theme.bgRaised
        border.color: Theme.lineSubtle
        MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (w) => w.accepted = false }

        Flickable {
            anchors.fill: parent
            anchors.margins: Theme.margin
            contentHeight: column.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: column
                width: parent.width
                spacing: 28

                Row {
                    width: parent.width
                    Text {
                        width: parent.width - 28
                        text: "Statistiques"
                        color: Theme.textPrimary
                        font.pixelSize: 20
                        font.weight: Font.DemiBold
                    }
                    CloseButton { onClicked: root.open = false }
                }

                Text {
                    visible: root.error !== ""
                    width: column.width
                    wrapMode: Text.Wrap
                    text: root.error
                    color: Theme.danger
                    font.pixelSize: 13
                }

                // ------------------------------------------------------------ base
                Section {
                    title: "Base"
                    Grid {
                        columns: 2
                        columnSpacing: 12
                        rowSpacing: 12
                        width: column.width
                        Tile { label: "documents"; value: root.info ? root.count(root.info.docs) : "–" }
                        Tile { label: "passages"; value: root.info ? root.count(root.info.passages) : "–" }
                        Tile { label: "mots indexés"; value: root.info ? root.count(root.info.words) : "–" }
                        Tile { label: "sur le disque"; value: root.info ? root.bytes(root.info.base_bytes) : "–" }
                    }
                }

                // ------------------------------------------------------------ liens
                Section {
                    title: "Liens entre les mots"

                    Text {
                        width: column.width
                        wrapMode: Text.Wrap
                        color: Theme.textSecondary
                        font.pixelSize: 12
                        text: "Le dictionnaire relie les mots de sens proche. Vos documents confirment ces liens, ou en apprennent d'autres."
                    }

                    // Dictionnaires chargés, par langue.
                    Flow {
                        width: column.width
                        spacing: 8
                        Repeater {
                            model: root.info ? root.info.dictionaries : []
                            delegate: Chip {
                                text: root.langName(modelData.lang) + " · " + root.count(modelData.lemmas) + " mots"
                                tint: Theme.kindColor("related")
                            }
                        }
                        Text {
                            visible: !!(root.info !== null && root.info.dictionaries.length === 0)
                            text: "Aucun dictionnaire de liens chargé"
                            color: Theme.danger
                            font.pixelSize: 13
                        }
                    }

                    // Liens appris : état du calcul.
                    Rectangle {
                        width: column.width
                        height: learnedCol.implicitHeight + 24
                        radius: Theme.radiusCard
                        color: Theme.bgBase
                        border.color: Theme.lineSubtle
                        Column {
                            id: learnedCol
                            x: 12; y: 12
                            width: parent.width - 24
                            spacing: 10

                            Row {
                                spacing: 8
                                Rectangle {
                                    width: 8; height: 8; radius: 4
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: {
                                        const s = root.learned ? root.learned.state.state : ""
                                        if (s === "ready") return Theme.ok
                                        if (s === "running") return Theme.accent
                                        if (s === "failed") return Theme.danger
                                        return Theme.textSecondary
                                    }
                                    SequentialAnimation on opacity {
                                        running: !!(root.learned && root.learned.state.state === "running")
                                        loops: Animation.Infinite
                                        NumberAnimation { to: 0.3; duration: 600 }
                                        NumberAnimation { to: 1; duration: 600 }
                                    }
                                }
                                Text {
                                    color: Theme.textPrimary
                                    font.pixelSize: 14
                                    font.weight: Font.DemiBold
                                    text: {
                                        const l = root.learned
                                        if (!l) return "Liens appris"
                                        switch (l.state.state) {
                                        case "running": return "Apprentissage en cours · " + l.state.secs + " s"
                                        case "failed": return "Apprentissage en échec"
                                        case "ready": return "Liens appris " + Theme.ago(l.at)
                                        default: return "Pas encore de liens appris"
                                        }
                                    }
                                }
                            }

                            Text {
                                visible: !!(root.learned && root.learned.state.state === "failed")
                                width: parent.width
                                wrapMode: Text.Wrap
                                color: Theme.danger
                                font.pixelSize: 12
                                text: root.learned && root.learned.state.message ? root.learned.state.message : ""
                            }

                            Grid {
                                visible: !!(root.learned && root.learned.words > 0)
                                columns: 3
                                columnSpacing: 12
                                width: parent.width
                                Metric { label: "mots reliés"; value: root.learned ? root.count(root.learned.words) : "" }
                                Metric { label: "liens de sens"; value: root.learned ? root.count(root.learned.syntactic) : "" }
                                Metric { label: "liens de thème"; value: root.learned ? root.count(root.learned.topical) : "" }
                            }

                            // Prochain calcul : quand la base aura assez grossi.
                            Column {
                                width: parent.width
                                spacing: 6
                                visible: !!(root.learned && root.learned.state.state !== "running")
                                Text {
                                    width: parent.width
                                    wrapMode: Text.Wrap
                                    color: Theme.textSecondary
                                    font.pixelSize: 12
                                    text: {
                                        const l = root.learned
                                        if (!l || !root.info) return ""
                                        let t = "Prochain calcul à " + root.count(l.next_passages) + " passages (" + root.count(root.info.passages) + " aujourd'hui)"
                                        if (l.took_secs) t += " · dernier calcul : " + Math.round(l.took_secs) + " s"
                                        return t
                                    }
                                }
                                Rectangle {
                                    width: parent.width
                                    height: 4
                                    radius: 2
                                    color: Theme.lineSubtle
                                    Rectangle {
                                        height: parent.height
                                        radius: 2
                                        color: Theme.accent
                                        width: {
                                            const l = root.learned
                                            if (!l || !root.info || !l.next_passages) return 0
                                            const from = l.at_passages || 0
                                            const span = Math.max(1, l.next_passages - from)
                                            return parent.width * Math.max(0, Math.min(1, (root.info.passages - from) / span))
                                        }
                                        Behavior on width { NumberAnimation { duration: Theme.standard } }
                                    }
                                }
                            }
                        }
                    }

                    // Exemples de liens appris.
                    Column {
                        width: column.width
                        spacing: 6
                        visible: !!(root.learned && root.learned.examples.length > 0)
                        Repeater {
                            model: root.learned ? root.learned.examples : []
                            delegate: Row {
                                width: column.width
                                spacing: 10
                                Text {
                                    width: 120
                                    elide: Text.ElideRight
                                    text: modelData.lemma
                                    color: Theme.kindColor("term")
                                    font.pixelSize: 13
                                    MouseArea {
                                        anchors.fill: parent
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: { probe.text = modelData.lemma; probe.run() }
                                    }
                                }
                                Text {
                                    width: parent.width - 130
                                    elide: Text.ElideRight
                                    text: modelData.links.join(" · ")
                                    color: Theme.textSecondary
                                    font.pixelSize: 13
                                }
                            }
                        }
                    }
                }

                // ------------------------------------------------------------ tester un mot
                Section {
                    title: "Tester un mot"
                    Field {
                        id: probe
                        width: column.width
                        placeholder: "élection, voiture, battery…"
                        function run() { root.probeWord = text.trim(); root.load(root.probeWord) }
                        onAccepted: run()
                    }
                    Repeater {
                        model: root.info && root.info.probe ? root.info.probe : []
                        delegate: Column {
                            width: column.width
                            spacing: 8
                            Row {
                                spacing: 8
                                Text {
                                    text: modelData.text
                                    color: Theme.textPrimary
                                    font.pixelSize: 15
                                    font.weight: Font.DemiBold
                                }
                                Text {
                                    anchors.baseline: parent.children[0].baseline
                                    text: modelData.readings.join(" · ")
                                    color: Theme.textSecondary
                                    font.pixelSize: 11
                                }
                            }
                            Text {
                                visible: modelData.related.length === 0
                                text: "Aucun mot lié : la recherche cherchera ce mot seul."
                                color: Theme.textSecondary
                                font.pixelSize: 12
                            }
                            Flow {
                                width: parent.width
                                spacing: 6
                                Repeater {
                                    model: modelData.related
                                    delegate: Chip {
                                        text: modelData.word + "  " + Math.round(modelData.weight * 100) + " %"
                                        tint: root.originColor(modelData.origin)
                                        tip: root.originName(modelData.origin)
                                    }
                                }
                            }
                        }
                    }
                    // Légende des couleurs.
                    Flow {
                        visible: !!(root.info && root.info.probe && root.info.probe.length > 0)
                        width: column.width
                        spacing: 14
                        Repeater {
                            model: ["dictionary", "confirmed", "learned"]
                            delegate: Row {
                                spacing: 6
                                Rectangle {
                                    width: 8; height: 8; radius: 4
                                    anchors.verticalCenter: parent.verticalCenter
                                    color: root.originColor(modelData)
                                }
                                Text { text: root.originName(modelData); color: Theme.textSecondary; font.pixelSize: 11 }
                            }
                        }
                    }
                }

                // ------------------------------------------------------------ ressources
                Section {
                    title: "Ressources"
                    Grid {
                        columns: 2
                        columnSpacing: 12
                        rowSpacing: 12
                        width: column.width
                        Tile { label: "mémoire du démon"; value: root.memory ? root.bytes(root.memory.resident) : "–" }
                        Tile { label: "au plus haut"; value: root.memory ? root.bytes(root.memory.peak) : "–" }
                        Tile { label: "dans le swap"; value: root.memory ? root.bytes(root.memory.swap) : "–" }
                        Tile { label: "cache de la base (max)"; value: root.memory ? root.bytes(root.memory.base_cache) : "–" }
                    }
                    Column {
                        width: column.width
                        spacing: 6
                        Text {
                            color: Theme.textPrimary
                            font.pixelSize: 13
                            text: {
                                const c = root.compute
                                if (!c) return ""
                                const p = c.parser === "gpu" ? "sur le GPU" : c.parser === "cpu" ? "sur le CPU" : "désactivé"
                                return "Analyseur " + p
                            }
                        }
                        Text {
                            visible: !!(root.compute && root.compute.translators.length > 0)
                            color: Theme.textPrimary
                            font.pixelSize: 13
                            text: root.compute ? "Traduction : " + root.compute.translators.join(", ") : ""
                        }
                        Text {
                            visible: !!(root.compute && root.compute.translating)
                            color: Theme.accent
                            font.pixelSize: 13
                            text: root.compute && root.compute.translating
                                ? "Traduction en cours : " + root.compute.translating.done + " / " + root.compute.translating.total
                                : ""
                        }
                        Text {
                            visible: !!(root.compute && root.compute.collecting)
                            color: Theme.accent
                            font.pixelSize: 13
                            text: "Collecte en cours"
                        }
                        Text {
                            visible: !!(root.compute && root.compute.gpu_lost)
                            width: column.width
                            wrapMode: Text.Wrap
                            color: Theme.danger
                            font.pixelSize: 13
                            text: "Le GPU a lâché (mémoire pleine ?) : tout calcule sur le CPU jusqu'au prochain redémarrage du démon."
                        }
                    }
                }
            }
        }
    }

    component Section: Column {
        property string title
        width: parent ? parent.width : 0
        spacing: 12
        Text {
            text: parent.title
            color: Theme.textSecondary
            font.pixelSize: 11
            font.weight: Font.DemiBold
            font.letterSpacing: 1.2
            font.capitalization: Font.AllUppercase
        }
    }

    component Tile: Rectangle {
        property string label
        property string value
        width: (column.width - 12) / 2
        height: 64
        radius: Theme.radiusCard
        color: Theme.bgBase
        border.color: Theme.lineSubtle
        Column {
            anchors.verticalCenter: parent.verticalCenter
            x: 14
            spacing: 2
            Text { text: value; color: Theme.textPrimary; font.pixelSize: 20; font.weight: Font.DemiBold }
            Text { text: label; color: Theme.textSecondary; font.pixelSize: 11 }
        }
    }

    component Metric: Column {
        property string label
        property string value
        width: (learnedCol.width - 24) / 3
        spacing: 2
        Text { text: value; color: Theme.textPrimary; font.pixelSize: 17; font.weight: Font.DemiBold }
        Text { text: label; color: Theme.textSecondary; font.pixelSize: 11 }
    }

    component Chip: Rectangle {
        id: chip
        property alias text: chipText.text
        property color tint: Theme.accent
        property string tip: ""
        width: chipText.implicitWidth + 20
        height: 26
        radius: 13
        color: Qt.alpha(tint, 0.12)
        border.color: Qt.alpha(tint, 0.35)
        Text {
            id: chipText
            anchors.centerIn: parent
            color: chip.tint
            font.pixelSize: 12
        }
        MouseArea { id: chipHover; anchors.fill: parent; hoverEnabled: chip.tip !== "" }
        ToolTip.visible: chipHover.containsMouse && chip.tip !== ""
        ToolTip.delay: 400
        ToolTip.text: chip.tip
    }

    component Field: TextField {
        property string placeholder
        placeholderText: placeholder
        placeholderTextColor: Theme.textSecondary
        color: Theme.textPrimary
        font.pixelSize: 13
        leftPadding: 12
        rightPadding: 12
        height: 36
        background: Rectangle {
            radius: 8
            color: Theme.bgBase
            border.color: parent.activeFocus ? Qt.alpha(Theme.accent, 0.6) : Theme.lineSubtle
            Behavior on border.color { ColorAnimation { duration: Theme.quick } }
        }
    }

    component CloseButton: Rectangle {
        signal clicked()
        width: 28; height: 28; radius: 14
        color: closeHover.containsMouse ? Theme.bgHover : "transparent"
        Text { anchors.centerIn: parent; text: "✕"; color: Theme.textSecondary; font.pixelSize: 13 }
        MouseArea {
            id: closeHover
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: parent.clicked()
        }
        Accessible.role: Accessible.Button
        Accessible.name: "Fermer"
    }
}

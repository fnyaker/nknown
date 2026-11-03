// Réglages : sources, moteur, démon. Un panneau qui glisse depuis la droite ; Échap ou un clic à
// côté le referme. Peu de mots : chaque ligne dit ce qu'elle règle, l'état se lit aux signes.
import QtQuick
import QtQuick.Controls.Basic

Item {
    id: root
    property bool open: false
    /// État relu toutes les deux secondes (Backend.status).
    property var state: ({})
    signal addSource(string spec)
    signal removeSource(string id)
    signal openImport()
    signal setEngine(string mode, string url, string token)
    signal setDaemonEnabled(bool enabled)
    signal setAnalysis(bool parser, string precision, string device)

    visible: open || panel.x < width
    readonly property var sources: state.sources || []
    readonly property var settings: state.settings || ({ engine: "auto", remote_url: "" })
    readonly property var daemon: state.daemon || ({ installed: false, enabled: false })
    readonly property var analysis: state.analysis || null
    function applyAnalysis(change) {
        const a = Object.assign({}, root.analysis, change)
        root.setAnalysis(a.parser, a.precision, a.device)
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
        width: Math.min(460, root.width - 48)
        height: root.height
        x: root.open ? root.width - width : root.width
        Behavior on x { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
        color: Theme.bgRaised
        border.color: Theme.lineSubtle
        // Le panneau garde les clics pour lui.
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
                        text: "Réglages"
                        color: Theme.textPrimary
                        font.pixelSize: 20
                        font.weight: Font.DemiBold
                    }
                    CloseButton { onClicked: root.open = false }
                }

                // ------------------------------------------------------------ sources
                Section {
                    title: "Sources"
                    Text {
                        width: column.width
                        wrapMode: Text.Wrap
                        color: Theme.textSecondary
                        font.pixelSize: 12
                        text: root.sources.length === 0 ? "Aucune source pour l'instant."
                            : root.sources.length + (root.sources.length > 1 ? " sources suivies" : " source suivie")
                              + (root.sources.some(s => s.state.state === "collecting") ? " · collecte en cours" : "")
                    }
                    Pill {
                        implicitHeight: 36
                        text: "Importer et gérer les sources"
                        onClicked: root.openImport()
                    }
                }

                // ------------------------------------------------------------ moteur
                Section {
                    title: "Moteur"
                    Segmented {
                        id: mode
                        width: column.width
                        options: [
                            { key: "auto", label: "Auto" },
                            { key: "daemon", label: "Démon" },
                            { key: "embedded", label: "Intégré" },
                            { key: "remote", label: "Distant" }
                        ]
                        current: root.settings.engine
                        onPicked: (key) => { if (key !== "remote") root.setEngine(key, "", "") }
                    }
                    Text {
                        width: column.width
                        wrapMode: Text.Wrap
                        color: Theme.textSecondary
                        font.pixelSize: 12
                        text: ({
                            "auto": "Le démon local s'il tourne, sinon le moteur intégré à l'app.",
                            "daemon": "Le démon local : partagé avec KRunner et la ligne de commande.",
                            "embedded": "Dans l'app : rien ne tourne en arrière-plan quand elle est fermée.",
                            "remote": "Un serveur NKnown : la base est ailleurs, l'app n'en est que la vue."
                        })[mode.shown] || ""
                    }
                    Column {
                        visible: mode.shown === "remote"
                        width: column.width
                        spacing: 8
                        Field {
                            id: url
                            width: parent.width
                            placeholder: "https://serveur.exemple"
                            text: root.settings.remote_url
                        }
                        Field {
                            id: token
                            width: parent.width
                            placeholder: "Jeton (gardé dans KWallet)"
                            secret: true
                        }
                        Pill {
                            implicitHeight: 36
                            text: "Se connecter"
                            onClicked: { root.setEngine("remote", url.text, token.text); token.text = "" }
                        }
                    }
                    // Moteur effectivement utilisé.
                    Row {
                        spacing: 8
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 7; height: 7; radius: 3.5
                            color: root.state.error ? Theme.danger
                                 : root.state.engine === "connecting" ? Theme.accent : Theme.ok
                        }
                        Text {
                            color: Theme.textSecondary
                            font.pixelSize: 12
                            text: {
                                const s = root.state
                                if (s.error) return s.error
                                if (s.engine === "connecting" || !s.engine) return "Connexion…"
                                const name = ({ daemon: "démon local", embedded: "moteur intégré",
                                                remote: "serveur distant" })[s.engine] || s.engine
                                return name + (s.status ? " · " + s.status.docs + " documents" : "")
                            }
                        }
                    }
                }

                // ------------------------------------------------------------ analyse
                Section {
                    title: "Analyse"
                    visible: root.analysis !== null
                    Row {
                        width: column.width
                        Text {
                            width: parent.width - parserToggle.width
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Analyse syntaxique"
                            color: Theme.textPrimary
                            font.pixelSize: 13
                        }
                        Toggle {
                            id: parserToggle
                            checked: !!root.analysis && root.analysis.parser
                            onToggled: (on) => root.applyAnalysis({ parser: on })
                        }
                    }
                    Segmented {
                        visible: !!root.analysis && root.analysis.parser
                        width: column.width
                        options: [
                            { key: "auto", label: "Auto" },
                            { key: "gpu", label: "GPU" },
                            { key: "cpu", label: "CPU" }
                        ]
                        current: root.analysis ? root.analysis.device : "auto"
                        onPicked: (key) => root.applyAnalysis({ device: key })
                    }
                    Segmented {
                        visible: !!root.analysis && root.analysis.parser
                        width: column.width
                        options: [
                            { key: "standard", label: "Précise" },
                            { key: "light", label: "Légère" }
                        ]
                        current: root.analysis ? root.analysis.precision : "standard"
                        onPicked: (key) => root.applyAnalysis({ precision: key })
                    }
                    Text {
                        width: column.width
                        wrapMode: Text.Wrap
                        color: Theme.textSecondary
                        font.pixelSize: 12
                        text: {
                            const s = root.state.status
                            if (!root.analysis || !root.analysis.parser)
                                return "Désactivée : indexation plus légère, adjectifs moins bien reliés aux noms."
                            const where = s && s.parser_device === "gpu" ? "Calcule sur le GPU (Vulkan) : le processeur reste libre."
                                        : s && s.parser_device === "cpu" ? "Calcule sur le processeur, en priorité basse."
                                        : "Chargement…"
                            return where + (root.analysis.precision === "light" ? " Version légère (CPU)." : "")
                        }
                    }
                }

                // ------------------------------------------------------------ démon
                Section {
                    title: "Démon"
                    visible: root.daemon.installed
                    Row {
                        width: column.width
                        Text {
                            width: parent.width - toggle.width
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Démarrer avec la session"
                            color: Theme.textPrimary
                            font.pixelSize: 13
                        }
                        Toggle {
                            id: toggle
                            checked: root.daemon.enabled
                            onToggled: (on) => root.setDaemonEnabled(on)
                        }
                    }
                    Text {
                        width: column.width
                        wrapMode: Text.Wrap
                        color: Theme.textSecondary
                        font.pixelSize: 12
                        text: "Indexe vos sources en continu et répond à KRunner, même app fermée."
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ composants
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

    component Field: TextField {
        property string placeholder
        property bool secret: false
        placeholderText: placeholder
        placeholderTextColor: Theme.textSecondary
        echoMode: secret ? TextInput.Password : TextInput.Normal
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

    component Segmented: Rectangle {
        id: seg
        property var options: []
        property string current: ""
        /// Choix montré (avant confirmation, pour « Distant » qui demande une adresse).
        property string shown: current
        onCurrentChanged: shown = current
        signal picked(string key)
        height: 36
        radius: 10
        color: Theme.bgBase
        border.color: Theme.lineSubtle
        // Curseur qui glisse sous le choix.
        Rectangle {
            readonly property int index: Math.max(0, seg.options.findIndex(o => o.key === seg.shown))
            width: (seg.width - 6) / Math.max(1, seg.options.length)
            height: seg.height - 6
            x: 3 + index * width
            y: 3
            radius: 8
            color: Qt.alpha(Theme.accent, 0.16)
            border.color: Qt.alpha(Theme.accent, 0.45)
            Behavior on x { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
        }
        Row {
            anchors.fill: parent
            anchors.margins: 3
            Repeater {
                model: seg.options
                Item {
                    required property var modelData
                    width: (seg.width - 6) / Math.max(1, seg.options.length)
                    height: parent.height
                    Text {
                        anchors.centerIn: parent
                        text: modelData.label
                        color: seg.shown === modelData.key ? Theme.accent : Theme.textSecondary
                        font.pixelSize: 12
                        font.weight: Font.Medium
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: { seg.shown = modelData.key; seg.picked(modelData.key) }
                    }
                }
            }
        }
    }

    component Toggle: Rectangle {
        id: toggleRoot
        property bool checked: false
        signal toggled(bool on)
        width: 40; height: 22; radius: 11
        color: checked ? Qt.alpha(Theme.accent, 0.35) : Theme.lineSubtle
        Behavior on color { ColorAnimation { duration: Theme.standard } }
        Rectangle {
            width: 16; height: 16; radius: 8
            y: 3
            x: toggleRoot.checked ? toggleRoot.width - width - 3 : 3
            color: toggleRoot.checked ? Theme.accent : Theme.textSecondary
            Behavior on x { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
        }
        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: toggleRoot.toggled(!toggleRoot.checked)
        }
        Accessible.role: Accessible.CheckBox
        Accessible.checked: checked
    }
}

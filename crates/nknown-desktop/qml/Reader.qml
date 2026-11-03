// Lecteur intégré : le document entier, passage trouvé en vue, mots de la requête en couleur.
// Le texte se sélectionne ; la sélection se cherche dans NKnown (bouton, Ctrl+E, menu du clic
// droit). Ctrl+F cherche dans le document. Toujours un bouton pour ouvrir la source ailleurs
// (navigateur, éditeur) et, pour un fichier, pour le montrer dans son dossier.
import QtQuick
import QtQuick.Controls.Basic

Rectangle {
    id: reader
    /// Document chargé (JSON du pont), ou null pendant le chargement.
    property var doc: null
    property bool loading: false
    /// Mots de la requête à colorer, passage à mettre en vue.
    property var matched: []
    property string focusPassage: ""
    /// Dernier texte sélectionné, et texte cherché dans le document.
    property string selection: ""
    property string findText: ""
    /// Résultat courant de la recherche dans le texte (bloc, ou occurrence d'un fichier entier).
    property int findIndex: -1
    property int findCount: 0
    /// Ce qui s'affiche : le fichier entier s'il est lisible, sinon les passages indexés.
    readonly property bool whole: !!doc && doc.full !== null && doc.full !== undefined
    readonly property var blocks: !doc ? [] : whole
        ? [{ id: "full", text: doc.full, is_code: doc.is_code, code_lang: doc.code_lang }]
        : doc.passages

    signal closeRequested()
    signal searchText(string text)
    signal reveal(string path)

    function focusFind() { find.forceActiveFocus(); find.selectAll() }
    function searchSelection() {
        const s = reader.selection.trim()
        if (s !== "") reader.searchText(s.length > 200 ? s.slice(0, 200) : s)
    }

    /// Fait défiler le fichier entier jusqu'à une position (unités UTF-16, comme JavaScript).
    function scrollToPosition(pos) {
        const item = passages.itemAtIndex(0)
        if (!item) return
        const r = item.textItem.positionToRectangle(pos)
        const y = item.y + item.textItem.y + r.y - passages.height * 0.3
        passages.contentY = Math.max(passages.originY - passages.topMargin,
                                     Math.min(y, passages.contentHeight - passages.height + passages.originY))
    }

    /// Occurrence (fichier entier) ou passage suivant (ou précédent) du texte cherché.
    function findNext(step) {
        const needle = reader.findText.toLowerCase()
        if (reader.whole) {
            const hay = reader.doc.full.toLowerCase()
            const at = []
            if (needle !== "") for (let i = hay.indexOf(needle); i >= 0 && at.length < 10000; i = hay.indexOf(needle, i + 1)) at.push(i)
            reader.findCount = at.length
            if (at.length === 0) { reader.findIndex = -1; return }
            reader.findIndex = reader.findIndex < 0 ? (step > 0 ? 0 : at.length - 1)
                                                    : (reader.findIndex + step + at.length) % at.length
            reader.scrollToPosition(at[reader.findIndex])
            return
        }
        const ps = reader.doc ? reader.doc.passages : []
        reader.findCount = ps.length
        if (needle === "" || ps.length === 0) { reader.findIndex = -1; return }
        for (let k = 1; k <= ps.length; k++) {
            const i = ((reader.findIndex < 0 ? (step > 0 ? -1 : 0) : reader.findIndex) + step * k
                       + ps.length * k) % ps.length
            if (ps[i].text.toLowerCase().includes(needle)) {
                reader.findIndex = i
                passages.positionViewAtIndex(i, ListView.Center)
                return
            }
        }
        reader.findIndex = -1
    }

    function esc(t) {
        return t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }
    function pattern(words) {
        const w = words.filter(x => x.length > 1).map(x => x.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
        return w.length ? new RegExp("(" + w.map(x => reader.esc(x)).join("|") + ")", "gi") : null
    }
    /// Passage → texte enrichi : mots de la requête en couleur, texte cherché surligné.
    function render(text) {
        let html = reader.esc(text)
        const q = reader.pattern(reader.matched || [])
        if (q) html = html.replace(q, "<font color=\"" + Theme.kindColor("term") + "\">$1</font>")
        const f = reader.findText !== "" ? reader.pattern([reader.findText]) : null
        if (f) html = html.replace(f, "<span style=\"background-color:" + Qt.alpha(Theme.accent, 0.3) + "\">$1</span>")
        return "<div style=\"white-space:pre-wrap\">" + html + "</div>"
    }

    onDocChanged: {
        reader.selection = ""
        reader.findIndex = -1
        reader.findCount = 0
        if (!doc) return
        // Le passage trouvé en vue, une fois la liste remplie.
        Qt.callLater(() => {
            if (reader.whole) {
                if (doc.focus !== null && doc.focus !== undefined) reader.scrollToPosition(doc.focus)
                else passages.positionViewAtBeginning()
                return
            }
            for (let i = 0; i < doc.passages.length; i++)
                if (doc.passages[i].id === reader.focusPassage) {
                    passages.positionViewAtIndex(i, ListView.Beginning)
                    return
                }
            passages.positionViewAtBeginning()
        })
    }

    radius: Theme.radiusPanel
    color: Theme.bgRaised
    border.color: Theme.lineSubtle
    border.width: 1

    // Le panneau garde ses clics : rien n'atteint la carte ou la liste en dessous.
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (w) => w.accepted = true }

    Shortcut { enabled: reader.visible; sequence: "Ctrl+E"; onActivated: reader.searchSelection() }
    Shortcut { enabled: reader.visible; sequence: "F3"; onActivated: reader.findNext(1) }
    Shortcut { enabled: reader.visible; sequence: "Shift+F3"; onActivated: reader.findNext(-1) }

    Column {
        id: head
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Theme.margin
        spacing: 10

        Row {
            width: parent.width
            spacing: 14
            KindGlyph {
                anchors.verticalCenter: parent.verticalCenter
                kind: reader.doc && reader.doc.is_code ? "code" : "prose"
                size: 22
            }
            Text {
                width: parent.width - 22 - 14 - 14 - close.width
                anchors.verticalCenter: parent.verticalCenter
                text: reader.doc ? reader.doc.title : (reader.loading ? "…" : "")
                elide: Text.ElideRight
                color: Theme.textPrimary
                font.family: Theme.fontUi
                font.pixelSize: 19
                font.weight: Font.DemiBold
            }
            IconButton {
                id: close
                anchors.verticalCenter: parent.verticalCenter
                tip: "Fermer (Échap)"
                onClicked: reader.closeRequested()
            }
        }

        // Emplacement : chemin complet d'un fichier (sélectionnable), adresse sinon.
        TextEdit {
            width: parent.width
            visible: text !== ""
            readOnly: true
            selectByMouse: true
            wrapMode: TextEdit.WrapAnywhere
            text: reader.doc ? (reader.doc.path || reader.doc.url || "") : ""
            color: Theme.textSecondary
            selectionColor: Qt.alpha(Theme.accent, 0.35)
            selectedTextColor: Theme.textPrimary
            font.family: Theme.fontCode
            font.pixelSize: 12
        }

        Text {
            visible: !!reader.doc
            text: reader.doc ? [reader.doc.source, Theme.ago(reader.doc.date), reader.doc.lang,
                                reader.whole ? reader.doc.full.split("\n").length + " lignes"
                                             : reader.doc.passages.length + (reader.doc.passages.length > 1 ? " passages" : " passage")]
                                .filter(x => !!x).join("  ·  ") : ""
            color: Theme.textSecondary
            font.pixelSize: 11
            font.weight: Font.Medium
            font.features: { "tnum": 1 }
        }

        // Actions, puis recherche dans le document.
        Flow {
            width: parent.width
            spacing: 8
            Pill {
                visible: !!reader.doc && !!reader.doc.url
                text: reader.doc && reader.doc.path ? "Ouvrir dans l'éditeur" : "Ouvrir dans le navigateur"
                onClicked: Qt.openUrlExternally(reader.doc.url)
            }
            Pill {
                visible: !!reader.doc && !!reader.doc.path
                quiet: true
                text: "Montrer dans le dossier"
                onClicked: reader.reveal(reader.doc.path)
            }
            Pill {
                visible: reader.selection.trim() !== ""
                text: "Chercher « " + (reader.selection.trim().length > 24
                                        ? reader.selection.trim().slice(0, 24) + "…"
                                        : reader.selection.trim()) + " »"
                onClicked: reader.searchSelection()
            }
            // Chercher dans le document.
            Rectangle {
                width: 220
                height: 30
                radius: 15
                color: Theme.bgBase
                border.color: find.activeFocus ? Qt.alpha(Theme.accent, 0.6) : Theme.lineSubtle
                TextInput {
                    id: find
                    anchors.fill: parent
                    anchors.leftMargin: 14
                    anchors.rightMargin: 40
                    verticalAlignment: TextInput.AlignVCenter
                    color: Theme.textPrimary
                    selectionColor: Qt.alpha(Theme.accent, 0.35)
                    font.family: Theme.fontUi
                    font.pixelSize: 12
                    clip: true
                    onTextChanged: { reader.findText = text; reader.findIndex = -1; reader.findNext(1) }
                    onAccepted: reader.findNext(1)
                    Keys.onEscapePressed: (e) => { if (text !== "") { text = ""; e.accepted = true } else e.accepted = false }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: find.text === "" && !find.activeFocus
                        text: "Chercher dans le texte"
                        color: Theme.textSecondary
                        font: find.font
                    }
                }
                Text {
                    anchors.right: parent.right
                    anchors.rightMargin: 12
                    anchors.verticalCenter: parent.verticalCenter
                    visible: reader.findText !== ""
                    text: reader.findIndex < 0 ? "0" : (reader.findIndex + 1) + "/" + reader.findCount
                    color: reader.findIndex < 0 ? Theme.danger : Theme.textSecondary
                    font.pixelSize: 10
                    font.features: { "tnum": 1 }
                }
            }
        }
    }

    Rectangle {
        id: rule
        anchors.top: head.bottom
        anchors.topMargin: 14
        anchors.left: parent.left
        anchors.right: parent.right
        height: 1
        color: Theme.lineSubtle
    }

    ListView {
        id: passages
        anchors.top: rule.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.leftMargin: Theme.margin
        anchors.rightMargin: 8
        anchors.bottomMargin: 8
        clip: true
        spacing: 14
        topMargin: 16
        bottomMargin: 16
        model: reader.blocks
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
        // Rendu paresseux, mais assez d'avance pour un défilement fluide.
        cacheBuffer: 2000

        delegate: Item {
            id: row
            required property var modelData
            required property int index
            readonly property bool code: modelData.is_code
            readonly property bool current: !reader.whole
                && (modelData.id === reader.focusPassage || index === reader.findIndex)
            readonly property Item textItem: body
            width: ListView.view.width - 16
            height: box.height

            // Repère du passage trouvé.
            Rectangle {
                x: -12
                width: 3
                height: parent.height
                radius: 1.5
                color: Theme.accent
                opacity: row.current ? 0.9 : 0
                Behavior on opacity { NumberAnimation { duration: Theme.standard } }
            }
            Rectangle {
                id: box
                width: parent.width
                height: body.implicitHeight + (row.code ? 24 + (lang.visible ? 18 : 0) : 0)
                radius: 8
                color: row.code ? Theme.bgBase : "transparent"
                border.color: row.code ? Theme.lineSubtle : "transparent"
                Text {
                    id: lang
                    visible: row.code && !!row.modelData.code_lang
                    x: 12; y: 8
                    text: row.modelData.code_lang || ""
                    color: Theme.kindColor("code")
                    font.pixelSize: 10
                    font.weight: Font.Medium
                }
                TextEdit {
                    id: body
                    x: row.code ? 12 : 0
                    y: row.code ? (lang.visible ? 26 : 12) : 0
                    width: parent.width - (row.code ? 24 : 0)
                    readOnly: true
                    selectByMouse: true
                    persistentSelection: false
                    textFormat: TextEdit.RichText
                    wrapMode: row.code ? TextEdit.WrapAnywhere : TextEdit.Wrap
                    text: reader.render(row.modelData.text)
                    color: Theme.textPrimary
                    selectionColor: Qt.alpha(Theme.accent, 0.35)
                    selectedTextColor: Theme.textPrimary
                    font.family: row.code ? Theme.fontCode : Theme.fontUi
                    font.pixelSize: row.code ? 13 : 14
                    onSelectedTextChanged: if (selectedText !== "") reader.selection = selectedText
                    // Clic droit : chercher la sélection, ou copier.
                    TapHandler {
                        acceptedButtons: Qt.RightButton
                        onTapped: menu.popup()
                    }
                    Menu {
                        id: menu
                        MenuItem {
                            text: "Chercher dans NKnown"
                            enabled: reader.selection.trim() !== ""
                            onTriggered: reader.searchSelection()
                        }
                        MenuItem {
                            text: "Copier"
                            enabled: body.selectedText !== ""
                            onTriggered: body.copy()
                        }
                    }
                }
            }
        }
    }

    // Chargement.
    Text {
        anchors.centerIn: parent
        visible: reader.loading && !reader.doc
        text: "Lecture…"
        color: Theme.textSecondary
        font.pixelSize: 13
    }
}

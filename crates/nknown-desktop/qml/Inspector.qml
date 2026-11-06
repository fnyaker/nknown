// Inspecteur d'un nœud (charte, section 6) : ce qu'il est, ses actions, le passage trouvé et ses
// liens triés par poids. Un seul contact : pointeur et focus partagent le même rendu.
import QtQuick
import QtQuick.Controls.Basic

Rectangle {
    id: panel
    /// Nœud inspecté (données de la carte) et ses liens (MapView.linksOf).
    property var node: null
    property var links: []
    /// Résultat correspondant au nœud (liste des résultats), s'il en est un.
    property var hit: null
    /// Contact courant de la carte.
    property string contactEdge: ""
    property string contactNode: ""
    signal closeRequested()
    signal linkPicked(string id)
    signal searchWord(string word)
    signal read()
    signal reveal(string path)
    signal hoverEdge(string edge, bool on)
    signal focusEdge(string edge, bool on)
    signal hoverNode(string id, bool on)

    radius: Theme.radiusPanel
    color: Theme.bgRaised
    border.color: Theme.lineSubtle
    border.width: 1

    // Le panneau garde ses clics : rien n'atteint la liste en dessous.
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (w) => w.accepted = true }

    /// Un lien est chaud si c'est le lien exact ou si le nœud au contact en est une extrémité.
    function hotLink(link) {
        if (!link) return false
        if (contactEdge !== "") return contactEdge === link.edge
        return contactNode !== "" && (link.from === contactNode || link.to === contactNode)
    }

    function esc(text) {
        return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    }
    function isWordChar(c) {
        return Theme.isWordChar(c)
    }

    /// Mot de `matched` dans un jeton (mot + ponctuation), aux limites de mot.
    function matchIn(token) {
        const lower = token.toLowerCase()
        if (lower.length !== token.length) return null
        let best = null
        for (const raw of (panel.hit ? (panel.hit.matched || []) : [])) {
            if (raw.length <= 1) continue
            const word = raw.toLowerCase()
            let from = 0
            while (true) {
                const at = lower.indexOf(word, from)
                if (at < 0) break
                const end = at + word.length
                const before = at === 0 || !isWordChar(token[at - 1])
                const after = end >= token.length || !isWordChar(token[end])
                if (before && after && (!best || word.length > best.length)) {
                    best = { start: at, length: word.length, word: token.slice(at, end) }
                    break
                }
                from = at + 1
            }
        }
        return best
    }

    function linkForWord(word) {
        const key = word.toLowerCase()
        let best = null
        for (const l of panel.links) {
            if (l.kind !== "term_hit") continue
            if ((l.node.label || "").toLowerCase() !== key) continue
            if (!best || l.weight > best.weight) best = l
        }
        return best
    }

    /// Prose : jetons séparés par les espaces, pour un survol par mot sans découper la mise en
    /// page. Les retours et espaces multiples sont normalisés.
    function buildTokens(text) {
        const words = text.split(/\s+/).filter(w => w !== "")
        const tokens = []
        for (let i = 0; i < words.length; i++) {
            const body = words[i]
            const match = matchIn(body)
            const link = match ? linkForWord(match.word) : null
            tokens.push({
                prefix: match ? body.slice(0, match.start) : "",
                word: match ? body.slice(match.start, match.start + match.length) : body,
                suffix: match ? body.slice(match.start + match.length) : "",
                matched: !!match,
                link: link ? { edge: link.edge, from: link.from, to: link.to } : null,
                space: false
            })
            if (i + 1 < words.length)
                tokens.push({ prefix: "", word: "", suffix: "", matched: false,
                              link: null, space: true })
        }
        return tokens
    }

    /// Code : un seul texte enrichi, espaces et retours conservés ; pas de survol par mot.
    function codeHtml(text) {
        const lower = text.toLowerCase()
        if (lower.length !== text.length) {
            return "<pre>" + esc(text) + "</pre>"
        }
        const words = (panel.hit.matched || []).filter(w => w.length > 1)
        let out = ""
        let last = 0
        let i = 0
        while (i < text.length) {
            let word = null
            if (i === 0 || !isWordChar(text[i - 1])) {
                for (const raw of words) {
                    const w = raw.toLowerCase()
                    if (word !== null && w.length <= word.length) continue
                    if (lower.startsWith(w, i)) {
                        const end = i + w.length
                        if (end >= text.length || !isWordChar(text[end])) word = raw
                    }
                }
            }
            if (word === null) {
                i++
                continue
            }
            out += esc(text.slice(last, i))
            out += "<font color=\"" + Theme.kindColor("term") + "\">"
                + esc(text.slice(i, i + word.length)) + "</font>"
            i += word.length
            last = i
        }
        out += esc(text.slice(last))
        return "<pre>" + out + "</pre>"
    }

    function updatePassage() {
        const text = panel.hit
            ? ((panel.hit.passage_text || "") !== "" ? panel.hit.passage_text : (panel.hit.snippet || ""))
            : ""
        passage.plain = text
        passage.html = ""
        passage.tokens = []
        passage.firstToken = -1
        revealTimer.stop()
        passageScroll.contentY = 0
        passageScroll.revealTries = 0
        if (text === "") return
        if (panel.hit.is_code) {
            passage.html = codeHtml(text)
            return
        }
        const tokens = buildTokens(text)
        passage.tokens = tokens
        for (let i = 0; i < tokens.length; i++)
            if (tokens[i].matched) { passage.firstToken = i; break }
        revealTimer.start()
    }

    onHitChanged: updatePassage()
    onLinksChanged: updatePassage()
    Component.onCompleted: updatePassage()

    Column {
        id: content
        anchors.fill: parent
        anchors.margins: Theme.margin
        spacing: 16

        Row {
            spacing: 14
            width: parent.width
            KindGlyph {
                anchors.verticalCenter: parent.verticalCenter
                kind: panel.node ? panel.node.kind : "term"
                size: 26
            }
            Column {
                id: titleBlock
                width: parent.width - 26 - 14 - 14 - 30
                spacing: 2
                HoverHandler {
                    onHoveredChanged: panel.hoverNode(panel.node ? panel.node.id : "", hovered)
                }
                Text {
                    width: parent.width
                    text: panel.node ? (panel.node.label || "requête") : ""
                    elide: Text.ElideRight
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: 17
                    font.weight: Font.DemiBold
                }
                Text {
                    text: panel.node ? Theme.kindName(panel.node.kind) : ""
                    color: Theme.kindColor(panel.node ? panel.node.kind : "term")
                    font.family: Theme.fontUi
                    font.pixelSize: 11
                    font.weight: Font.Medium
                }
                // Fichier : la fin de son chemin.
                Text {
                    width: parent.width
                    visible: !!panel.hit && !!panel.hit.location
                    text: panel.hit && panel.hit.location ? panel.hit.location : ""
                    elide: Text.ElideLeft
                    color: Theme.textSecondary
                    font.family: Theme.fontCode
                    font.pixelSize: 11
                }
            }
            // Fermer (Échap fait de même).
            IconButton {
                anchors.verticalCenter: parent.verticalCenter
                tip: "Fermer (Échap)"
                onClicked: panel.closeRequested()
            }
        }

        // Actions.
        Flow {
            width: parent.width
            spacing: 8
            Pill {
                visible: !!panel.hit
                text: "Lire"
                onClicked: panel.read()
            }
            Pill {
                visible: !!panel.hit && !!panel.hit.url
                quiet: true
                text: panel.hit && panel.hit.path ? "Ouvrir dans l'éditeur" : "Ouvrir"
                onClicked: Qt.openUrlExternally(panel.hit.url)
            }
            Pill {
                visible: !!panel.hit && !!panel.hit.path
                quiet: true
                text: "Dossier"
                onClicked: panel.reveal(panel.hit.path)
            }
            Pill {
                visible: !!panel.node && (panel.node.kind === "term" || panel.node.kind === "qualifier")
                text: "Chercher ce mot"
                onClicked: panel.searchWord(panel.node.label)
            }
        }

        Rectangle { width: parent.width; height: 1; color: Theme.lineSubtle }

        // ------------------------------------------------------------- carte de passage
        Rectangle {
            id: passage
            property string plain: ""
            property string html: ""
            property var tokens: []
            property int firstToken: -1
            readonly property real cap: Math.min(Theme.passageMax,
                Math.max(Theme.passageMin, panel.height * 0.32))
            readonly property real wanted: meta.implicitHeight + 32
                + (panel.hit && panel.hit.is_code ? codeText.contentHeight : proseFlow.height)

            visible: plain !== ""
            width: parent.width
            height: visible ? Math.min(cap, wanted) : 0
            color: Theme.bgBase
            border.color: passageScroll.activeFocus ? Qt.alpha(Theme.accent, 0.55) : Theme.lineSubtle
            border.width: 1
            radius: Theme.radiusCard
            clip: true

            Text {
                id: meta
                x: 14
                y: 12
                width: parent.width - 28
                textFormat: Text.PlainText
                text: {
                    if (!panel.hit) return ""
                    const bits = [panel.hit.source]
                    if (panel.hit.date) bits.push(Theme.ago(panel.hit.date))
                    if (panel.hit.code_lang) bits.push(panel.hit.code_lang)
                    if (panel.hit.translated_from) bits.push("traduit de l'" + panel.hit.translated_from)
                    return bits.filter(x => !!x).join("  ·  ")
                }
                elide: Text.ElideRight
                color: Theme.textSecondary
                font.family: Theme.fontUi
                font.pixelSize: 11
                font.weight: Font.Medium
                font.features: { "tnum": 1 }
            }

            Flickable {
                id: passageScroll
                x: 14
                y: meta.y + meta.height + 8
                width: parent.width - 28
                height: Math.max(0, passage.height - y - 12)
                contentWidth: width
                contentHeight: panel.hit && panel.hit.is_code ? codeText.contentHeight : proseFlow.height
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                activeFocusOnTab: true
                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                // La mise en page du Flow arrive en plusieurs passes : réessayer brièvement.
                property int revealTries: 0
                Timer {
                    id: revealTimer
                    interval: 80
                    repeat: true
                    running: false
                    onTriggered: passageScroll.revealFirst()
                }
                function revealFirst() {
                    if (revealTries++ > 30) {
                        revealTimer.stop()
                        return
                    }
                    if (panel.hit && panel.hit.is_code) {
                        revealTimer.stop()
                        return
                    }
                    if (passage.firstToken < 0) {
                        revealTimer.stop()
                        return
                    }
                    if (height <= 0 || proseFlow.height <= 0) return
                    if (proseFlow.height <= height) {
                        revealTimer.stop()
                        return
                    }
                    const item = tokenRepeater.itemAt(passage.firstToken)
                    if (!item) return
                    // Le jeton existe mais n'est pas encore placé par le Flow.
                    if (item.y <= 0 && passage.firstToken > 0) return
                    contentY = Math.max(0, Math.min(contentHeight - height, item.y - height / 3))
                    revealTimer.stop()
                }
                onContentHeightChanged: revealFirst()
                onHeightChanged: revealFirst()
                Keys.onPressed: (event) => {
                    const page = Math.max(48, height * 0.8)
                    const end = Math.max(0, contentHeight - height)
                    if (event.key === Qt.Key_Down) { contentY = Math.min(end, contentY + 24); event.accepted = true }
                    else if (event.key === Qt.Key_Up) { contentY = Math.max(0, contentY - 24); event.accepted = true }
                    else if (event.key === Qt.Key_PageDown) { contentY = Math.min(end, contentY + page); event.accepted = true }
                    else if (event.key === Qt.Key_PageUp) { contentY = Math.max(0, contentY - page); event.accepted = true }
                    else if (event.key === Qt.Key_Home) { contentY = 0; event.accepted = true }
                    else if (event.key === Qt.Key_End) { contentY = end; event.accepted = true }
                }
                Accessible.role: Accessible.Grouping
                Accessible.name: "Passage trouvé"
                Accessible.description: passage.plain

                // Code : un seul texte pré-formaté ; pas de survol par mot.
                Text {
                    id: codeText
                    visible: panel.hit && panel.hit.is_code
                    width: passageScroll.width
                    text: passage.html
                    textFormat: Text.StyledText
                    wrapMode: Text.WrapAnywhere
                    lineHeight: 1.45
                    color: Theme.textSecondary
                    font.family: Theme.fontCode
                    font.pixelSize: 12
                }

                // Prose : un jeton par mot, pour un survol exact sans découper la mise en page.
                Flow {
                    id: proseFlow
                    visible: !(panel.hit && panel.hit.is_code)
                    width: passageScroll.width
                    Repeater {
                        id: tokenRepeater
                        model: passage.tokens
                        delegate: Row {
                            id: wordToken
                            required property var modelData
                            readonly property bool isSpace: modelData.space === true
                            readonly property bool hot: !!modelData.link
                                && panel.hotLink(modelData.link)
                            width: isSpace ? 4 : implicitWidth
                            height: isSpace ? 19 : implicitHeight
                            spacing: 0
                            Text {
                                id: prefix
                                visible: !wordToken.isSpace && wordToken.modelData.prefix !== ""
                                text: wordToken.isSpace ? "" : wordToken.modelData.prefix
                                textFormat: Text.PlainText
                                lineHeight: 1.45
                                color: Theme.textSecondary
                                font.family: Theme.fontUi
                                font.pixelSize: 13
                            }
                            Text {
                                id: word
                                visible: !wordToken.isSpace
                                text: wordToken.isSpace ? "" : wordToken.modelData.word
                                textFormat: Text.PlainText
                                lineHeight: 1.45
                                color: wordToken.isSpace ? Theme.textSecondary
                                    : (wordToken.modelData.matched ? Theme.kindColor("term")
                                                                   : Theme.textSecondary)
                                font.family: Theme.fontUi
                                font.pixelSize: 13
                                font.underline: wordToken.hot
                                HoverHandler {
                                    enabled: !!wordToken.modelData.link
                                    onHoveredChanged: {
                                        if (!wordToken.modelData.link) return
                                        if (hovered) panel.hoverEdge(wordToken.modelData.link.edge, true)
                                        else if (panel.contactEdge === wordToken.modelData.link.edge)
                                            panel.hoverEdge(wordToken.modelData.link.edge, false)
                                    }
                                }
                            }
                            Text {
                                id: suffix
                                visible: !wordToken.isSpace && wordToken.modelData.suffix !== ""
                                text: wordToken.isSpace ? "" : wordToken.modelData.suffix
                                textFormat: Text.PlainText
                                lineHeight: 1.45
                                color: Theme.textSecondary
                                font.family: Theme.fontUi
                                font.pixelSize: 13
                            }
                            Accessible.ignored: true
                        }
                    }
                }
            }
        }

        // Liens, du plus lourd au plus léger.
        ListView {
            id: linksList
            width: parent.width
            height: Math.max(0, panel.height - y - 2 * Theme.margin)
            clip: true
            spacing: 2
            model: panel.links
            ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }
            delegate: Item {
                id: row
                required property var modelData
                required property int index
                readonly property bool hot: panel.hotLink(modelData)
                width: ListView.view.width
                height: 34
                activeFocusOnTab: true
                onActiveFocusChanged: {
                    panel.focusEdge(modelData.edge, activeFocus)
                    if (activeFocus) linksList.positionViewAtIndex(index, ListView.Contain)
                }
                Keys.onReturnPressed: panel.linkPicked(modelData.id)
                Keys.onEnterPressed: panel.linkPicked(modelData.id)

                Rectangle {
                    anchors.fill: parent
                    radius: 8
                    color: row.hot || rowArea.containsMouse || row.activeFocus ? Theme.bgHover : "transparent"
                    border.width: row.activeFocus ? 1 : 0
                    border.color: Qt.alpha(Theme.accent, 0.6)
                }
                KindGlyph {
                    id: g
                    anchors.left: parent.left
                    anchors.leftMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    kind: modelData.node.kind
                    size: 10
                }
                Text {
                    anchors.left: g.right
                    anchors.leftMargin: 10
                    anchors.right: bar.left
                    anchors.rightMargin: 10
                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData.node.label || "requête"
                    elide: Text.ElideRight
                    color: Theme.textPrimary
                    font.family: Theme.fontUi
                    font.pixelSize: 13
                }
                // Poids du lien.
                Rectangle {
                    id: bar
                    anchors.right: parent.right
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    width: 64
                    height: 4
                    radius: 2
                    color: Theme.lineSubtle
                    Rectangle {
                        width: parent.width * Math.max(0.04, modelData.weight)
                        height: parent.height
                        radius: 2
                        color: Theme.kindColor(modelData.node.kind)
                    }
                }
                MouseArea {
                    id: rowArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onContainsMouseChanged: panel.hoverEdge(modelData.edge, containsMouse)
                    onClicked: {
                        row.forceActiveFocus()
                        panel.linkPicked(modelData.id)
                    }
                }
                Accessible.role: Accessible.Link
                Accessible.name: "Lien vers " + (modelData.node.label || "requête")
                    + ", " + Theme.kindName(modelData.node.kind)
                    + ", poids " + Math.round(modelData.weight * 100) + " %"
            }
        }
    }
}

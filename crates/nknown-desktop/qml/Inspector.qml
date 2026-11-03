// Inspecteur d'un nœud (charte, section 6) : ce qu'il est (forme et couleur en en-tête), ses
// liens triés par poids (barres), ses actions.
import QtQuick

Rectangle {
    id: panel
    /// Nœud inspecté (données de la carte) et ses liens (MapView.linksOf).
    property var node: null
    property var links: []
    /// Adresse à ouvrir, si le nœud est un résultat qui en a une.
    property string url: ""
    signal closeRequested()
    signal linkPicked(string id)
    signal searchWord(string word)

    radius: Theme.radiusPanel
    color: Theme.bgRaised
    border.color: Theme.lineSubtle
    border.width: 1

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
                width: parent.width - 26 - 14 - 28
                spacing: 2
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
            }
            // Fermer (Échap fait de même).
            Rectangle {
                width: 28
                height: 28
                radius: 14
                color: closeHover.hovered ? Theme.bgHover : "transparent"
                Text {
                    anchors.centerIn: parent
                    text: "✕"
                    color: Theme.textSecondary
                    font.pixelSize: 13
                }
                HoverHandler { id: closeHover; cursorShape: Qt.PointingHandCursor }
                TapHandler { onTapped: panel.closeRequested() }
                Accessible.role: Accessible.Button
                Accessible.name: "Fermer"
            }
        }

        // Actions.
        Row {
            spacing: 8
            visible: children.length > 0
            Repeater {
                model: [
                    { label: "Ouvrir", show: panel.url !== "", act: () => Qt.openUrlExternally(panel.url) },
                    { label: "Chercher ce mot",
                      show: !!panel.node && (panel.node.kind === "term" || panel.node.kind === "qualifier"),
                      act: () => panel.searchWord(panel.node.label) }
                ].filter(a => a.show)
                Rectangle {
                    required property var modelData
                    height: 30
                    width: actText.implicitWidth + 24
                    radius: 15
                    color: actHover.hovered ? Qt.alpha(Theme.accent, 0.2) : Qt.alpha(Theme.accent, 0.12)
                    Text {
                        id: actText
                        anchors.centerIn: parent
                        text: modelData.label
                        color: Theme.accent
                        font.family: Theme.fontUi
                        font.pixelSize: 12
                        font.weight: Font.Medium
                    }
                    HoverHandler { id: actHover; cursorShape: Qt.PointingHandCursor }
                    TapHandler { onTapped: modelData.act() }
                }
            }
        }

        Rectangle { width: parent.width; height: 1; color: Theme.lineSubtle }

        // Liens, du plus lourd au plus léger.
        ListView {
            width: parent.width
            height: panel.height - y - 2 * Theme.margin
            clip: true
            spacing: 2
            model: panel.links
            delegate: Item {
                required property var modelData
                width: ListView.view.width
                height: 34
                Rectangle {
                    anchors.fill: parent
                    radius: 8
                    color: rowHover.hovered ? Theme.bgHover : "transparent"
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
                HoverHandler { id: rowHover; cursorShape: Qt.PointingHandCursor }
                TapHandler { onTapped: panel.linkPicked(modelData.id) }
            }
        }
    }
}

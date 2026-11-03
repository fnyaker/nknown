// Inspecteur d'un nœud (charte, section 6) : ce qu'il est (forme et couleur en en-tête), ses
// liens triés par poids (barres), ses actions.
import QtQuick

Rectangle {
    id: panel
    /// Nœud inspecté (données de la carte) et ses liens (MapView.linksOf).
    property var node: null
    property var links: []
    /// Résultat correspondant au nœud (liste des résultats), s'il en est un.
    property var hit: null
    signal closeRequested()
    signal linkPicked(string id)
    signal searchWord(string word)
    signal read()
    signal reveal(string path)

    radius: Theme.radiusPanel
    color: Theme.bgRaised
    border.color: Theme.lineSubtle
    border.width: 1

    // Le panneau garde ses clics : rien n'atteint la liste en dessous.
    MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons; onWheel: (w) => w.accepted = true }

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
                width: parent.width - 26 - 14 - 14 - 30
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
                    color: rowArea.containsMouse ? Theme.bgHover : "transparent"
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
                    onClicked: panel.linkPicked(modelData.id)
                }
            }
        }
    }
}

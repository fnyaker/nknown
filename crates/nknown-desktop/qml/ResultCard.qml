// Carte de résultat (charte, section 7) : barre de score à gauche, nature, titre, source et date,
// extrait de deux lignes avec les mots de la requête en couleur.
import QtQuick

Rectangle {
    id: card
    required property var hit
    property bool selected: false
    /// Nœud ou lien au contact (survol, focus ou sélection dans la carte).
    property bool contact: false
    signal picked()
    /// Double clic : lire le document.
    signal read()
    signal hoverContact(bool on)
    signal focusContact(bool on)
    /// Focus clavier obtenu : la liste peut amener la carte en vue.
    signal focused()

    implicitHeight: body.implicitHeight + 24
    radius: Theme.radiusCard
    color: selected || contact ? Theme.bgHover
         : hover.containsMouse ? Qt.alpha(Theme.bgHover, 0.7) : Theme.bgRaised
    border.color: selected ? Qt.alpha(Theme.accent, 0.6) : Theme.lineSubtle
    border.width: 1
    activeFocusOnTab: true
    Behavior on color { ColorAnimation { duration: Theme.quick } }
    Behavior on border.color { ColorAnimation { duration: Theme.quick } }

    // Anneau de focus : le survol et le focus produisent le même contact, seul le focus a un anneau.
    Rectangle {
        anchors.fill: parent
        anchors.margins: -2
        radius: parent.radius + 2
        color: "transparent"
        border.width: 2
        border.color: card.activeFocus ? Qt.alpha(Theme.accent, 0.7) : "transparent"
        Behavior on border.color { ColorAnimation { duration: Theme.quick } }
    }

    Keys.onReturnPressed: card.picked()
    Keys.onEnterPressed: card.picked()
    onActiveFocusChanged: {
        card.focusContact(activeFocus)
        if (activeFocus) card.focused()
    }

    // Score relatif au meilleur résultat.
    Rectangle {
        anchors.left: parent.left
        anchors.leftMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        width: 3
        radius: 1.5
        height: Math.max(6, (parent.height - 24) * card.hit.relative)
        color: Theme.kindColor(card.hit.is_code ? "code" : "prose")
        opacity: 0.85
    }

    Column {
        id: body
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.leftMargin: 22
        anchors.rightMargin: 14
        anchors.topMargin: 12
        spacing: 5

        Row {
            spacing: 8
            width: parent.width
            KindGlyph {
                anchors.verticalCenter: parent.verticalCenter
                kind: card.hit.is_code ? "code" : "prose"
                size: 10
            }
            Text {
                width: parent.width - 18
                text: card.hit.title
                elide: Text.ElideRight
                color: Theme.textPrimary
                font.family: Theme.fontUi
                font.pixelSize: 15
                font.weight: Font.DemiBold
            }
        }
        Text {
            width: parent.width
            // Un fichier se reconnaît à la fin de son chemin.
            text: (card.hit.location || card.hit.source) + (card.hit.date ? "  ·  " + Theme.ago(card.hit.date) : "")
                  + (card.hit.code_lang ? "  ·  " + card.hit.code_lang : "")
                  + (card.hit.translated_from ? "  ·  traduit de l'" + card.hit.translated_from : "")
            elide: Text.ElideMiddle
            color: Theme.textSecondary
            font.family: Theme.fontUi
            font.pixelSize: 11
            font.weight: Font.Medium
            font.features: { "tnum": 1 }
        }
        Text {
            width: parent.width
            text: Theme.highlight(card.hit.snippet, card.hit.matched)
            textFormat: Text.StyledText
            wrapMode: Text.Wrap
            maximumLineCount: 2
            elide: Text.ElideRight
            lineHeight: 1.45
            color: Theme.textSecondary
            font.family: card.hit.is_code ? Theme.fontCode : Theme.fontUi
            font.pixelSize: card.hit.is_code ? 12.5 : 13
        }
    }

    MouseArea {
        id: hover
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onContainsMouseChanged: card.hoverContact(containsMouse)
        onClicked: {
            card.forceActiveFocus()
            card.picked()
        }
        onDoubleClicked: card.read()
    }
    Accessible.role: Accessible.ListItem
    Accessible.name: hit.title + (hit.source ? " — " + hit.source : "")
}

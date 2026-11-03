// Bouton en pilule (charte, section 6) : texte court, teinte d'accent. Un MouseArea accepte
// le clic : rien ne le traverse vers ce qui est dessous.
import QtQuick

Rectangle {
    id: pill
    property string text: ""
    /// Variante discrète (contour seul), pour les actions secondaires.
    property bool quiet: false
    signal clicked()

    implicitHeight: 30
    implicitWidth: label.implicitWidth + 24
    radius: height / 2
    opacity: enabled ? 1 : 0.4
    color: quiet ? (area.containsMouse ? Theme.bgHover : "transparent")
                 : Qt.alpha(Theme.accent, area.pressed ? 0.28 : area.containsMouse ? 0.2 : 0.12)
    border.color: quiet ? Theme.lineSubtle : "transparent"
    border.width: quiet ? 1 : 0
    Behavior on color { ColorAnimation { duration: Theme.quick } }

    Text {
        id: label
        anchors.centerIn: parent
        text: pill.text
        color: pill.quiet ? Theme.textPrimary : Theme.accent
        font.family: Theme.fontUi
        font.pixelSize: 12
        font.weight: Font.Medium
    }
    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: pill.clicked()
    }
    Accessible.role: Accessible.Button
    Accessible.name: text
}

// Bouton rond à pictogramme (fermer, retour…). Le clic est accepté ici : un double clic ne
// déclenche rien en dessous.
import QtQuick
import QtQuick.Controls.Basic

Rectangle {
    id: button
    property string glyph: "✕"
    property string tip: ""
    signal clicked()

    width: 30
    height: 30
    radius: 15
    activeFocusOnTab: true
    color: area.pressed ? Theme.lineSubtle : area.containsMouse ? Theme.bgHover : "transparent"
    border.width: activeFocus ? 1 : 0
    border.color: Qt.alpha(Theme.accent, 0.6)
    Behavior on color { ColorAnimation { duration: Theme.quick } }
    Keys.onReturnPressed: button.clicked()
    Keys.onEnterPressed: button.clicked()
    Text {
        anchors.centerIn: parent
        text: button.glyph
        color: area.containsMouse ? Theme.textPrimary : Theme.textSecondary
        font.pixelSize: 14
    }
    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: button.clicked()
        onDoubleClicked: {}
    }
    ToolTip.visible: tip !== "" && area.containsMouse
    ToolTip.delay: 600
    ToolTip.text: tip
    Accessible.role: Accessible.Button
    Accessible.name: tip
}

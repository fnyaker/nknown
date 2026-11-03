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
    color: area.pressed ? Theme.lineSubtle : area.containsMouse ? Theme.bgHover : "transparent"
    Behavior on color { ColorAnimation { duration: Theme.quick } }
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

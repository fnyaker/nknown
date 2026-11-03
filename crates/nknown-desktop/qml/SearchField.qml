// Champ de recherche : sans bordure, un trait dessous. Ce trait est aussi la jauge de la
// recherche, en trois segments qui se remplissent dans l'ordre (analyse, index, carte) ; il ne
// s'allume qu'au focus et pendant une recherche (charte, sections 6 et 7).
import QtQuick
import QtQuick.Controls.Basic

Item {
    id: root
    property bool compact: false
    /// "", "analyze", "search", "graph" ou "done".
    property string phase: ""
    property string intent: ""
    property alias text: input.text
    signal submitted(string query)

    implicitHeight: input.implicitHeight + 14
    readonly property int step: ({ "": 0, "analyze": 1, "search": 2, "graph": 3, "done": 3 })[phase] ?? 0
    readonly property bool running: phase !== "" && phase !== "done"

    function focusInput() { input.forceActiveFocus() }

    TextField {
        id: input
        anchors.left: parent.left
        anchors.right: pill.visible ? pill.left : parent.right
        anchors.rightMargin: pill.visible ? 12 : 0
        anchors.top: parent.top
        placeholderText: "Rechercher"
        placeholderTextColor: Theme.textSecondary
        color: Theme.textPrimary
        selectionColor: Qt.alpha(Theme.accent, 0.35)
        selectedTextColor: Theme.textPrimary
        font.family: Theme.fontUi
        font.pixelSize: root.compact ? 18 : 28
        font.weight: root.compact ? Font.Normal : Font.Light
        horizontalAlignment: root.compact ? TextInput.AlignLeft : TextInput.AlignHCenter
        leftPadding: 2
        rightPadding: 2
        background: null
        focus: true
        Behavior on font.pixelSize { NumberAnimation { duration: Theme.emphasis; easing.type: Easing.OutExpo } }
        onAccepted: if (text.trim().length > 0) root.submitted(text.trim())
        Accessible.name: "Rechercher"
    }

    // Intention reconnue (actu, code…) : une pastille dans le champ.
    Rectangle {
        id: pill
        visible: root.intent !== ""
        anchors.right: parent.right
        anchors.verticalCenter: input.verticalCenter
        height: 22
        width: pillText.implicitWidth + 18
        radius: height / 2
        color: Qt.alpha(Theme.accent, 0.12)
        Text {
            id: pillText
            anchors.centerIn: parent
            text: root.intent
            color: Theme.accent
            font.family: Theme.fontUi
            font.pixelSize: 11
            font.weight: Font.Medium
        }
    }

    // Trait / jauge.
    Row {
        id: gauge
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 2
        // Un trait continu au repos ; trois segments (les étapes) pendant la recherche.
        spacing: root.running ? 4 : 0
        Behavior on spacing { NumberAnimation { duration: Theme.standard } }
        Repeater {
            model: 3
            Rectangle {
                required property int index
                width: (gauge.width - 2 * gauge.spacing) / 3
                height: 2
                radius: 1
                color: input.activeFocus ? Qt.alpha(Theme.accent, 0.22) : Theme.lineSubtle
                Behavior on color { ColorAnimation { duration: Theme.standard } }
                Rectangle {
                    // Remplissage de l'étape, avec une lueur tant qu'elle est en cours.
                    height: parent.height
                    radius: 1
                    color: Theme.accent
                    width: root.step > index ? parent.width : 0
                    opacity: root.phase === "done" ? 0 : 1
                    Behavior on width { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
                    Behavior on opacity { NumberAnimation { duration: Theme.emphasis } }
                    Rectangle {
                        visible: root.running && root.step === index + 1
                        anchors.centerIn: parent
                        width: parent.width
                        height: 6
                        radius: 3
                        color: Qt.alpha(Theme.accent, 0.25)
                    }
                }
            }
        }
    }
}

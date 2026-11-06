// Curseur de variété (charte, section 7) : trois positions — Strict, Normal, Diversifié.
// Une seule teinte d'accent ; le libellé dit la position courante, l'infobulle l'explique.
// Collé sous le trait du champ, il se règle au clic, au clavier et au lecteur d'écran.
import QtQuick
import QtQuick.Controls.Basic

Item {
    id: root
    /// "strict", "normal" ou "diverse" (valeurs du corps de recherche).
    property string value: "normal"
    signal picked(string value)

    readonly property var values: ["strict", "normal", "diverse"]
    readonly property var labels: ["Strict", "Normal", "Diversifié"]
    readonly property var hints: [
        "Liens de sens très proches uniquement ; moins de résultats, plus précis.",
        "Élargissement habituel.",
        "Liens plus larges, définitions et textes liés ; plus de résultats, plus de bruit possible."
    ]
    readonly property int index: {
        const i = values.indexOf(value)
        return i < 0 ? 1 : i
    }
    readonly property int inset: Theme.varietyKnob / 2
    readonly property real knobCenter: inset + (Theme.varietyTrack - Theme.varietyKnob) * index / 2

    implicitWidth: Theme.varietyTrack + 8 + Theme.varietyLabel
    implicitHeight: 20
    activeFocusOnTab: true

    function pick(i) {
        const next = values[Math.max(0, Math.min(2, i))]
        if (next !== value) root.picked(next)
    }
    function step(delta) { pick(index + delta) }
    function pickAt(x) {
        pick(Math.round(Math.max(0, Math.min(1, x / Theme.varietyTrack)) * 2))
    }

    // Un seul arrêt de tabulation ; les flèches et Origine/Fin règlent la position.
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Left || event.key === Qt.Key_Up) {
            root.step(-1)
            event.accepted = true
        } else if (event.key === Qt.Key_Right || event.key === Qt.Key_Down) {
            root.step(1)
            event.accepted = true
        } else if (event.key === Qt.Key_Home) {
            root.pick(0)
            event.accepted = true
        } else if (event.key === Qt.Key_End) {
            root.pick(2)
            event.accepted = true
        }
    }

    // Contour de focus : la lueur ne sert qu'au focus (charte, section 2).
    Rectangle {
        anchors.fill: parent
        anchors.margins: -4
        radius: 8
        color: "transparent"
        border.width: 1
        border.color: root.activeFocus ? Qt.alpha(Theme.accent, 0.55) : "transparent"
        Behavior on border.color { ColorAnimation { duration: Theme.quick } }
    }

    // Piste fine, repères de 4 px, bouton de 12 px.
    Rectangle {
        id: track
        anchors.verticalCenter: parent.verticalCenter
        width: Theme.varietyTrack
        height: 2
        radius: 1
        color: Theme.lineSubtle
    }
    Repeater {
        model: 3
        Rectangle {
            required property int index
            anchors.verticalCenter: parent.verticalCenter
            x: root.inset + (Theme.varietyTrack - Theme.varietyKnob) * index / 2 - width / 2
            width: Theme.varietyTick
            height: width
            radius: width / 2
            color: Theme.textSecondary
            opacity: 0.55
        }
    }
    Rectangle {
        id: knob
        anchors.verticalCenter: parent.verticalCenter
        width: Theme.varietyKnob
        height: width
        radius: width / 2
        color: Theme.accent
        x: root.knobCenter - width / 2
        Behavior on x { enabled: !dragHandler.active; NumberAnimation { duration: Theme.quick; easing.type: Easing.OutCubic } }
    }

    // Cible du curseur : la piste seulement, pas le libellé.
    Item {
        id: hit
        width: Theme.varietyTrack
        height: root.height
        anchors.verticalCenter: parent.verticalCenter
        HoverHandler { cursorShape: Qt.PointingHandCursor }
        TapHandler { onTapped: (event) => root.pickAt(event.point.position.x) }
        DragHandler {
            id: dragHandler
            xAxis.enabled: true
            yAxis.enabled: false
            onActiveChanged: if (active) root.pickAt(centroid.position.x)
            onCentroidChanged: if (active) root.pickAt(centroid.position.x)
        }
    }

    Text {
        id: label
        anchors.verticalCenter: parent.verticalCenter
        x: Theme.varietyTrack + 8
        width: Theme.varietyLabel
        text: root.labels[root.index]
        color: root.activeFocus || hover.hovered ? Theme.textPrimary : Theme.textSecondary
        elide: Text.ElideRight
        font.family: Theme.fontUi
        font.pixelSize: 11
        font.weight: Font.Medium
        Behavior on color { ColorAnimation { duration: Theme.quick } }
    }

    HoverHandler { id: hover }

    ToolTip.visible: hover.hovered
    ToolTip.delay: 400
    ToolTip.text: root.hints[root.index]

    Accessible.role: Accessible.Slider
    Accessible.name: "Variété de recherche : " + root.labels[root.index]
    Accessible.description: root.hints[root.index]
    Accessible.focusable: true
    Accessible.onIncreaseAction: root.step(1)
    Accessible.onDecreaseAction: root.step(-1)
}

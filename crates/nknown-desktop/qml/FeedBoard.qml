// Flux d'info de l'accueil : pour chaque flux (un titre, des mots-clés surveillés en continu),
// ses derniers contenus, les nouveaux marqués d'un point. Un clic lit le contenu ; « Tout voir »
// lance la recherche ; une carte « + » crée un flux, le crayon le modifie.
import QtQuick
import QtQuick.Controls.Basic

Item {
    id: board
    property var feeds: []
    signal read(var hit, string feedId)
    signal search(string query)
    signal save(string id, string title, string keywords)
    signal remove(string id)

    /// Flux en cours de modification (`"new"` : création), vide sinon.
    property string editing: ""

    readonly property int columns: Math.max(1, Math.min(3, Math.floor((width + gap) / (minCard + gap))))
    readonly property int minCard: 320
    readonly property int gap: 16
    readonly property real cardWidth: (Math.min(width, 1180) - (columns - 1) * gap) / columns

    Flickable {
        anchors.fill: parent
        contentHeight: flow.implicitHeight + 24
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        Flow {
            id: flow
            width: Math.min(board.width, 1180)
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: board.gap

            Repeater {
                model: board.feeds
                delegate: FeedCard {
                    required property var modelData
                    feed: modelData
                    width: board.cardWidth
                }
            }

            // Nouveau flux.
            Rectangle {
                width: board.cardWidth
                height: board.editing === "new" ? editor.implicitHeight + 32 : 64
                radius: Theme.radiusCard
                color: addArea.containsMouse && board.editing !== "new" ? Theme.bgHover : "transparent"
                border.color: Theme.lineSubtle
                border.width: 1
                Behavior on height { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }
                Text {
                    anchors.centerIn: parent
                    visible: board.editing !== "new"
                    text: board.feeds.length === 0 ? "+  Suivre des mots-clés" : "+  Nouveau flux"
                    color: addArea.containsMouse ? Theme.accent : Theme.textSecondary
                    font.pixelSize: 13
                    font.weight: Font.Medium
                }
                MouseArea {
                    id: addArea
                    anchors.fill: parent
                    enabled: board.editing !== "new"
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: board.editing = "new"
                }
                FeedEditor {
                    id: editor
                    visible: board.editing === "new"
                    anchors.fill: parent
                    anchors.margins: 16
                    onDone: (title, keywords) => { board.save("", title, keywords); board.editing = "" }
                    onCancel: board.editing = ""
                }
            }
        }
    }

    // ------------------------------------------------------------------ une carte
    component FeedCard: Rectangle {
        id: card
        property var feed
        readonly property bool editingThis: board.editing === feed.id
        implicitHeight: editingThis ? cardEditor.implicitHeight + 32 : body.implicitHeight + 32
        height: implicitHeight
        radius: Theme.radiusCard
        color: Theme.bgRaised
        border.color: feed.fresh > 0 ? Qt.alpha(Theme.accent, 0.35) : Theme.lineSubtle
        border.width: 1
        Behavior on height { NumberAnimation { duration: Theme.standard; easing.type: Easing.OutCubic } }

        HoverHandler { id: cardHover }

        Column {
            id: body
            visible: !card.editingThis
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 16
            spacing: 10

            // En-tête : titre, nouveaux, actions au survol.
            Item {
                width: parent.width
                height: 30
                Text {
                    id: title
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    width: Math.min(implicitWidth, parent.width - badge.width - actions.width - 16)
                    text: card.feed.title
                    elide: Text.ElideRight
                    color: Theme.textPrimary
                    font.pixelSize: 15
                    font.weight: Font.DemiBold
                }
                Rectangle {
                    id: badge
                    anchors.left: title.right
                    anchors.leftMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    visible: card.feed.fresh > 0
                    width: visible ? badgeText.implicitWidth + 14 : 0
                    height: 20
                    radius: 10
                    color: Qt.alpha(Theme.accent, 0.16)
                    Text {
                        id: badgeText
                        anchors.centerIn: parent
                        text: card.feed.fresh + (card.feed.fresh > 1 ? " nouveaux" : " nouveau")
                        color: Theme.accent
                        font.pixelSize: 10
                        font.weight: Font.Medium
                        font.features: { "tnum": 1 }
                    }
                }
                Row {
                    id: actions
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2
                    opacity: cardHover.hovered ? 1 : 0
                    Behavior on opacity { NumberAnimation { duration: Theme.quick } }
                    IconButton { glyph: "✎"; tip: "Modifier"; onClicked: board.editing = card.feed.id }
                    IconButton { glyph: "✕"; tip: "Retirer ce flux"; onClicked: board.remove(card.feed.id) }
                }
            }

            // Mots-clés.
            Flow {
                width: parent.width
                spacing: 6
                Repeater {
                    model: card.feed.keywords
                    Rectangle {
                        required property string modelData
                        height: 20
                        width: kw.implicitWidth + 14
                        radius: 10
                        color: Qt.alpha(Theme.kindColor("term"), 0.12)
                        Text {
                            id: kw
                            anchors.centerIn: parent
                            text: modelData
                            color: Theme.kindColor("term")
                            font.pixelSize: 11
                        }
                    }
                }
            }

            // Derniers contenus.
            Column {
                width: parent.width
                spacing: 2
                Repeater {
                    model: card.feed.items.slice(0, 5)
                    Item {
                        required property var modelData
                        required property int index
                        readonly property bool fresh: index < card.feed.fresh
                        width: parent.width
                        height: 44
                        Rectangle {
                            anchors.fill: parent
                            anchors.leftMargin: -8
                            anchors.rightMargin: -8
                            radius: 8
                            color: itemArea.containsMouse ? Theme.bgHover : "transparent"
                        }
                        Rectangle {
                            x: -6
                            anchors.verticalCenter: parent.verticalCenter
                            width: 5; height: 5; radius: 2.5
                            color: Theme.accent
                            visible: parent.fresh
                        }
                        KindGlyph {
                            id: glyph
                            x: 4
                            anchors.verticalCenter: parent.verticalCenter
                            kind: modelData.is_code ? "code" : "prose"
                            size: 9
                        }
                        Column {
                            anchors.left: glyph.right
                            anchors.leftMargin: 10
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            Text {
                                width: parent.width
                                text: modelData.title
                                elide: Text.ElideRight
                                color: Theme.textPrimary
                                font.pixelSize: 13
                                font.weight: parent.parent.fresh ? Font.DemiBold : Font.Normal
                            }
                            Text {
                                width: parent.width
                                text: (modelData.location || modelData.source) + "  ·  " + Theme.ago(modelData.date)
                                elide: Text.ElideMiddle
                                color: Theme.textSecondary
                                font.pixelSize: 11
                            }
                        }
                        MouseArea {
                            id: itemArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: board.read(modelData, card.feed.id)
                        }
                    }
                }
                Text {
                    visible: card.feed.items.length === 0
                    text: "Rien pour l'instant : les nouveaux contenus apparaîtront ici."
                    color: Theme.textSecondary
                    font.pixelSize: 12
                    wrapMode: Text.Wrap
                    width: parent.width
                }
            }

            Pill {
                quiet: true
                text: "Tout voir"
                visible: card.feed.items.length > 0
                onClicked: board.search(card.feed.keywords.join(" ") + " @new")
            }
        }

        FeedEditor {
            id: cardEditor
            visible: card.editingThis
            anchors.fill: parent
            anchors.margins: 16
            initialTitle: card.feed.title
            initialKeywords: card.feed.keywords.join(", ")
            onDone: (title, keywords) => { board.save(card.feed.id, title, keywords); board.editing = "" }
            onCancel: board.editing = ""
        }
    }

    // ------------------------------------------------------------------ éditeur
    component FeedEditor: Column {
        id: ed
        property string initialTitle: ""
        property string initialKeywords: ""
        signal done(string title, string keywords)
        signal cancel()
        spacing: 10
        onVisibleChanged: if (visible) {
            titleField.text = initialTitle
            keywordsField.text = initialKeywords
            (initialTitle === "" ? titleField : keywordsField).forceActiveFocus()
        }
        function submit() {
            if (keywordsField.text.trim() !== "") ed.done(titleField.text.trim(), keywordsField.text.trim())
        }

        Field { id: titleField; width: parent.width; placeholderText: "Titre (ex. Actus GPU)" }
        Field {
            id: keywordsField
            width: parent.width
            placeholderText: "Mots-clés, séparés par des virgules"
        }
        Row {
            spacing: 8
            Pill { text: ed.initialTitle === "" ? "Créer" : "Enregistrer"; enabled: keywordsField.text.trim() !== ""; onClicked: ed.submit() }
            Pill { quiet: true; text: "Annuler"; onClicked: ed.cancel() }
        }
        Keys.onEscapePressed: ed.cancel()
    }

    component Field: TextField {
        height: 34
        color: Theme.textPrimary
        placeholderTextColor: Theme.textSecondary
        selectionColor: Qt.alpha(Theme.accent, 0.35)
        font.family: Theme.fontUi
        font.pixelSize: 13
        leftPadding: 12
        onAccepted: parent.submit()
        background: Rectangle {
            radius: 8
            color: Theme.bgBase
            border.color: parent.activeFocus ? Qt.alpha(Theme.accent, 0.6) : Theme.lineSubtle
        }
    }
}

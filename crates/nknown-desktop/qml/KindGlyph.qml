// Forme d'une nature de nœud (charte, section 4) : la même dans la carte, la liste et
// l'inspecteur. Requête : anneau ; mot : disque ; qualifiant : losange ; texte : carré arrondi ;
// code : carré arrondi et </> ; source : cercle creux ; apparenté : carré en contour.
import QtQuick

Item {
    id: glyph
    property string kind: "term"
    property real size: 16
    readonly property color tint: Theme.kindColor(kind)
    readonly property bool hollow: kind === "query" || kind === "source" || kind === "related"
    implicitWidth: size
    implicitHeight: size

    Rectangle {
        anchors.centerIn: parent
        width: glyph.kind === "qualifier" ? glyph.width * 0.74 : glyph.width
        height: width
        rotation: glyph.kind === "qualifier" ? 45 : 0
        radius: glyph.kind === "query" || glyph.kind === "term" || glyph.kind === "source"
                ? width / 2
                : glyph.kind === "qualifier" ? 2 : width * 0.28
        // Creux, mais opaque : les liens qui arrivent au centre ne le traversent pas.
        color: glyph.hollow ? Theme.bgBase : glyph.tint
        border.color: glyph.tint
        border.width: glyph.kind === "query" ? Math.max(2, glyph.size * 0.14)
                    : glyph.hollow ? Math.max(1.5, glyph.size * 0.08) : 0
        antialiasing: true
    }
    Text {
        visible: glyph.kind === "code" && glyph.size >= 14
        anchors.centerIn: parent
        text: "</>"
        color: Theme.bgBase
        font.family: Theme.fontCode
        font.pixelSize: glyph.size * 0.36
        font.weight: Font.Bold
    }
}
